#![cfg(unix)]

use std::{
  fs::File,
  io::{self, Read, Write},
  os::{
    fd::{FromRawFd, RawFd},
    unix::process::CommandExt,
  },
  process::{Command, Stdio},
  thread,
  time::{Duration, Instant},
};

const BEGIN_SYNC: &[u8] = b"\x1b[?2026h";
const END_SYNC: &[u8] = b"\x1b[?2026l";

#[test]
fn fixture_reaches_a_styled_first_live_frame_through_the_real_cli() {
  let transcript = replay_fixture(false, 80, 24);
  assert_eq!(count(&transcript, BEGIN_SYNC), count(&transcript, END_SYNC));
  assert!(count(&transcript, BEGIN_SYNC) > 0);
  assert!(transcript.windows(7).any(|w| w == b"\x1b[1;35m"));
  assert!(transcript.windows(4).any(|w| w == b"demo"));

  let frames = synchronized_transactions(&transcript);
  let initial = frames.first().expect("no initial terminal frame");
  assert!(initial.windows(6).any(|window| window == b"Builds"));
  assert!(initial.windows(6).any(|window| window == b"Status"));
  assert!(initial.windows(7).any(|window| window == b"Elapsed"));
  assert!(!initial.windows(4).any(|window| window == b"demo"));
  let first_graph = frames
    .iter()
    .find(|frame| frame.windows(4).any(|window| window == b"demo"))
    .expect("no atomic graph repaint was committed");
  assert!(first_graph.windows(6).any(|window| window == b"Builds"));
  assert!(first_graph.windows(4).any(|window| window == b"demo"));
  assert!(first_graph.windows(6).any(|window| window == b"Status"));
  assert!(first_graph.windows(7).any(|window| window == b"Elapsed"));

  for (needle, description) in [
    (b"checking".as_slice(), "styled log"),
    (b"configuring flags".as_slice(), "over-width log"),
  ] {
    let log_frame = frames
      .iter()
      .find(|frame| frame.windows(needle.len()).any(|window| window == needle))
      .unwrap_or_else(|| panic!("{description} was not committed"));
    assert!(log_frame.windows(6).any(|window| window == b"Builds"));
    assert!(log_frame.windows(4).any(|window| window == b"demo"));
    assert!(log_frame.windows(6).any(|window| window == b"Status"));
    assert!(log_frame.windows(7).any(|window| window == b"Elapsed"));
  }
  assert!(!transcript.windows(3).any(|window| window == b"\x1b[r"));
}

#[test]
fn multiplexer_gets_a_safe_initial_graph_without_live_controls() {
  let transcript = replay_fixture(true, 80, 24);
  assert_eq!(count(&transcript, BEGIN_SYNC), 0);
  assert_eq!(count(&transcript, END_SYNC), 0);
  assert!(!transcript.windows(4).any(|window| window == b"\x1b[2K"));
  assert!(transcript.windows(6).any(|window| window == b"Builds"));
  assert!(transcript.windows(4).any(|window| window == b"demo"));
  assert!(transcript.windows(7).any(|window| window == b"\x1b[1;35m"));
}

#[test]
fn undersized_terminal_falls_back_without_live_control_artifacts() {
  for (columns, rows) in [(19, 8), (80, 7)] {
    let transcript = replay_fixture(false, columns, rows);
    assert_eq!(count(&transcript, BEGIN_SYNC), 0);
    assert_eq!(count(&transcript, END_SYNC), 0);
    assert!(!transcript.windows(4).any(|window| window == b"\x1b[2K"));
    assert!(transcript.windows(8).any(|window| window == b"checking"));
    assert!(transcript.windows(6).any(|window| window == b"Builds"));
    assert!(transcript.windows(7).any(|window| window == b"\x1b[1;35m"));
  }
}

fn replay_fixture(multiplexer: bool, columns: u16, rows: u16) -> Vec<u8> {
  let (master, slave) = open_pty(columns, rows).unwrap();
  let slave = unsafe { File::from_raw_fd(slave) };
  let mut command = Command::new(env!("CARGO_BIN_EXE_rom"));
  command
    .env("TERM", "dumb")
    .env("TERM_PROGRAM", "ghostty")
    .env_remove("STY")
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::from(slave));
  if multiplexer {
    command.env("TMUX", "/tmp/tmux-test,1,0");
  } else {
    command.env_remove("TMUX");
  }
  // SAFETY: this closure only invokes async-signal-safe libc operations. The
  // PTY is installed as fd 2 by Command before the hook executes.
  unsafe {
    command.pre_exec(move || {
      if libc::setsid() == -1 {
        return Err(io::Error::last_os_error());
      }
      if libc::ioctl(2, libc::TIOCSCTTY as _, 0) == -1 {
        return Err(io::Error::last_os_error());
      }
      Ok(())
    });
  }
  let mut child = command.spawn().unwrap();
  // The command retains its stdio configuration; drop it so the parent's copy
  // cannot keep the slave side alive after the child exits.
  drop(command);
  let reader = thread::spawn(move || read_terminal(master));

  let fixture = include_str!("fixtures/download-progress/events.jsonl");
  let mut stdin = child.stdin.take().unwrap();
  let started = Instant::now();
  for line in fixture.lines().filter(|line| !line.trim().is_empty()) {
    let event: serde_json::Value = serde_json::from_str(line).unwrap();
    let at = Duration::from_millis(event["at_ms"].as_u64().unwrap());
    thread::sleep(at.saturating_sub(started.elapsed()));
    match event["type"].as_str().unwrap() {
      "write" => {
        stdin
          .write_all(event["text"].as_str().unwrap().as_bytes())
          .unwrap();
        stdin.flush().unwrap();
      },
      "checkpoint" => {},
      "eof" => break,
      other => panic!("unknown fixture event: {other}"),
    }
  }
  drop(stdin);
  let status = child.wait().unwrap();
  assert!(status.success());
  reader.join().unwrap()
}

fn open_pty(columns: u16, rows: u16) -> io::Result<(RawFd, RawFd)> {
  let mut master = -1;
  let mut slave = -1;
  let size = libc::winsize {
    ws_row:    rows,
    ws_col:    columns,
    ws_xpixel: 0,
    ws_ypixel: 0,
  };
  // SAFETY: all output pointers are valid and `size` is initialized.
  if unsafe {
    libc::openpty(
      &mut master,
      &mut slave,
      std::ptr::null_mut(),
      std::ptr::null(),
      &size,
    )
  } == -1
  {
    return Err(io::Error::last_os_error());
  }
  Ok((master, slave))
}

fn read_terminal(master: RawFd) -> Vec<u8> {
  // SAFETY: `master` is uniquely owned by this thread.
  let mut terminal = unsafe { File::from_raw_fd(master) };
  let mut transcript = Vec::new();
  let mut buffer = [0_u8; 4096];
  loop {
    match terminal.read(&mut buffer) {
      Ok(0) => break,
      Ok(count) => {
        transcript.extend_from_slice(&buffer[..count]);
      },
      Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
      Err(error) => panic!("PTY read failed: {error}"),
    }
  }
  transcript
}

fn synchronized_transactions(transcript: &[u8]) -> Vec<&[u8]> {
  let mut result = Vec::new();
  let mut remaining = transcript;
  while let Some(begin) = find(remaining, BEGIN_SYNC) {
    let transaction = &remaining[begin..];
    let Some(end) = find(transaction, END_SYNC) else {
      panic!("unterminated synchronized transaction");
    };
    let end = end + END_SYNC.len();
    result.push(&transaction[..end]);
    remaining = &transaction[end..];
  }
  result
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
  haystack
    .windows(needle.len())
    .position(|window| window == needle)
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
  haystack
    .windows(needle.len())
    .filter(|window| *window == needle)
    .count()
}
