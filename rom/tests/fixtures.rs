use std::{
  fs,
  path::{Path, PathBuf},
};

use rom::{
  display::{format_log, render_frame},
  monitor::{Output, StreamEngine},
  types::{EngineConfig, IconMode, RenderConfig},
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
  Write {
    at_ms: u64,
    #[serde(default)]
    text:  Option<String>,
    #[serde(default)]
    bytes: Option<Vec<u8>>,
  },
  Checkpoint {
    at_ms: u64,
    name:  String,
  },
  Eof {
    at_ms: u64,
  },
}

impl Event {
  const fn at_ms(&self) -> u64 {
    match self {
      Self::Write { at_ms, .. }
      | Self::Checkpoint { at_ms, .. }
      | Self::Eof { at_ms } => *at_ms,
    }
  }
}

fn fixture_root(name: &str) -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("tests/fixtures")
    .join(name)
}

fn read_events(root: &Path) -> Vec<Event> {
  let input = fs::read_to_string(root.join("events.jsonl")).unwrap();
  let mut previous = 0;
  input
    .lines()
    .enumerate()
    .filter_map(|(index, line)| {
      let line = line.trim();
      if line.is_empty() || line.starts_with('#') {
        return None;
      }
      let event: Event = serde_json::from_str(line).unwrap_or_else(|error| {
        panic!("{}:{}: {error}", root.display(), index + 1)
      });
      assert!(event.at_ms() >= previous, "timestamps must be monotonic");
      previous = event.at_ms();
      Some(event)
    })
    .collect()
}

fn assert_or_bless(path: &Path, actual: &str) {
  if std::env::var_os("ROM_BLESS_EXPECTED").is_some() {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("{actual}\n")).unwrap();
    return;
  }
  let expected = fs::read_to_string(path).unwrap_or_else(|_| {
    panic!(
      "missing expectation {}; run ROM_BLESS_EXPECTED=1 cargo test fixture_",
      path.display()
    )
  });
  let actual = format!("{actual}\n");
  if expected != actual {
    panic!(
      "fixture output mismatch for {}\n{}",
      path.display(),
      simple_diff(&expected, &actual),
    );
  }
}

fn simple_diff(expected: &str, actual: &str) -> String {
  let expected: Vec<_> = expected.lines().collect();
  let actual: Vec<_> = actual.lines().collect();
  let mut diff = String::from("--- expected\n+++ actual\n");
  for index in 0..expected.len().max(actual.len()) {
    match (expected.get(index), actual.get(index)) {
      (Some(left), Some(right)) if left == right => {
        diff.push_str(&format!(" {left}\n"));
      },
      (Some(left), Some(right)) => {
        diff.push_str(&format!("-{left}\n+{right}\n"));
      },
      (Some(left), None) => diff.push_str(&format!("-{left}\n")),
      (None, Some(right)) => diff.push_str(&format!("+{right}\n")),
      (None, None) => {},
    }
  }
  diff
}

fn fixture_render_config(ansi: bool) -> RenderConfig {
  RenderConfig {
    ansi,
    icons: IconMode::Unicode,
    ..RenderConfig::default()
  }
}

fn append_output(
  target: &mut Vec<u8>,
  output: &[Output],
  render: &RenderConfig,
) {
  for output in output {
    match output {
      Output::Passthrough(bytes) => target.extend(bytes),
      Output::Log(line) => {
        let line = format_log(line, render);
        target.extend(line.as_bytes());
        target.push(b'\n');
      },
    }
  }
}

fn visible_escapes(value: &str) -> String {
  value.replace('\x1b', "\\e")
}

fn append_frame(
  target: &mut String,
  event_index: usize,
  at_ms: u64,
  kind: &str,
  text: &str,
) {
  target.push_str(&format!(
    "--- event {event_index} @ {at_ms}ms: {kind} ---\n"
  ));
  target.push_str(text);
  target.push('\n');
}

fn replay(name: &str) {
  let root = fixture_root(name);
  let mut stream = StreamEngine::new(EngineConfig::default());
  let plain_render = fixture_render_config(false);
  let ansi_render = fixture_render_config(true);
  let mut plain_output = Vec::new();
  let mut ansi_output = Vec::new();
  let mut plain_frames = String::new();
  let mut ansi_frames = String::new();
  let mut finished = false;
  for (event_index, event) in read_events(&root).into_iter().enumerate() {
    let now = event.at_ms() as f64 / 1000.0;
    let at_ms = event.at_ms();
    match event {
      Event::Write { text, bytes, .. } => {
        let payload = match (text, bytes) {
          (Some(text), None) => text.into_bytes(),
          (None, Some(bytes)) => bytes,
          _ => panic!("write event needs exactly one of text or bytes"),
        };
        let processed = stream.push_at(&payload, now).unwrap();
        append_output(&mut plain_output, &processed.output, &plain_render);
        append_output(&mut ansi_output, &processed.output, &ansi_render);
        let frame = render_frame(
          stream.engine().state(),
          &plain_render,
          now,
          79,
          23,
          finished,
        );
        append_frame(
          &mut plain_frames,
          event_index,
          at_ms,
          "write",
          &frame.text(),
        );
        append_frame(
          &mut ansi_frames,
          event_index,
          at_ms,
          "write",
          &visible_escapes(&frame.ansi_text()),
        );
      },
      Event::Checkpoint { name, .. } => {
        let frame = render_frame(
          stream.engine().state(),
          &plain_render,
          now,
          79,
          23,
          finished,
        );
        assert_or_bless(
          &root
            .join("expected/plain-79x23")
            .join(format!("{name}.txt")),
          &frame.text(),
        );
        assert_or_bless(
          &root.join("expected/ansi-79x23").join(format!("{name}.txt")),
          &visible_escapes(&frame.ansi_text()),
        );
        append_frame(
          &mut plain_frames,
          event_index,
          at_ms,
          &format!("checkpoint {name}"),
          &frame.text(),
        );
        append_frame(
          &mut ansi_frames,
          event_index,
          at_ms,
          &format!("checkpoint {name}"),
          &visible_escapes(&frame.ansi_text()),
        );
      },
      Event::Eof { .. } => {
        let finished_output = stream.finish_at(now).unwrap();
        append_output(
          &mut plain_output,
          &finished_output.output,
          &plain_render,
        );
        append_output(&mut ansi_output, &finished_output.output, &ansi_render);
        finished = true;
        let frame = render_frame(
          stream.engine().state(),
          &plain_render,
          now,
          79,
          23,
          true,
        );
        append_frame(
          &mut plain_frames,
          event_index,
          at_ms,
          "eof",
          &frame.text(),
        );
        append_frame(
          &mut ansi_frames,
          event_index,
          at_ms,
          "eof",
          &visible_escapes(&frame.ansi_text()),
        );
      },
    }
  }
  assert!(finished, "fixture must contain an eof event");
  let plain_output = String::from_utf8(plain_output).unwrap();
  let ansi_output = String::from_utf8(ansi_output).unwrap();
  assert!(!plain_output.contains('\x1b'));
  assert!(ansi_output.contains("\x1b["));
  assert!(ansi_output.contains("\x1b[0;34m"));
  assert_or_bless(
    &root.join("expected/plain-79x23/stream.txt"),
    plain_output.trim_end_matches('\n'),
  );
  assert_or_bless(
    &root.join("expected/ansi-79x23/stream.txt"),
    visible_escapes(&ansi_output).trim_end_matches('\n'),
  );
  assert_or_bless(
    &root.join("expected/plain-79x23/frames.txt"),
    plain_frames.trim_end_matches('\n'),
  );
  assert_or_bless(
    &root.join("expected/ansi-79x23/frames.txt"),
    ansi_frames.trim_end_matches('\n'),
  );
}

#[test]
fn fixture_download_progress() {
  replay("download-progress");
}

#[test]
fn fixture_nixpkgs_hello() {
  replay("nixpkgs-hello");
}

#[test]
fn fixture_nested_builds() {
  replay("nested-builds");
}
