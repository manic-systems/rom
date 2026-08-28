//! Terminal admission and atomic normal-screen composition.

use std::io::{self, IsTerminal, Write};

use crossterm::{
  cursor::{Hide, MoveToColumn, MoveToPreviousLine, Show},
  queue,
  terminal::{Clear, ClearType},
};

use crate::{
  display::{Frame, render_frame, write_final},
  state::State,
  types::RenderConfig,
};

const BEGIN_SYNC: &[u8] = b"\x1b[?2026h";
const END_SYNC: &[u8] = b"\x1b[?2026l";
const MIN_LIVE_COLUMNS: u16 = 20;
const MIN_LIVE_ROWS: u16 = 8;

/// Why live presentation was or was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
  Live,
  NotATerminal,
  Multiplexer,
}

/// Select live redraws for every direct interactive terminal.
///
/// Like nix-output-monitor, ROM sends synchronized-update markers without a
/// capability round trip. Terminals which do not implement mode 2026 ignore
/// the private mode while still understanding the ordinary cursor operations.
/// Multiplexers remain append-only until their passthrough behavior has an
/// explicit compatibility test.
#[must_use]
pub fn admission() -> Admission {
  admission_from(
    io::stderr().is_terminal(),
    std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some(),
  )
}

const fn admission_from(is_terminal: bool, multiplexer: bool) -> Admission {
  if !is_terminal {
    Admission::NotATerminal
  } else if multiplexer {
    Admission::Multiplexer
  } else {
    Admission::Live
  }
}

/// Owns the live graph region and serializes all terminal output.
pub struct LiveTerminal<W: Write> {
  writer:       W,
  graph_height: u16,
  initialized:  bool,
  retired:      bool,
  finished:     bool,
  pending:      Vec<u8>,
}

impl<W: Write> LiveTerminal<W> {
  #[must_use]
  pub const fn new(writer: W) -> Self {
    Self {
      writer,
      graph_height: 0,
      initialized: false,
      retired: false,
      finished: false,
      pending: Vec::new(),
    }
  }

  #[must_use]
  pub const fn is_retired(&self) -> bool {
    self.retired
  }

  /// Render with bounded resize retries while the synchronized transaction is
  /// still only an in-memory byte string.
  pub fn render(
    &mut self,
    state: &State,
    config: &RenderConfig,
    now: f64,
    final_render: bool,
  ) -> io::Result<bool> {
    if self.retired {
      return Ok(false);
    }
    if !self.pending.is_empty() && !self.pending.ends_with(b"\n") {
      if final_render {
        self.retire()?;
      }
      return Ok(false);
    }
    for _ in 0..3 {
      let (columns, rows) = crossterm::terminal::size()?;
      if !live_geometry(columns, rows) {
        self.retire()?;
        return Ok(false);
      }
      let frame =
        render_frame(state, config, now, columns - 1, rows - 1, final_render);
      let bytes = self.compose(&frame)?;
      if crossterm::terminal::size()? != (columns, rows) {
        continue;
      }
      self.writer.write_all(&bytes)?;
      self.writer.flush()?;
      self.pending.clear();
      self.graph_height = frame.height.min(rows - 1);
      self.initialized = true;
      return Ok(true);
    }
    self.retire()?;
    Ok(false)
  }

  fn compose(&self, frame: &Frame) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(BEGIN_SYNC);
    queue!(bytes, Hide)?;
    self.queue_clear_graph(&mut bytes)?;
    bytes.extend_from_slice(&self.pending);
    queue!(bytes, MoveToColumn(0))?;
    bytes
      .extend_from_slice(&frame.ansi_text().replace('\n', "\r\n").into_bytes());
    queue!(bytes, Show)?;
    bytes.extend_from_slice(END_SYNC);
    Ok(bytes)
  }

  fn queue_clear_graph(&self, bytes: &mut Vec<u8>) -> io::Result<()> {
    if !self.initialized {
      return Ok(());
    }
    queue!(bytes, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
    for _ in 1..self.graph_height {
      queue!(bytes, MoveToPreviousLine(1), Clear(ClearType::CurrentLine))?;
    }
    Ok(())
  }

  /// Buffer exact output bytes for the next atomic logs-plus-graph commit.
  ///
  /// This deliberately matches nix-output-monitor: ROM does not interpret or
  /// classify controls belonging to the producer.
  pub fn write_passthrough(&mut self, input: &[u8]) -> io::Result<()> {
    if input.is_empty() {
      return Ok(());
    }
    if self.retired {
      self.writer.write_all(input)?;
      self.writer.flush()
    } else {
      self.pending.extend_from_slice(input);
      Ok(())
    }
  }

  pub fn retire(&mut self) -> io::Result<()> {
    if self.retired {
      return Ok(());
    }
    self.retired = true;
    self.clear_graph()?;
    self.writer.flush()
  }

  fn clear_graph(&mut self) -> io::Result<()> {
    let mut bytes = Vec::new();
    if self.initialized {
      bytes.extend_from_slice(BEGIN_SYNC);
      queue!(bytes, Hide)?;
      self.queue_clear_graph(&mut bytes)?;
    }
    bytes.extend_from_slice(&self.pending);
    if self.initialized {
      queue!(bytes, Show)?;
      bytes.extend_from_slice(END_SYNC);
    }
    self.writer.write_all(&bytes)?;
    self.pending.clear();
    self.initialized = false;
    self.graph_height = 0;
    Ok(())
  }

  /// Leave the final frame in normal scrollback and restore terminal modes.
  pub fn finish(&mut self) -> io::Result<()> {
    if self.finished {
      return Ok(());
    }
    if !self.retired && self.initialized {
      let mut bytes = Vec::new();
      bytes.extend_from_slice(BEGIN_SYNC);
      queue!(bytes, Show, MoveToColumn(0))?;
      bytes.extend_from_slice(b"\r\n");
      bytes.extend_from_slice(END_SYNC);
      self.writer.write_all(&bytes)?;
      self.writer.flush()?;
    }
    self.finished = true;
    Ok(())
  }

  /// Append a final frame constrained to the current terminal when live
  /// rendering retired, for example because the window was too small.
  pub fn append_final(
    &mut self,
    state: &State,
    config: &RenderConfig,
    now: f64,
  ) -> io::Result<()> {
    let mut config = config.clone();
    if let Ok((columns, rows)) = crossterm::terminal::size() {
      config.width.get_or_insert(columns);
      config.height.get_or_insert(rows);
    }
    write_final(self, state, &config, now)
  }
}

const fn live_geometry(columns: u16, rows: u16) -> bool {
  columns >= MIN_LIVE_COLUMNS && rows >= MIN_LIVE_ROWS
}

impl<W: Write> Drop for LiveTerminal<W> {
  fn drop(&mut self) {
    if !self.finished && !self.retired {
      let _ = self.retire();
    }
  }
}

impl<W: Write> Write for LiveTerminal<W> {
  fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
    self.write_passthrough(buffer)?;
    Ok(buffer.len())
  }

  fn flush(&mut self) -> io::Result<()> {
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn frame_commit_is_one_synchronized_full_repaint() {
    let config = RenderConfig::default();
    let frame = render_frame(&State::new(), &config, 0.0, 39, 8, true);
    let terminal = LiveTerminal::new(Vec::<u8>::new());
    let bytes = terminal.compose(&frame).unwrap();
    assert_eq!(count(&bytes, BEGIN_SYNC), 1);
    assert_eq!(count(&bytes, END_SYNC), 1);
    assert!(!bytes.windows(4).any(|window| window == b"\x1b[2J"));
    assert!(!bytes.windows(3).any(|window| window == b"\x1b[r"));
  }

  #[test]
  fn first_build_event_is_present_in_the_first_atomic_commit() {
    let mut stream =
      crate::monitor::StreamEngine::new(crate::types::EngineConfig::default());
    stream
      .push_at(
        b"@nix {\"action\":\"start\",\"id\":10,\"level\":3,\"parent\":0,\"text\":\"building '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-demo.drv'\",\"type\":105,\"fields\":[\"/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-demo.drv\",\"\",1,1]}\n",
        0.0,
      )
      .unwrap();
    let frame = render_frame(
      stream.engine().state(),
      &RenderConfig::default(),
      0.0,
      79,
      23,
      false,
    );
    assert!(frame.text().starts_with("┏━ Builds\n┣━"));
    let terminal = LiveTerminal::new(Vec::<u8>::new());
    let bytes = terminal.compose(&frame).unwrap();
    assert_eq!(count(&bytes, BEGIN_SYNC), 1);
    assert_eq!(count(&bytes, END_SYNC), 1);
    assert!(bytes.windows(4).any(|window| window == b"demo"));
  }

  #[test]
  fn pending_logs_and_complete_graph_share_one_atomic_commit() {
    let config = RenderConfig::default();
    let frame = render_frame(&State::new(), &config, 0.0, 39, 8, true);
    let mut terminal = LiveTerminal::new(Vec::<u8>::new());
    terminal.initialized = true;
    terminal.graph_height = 2;
    terminal
      .pending
      .extend_from_slice(b"a deliberately very long wrapped log line\n");
    let bytes = terminal.compose(&frame).unwrap();
    let log = find(&bytes, b"deliberately").unwrap();
    let graph = find(&bytes, b"Finished").unwrap();
    assert!(log < graph);
    assert_eq!(count(&bytes, BEGIN_SYNC), 1);
    assert_eq!(count(&bytes, END_SYNC), 1);
    assert!(!bytes.windows(3).any(|window| window == b"\x1b[r"));
  }

  #[test]
  fn producer_controls_are_buffered_without_retiring_the_graph() {
    let mut terminal = LiveTerminal::new(Vec::<u8>::new());
    terminal.graph_height = 2;
    terminal.initialized = true;
    terminal
      .write_passthrough(b"progress\rreplacement\x1b[K\n")
      .unwrap();
    assert!(!terminal.retired);
    assert_eq!(terminal.pending, b"progress\rreplacement\x1b[K\n");
  }

  #[test]
  fn direct_terminals_are_live_and_multiplexers_fail_closed() {
    assert_eq!(admission_from(false, false), Admission::NotATerminal);
    assert_eq!(admission_from(true, true), Admission::Multiplexer);
    assert_eq!(admission_from(true, false), Admission::Live);
  }

  #[test]
  fn live_rendering_requires_room_for_a_connected_graph() {
    assert!(!live_geometry(MIN_LIVE_COLUMNS - 1, MIN_LIVE_ROWS));
    assert!(!live_geometry(MIN_LIVE_COLUMNS, MIN_LIVE_ROWS - 1));
    assert!(live_geometry(MIN_LIVE_COLUMNS, MIN_LIVE_ROWS));

    let frame = render_frame(
      &State::new(),
      &RenderConfig::default(),
      0.0,
      MIN_LIVE_COLUMNS - 1,
      MIN_LIVE_ROWS - 1,
      false,
    );
    let text = frame.text();
    assert!(text.starts_with("┏━ Builds\n"), "{text}");
    assert!(text.lines().last().unwrap().starts_with("┗━"), "{text}");
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
}
