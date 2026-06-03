use std::collections::VecDeque;

use rom_core::tui::TuiView;

pub const DEFAULT_TUI_LOG_LINE_LIMIT: usize = 20_000;
pub const DEFAULT_TUI_LIVE_LOG_TAIL: usize = 512;
pub const POST_TUI_ERROR_LINE_LIMIT: usize = 60;

#[derive(Default)]
pub struct LogStore {
  lines: VecDeque<String>,
  limit: Option<usize>,
}

impl LogStore {
  pub fn new(limit: Option<usize>) -> Self {
    Self {
      lines: VecDeque::new(),
      limit,
    }
  }

  pub fn push(&mut self, line: String) {
    self.lines.push_back(line);
    if let Some(limit) = self.limit {
      while self.lines.len() > limit {
        self.lines.pop_front();
      }
    }
  }

  pub fn snapshot(&self, view: Option<&TuiView>) -> Vec<String> {
    let snapshot_len =
      view.and_then(|view| live_log_snapshot_len(view, self.lines.len()));
    match snapshot_len {
      Some(len) => {
        self
          .lines
          .iter()
          .skip(self.lines.len().saturating_sub(len))
          .cloned()
          .collect()
      },
      None => self.lines.iter().cloned().collect(),
    }
  }
}

pub fn live_log_snapshot_len(
  view: &TuiView,
  available: usize,
) -> Option<usize> {
  if !view.search_query.is_empty() {
    return None;
  }

  let scroll = view.log_scroll.min(available);
  Some(
    DEFAULT_TUI_LIVE_LOG_TAIL
      .saturating_add(scroll)
      .min(available),
  )
}

pub fn post_tui_failure_error_lines(
  state: &rom_core::state::State,
  logs: &[String],
) -> Vec<String> {
  let mut lines = Vec::new();

  for line in &state.nix_errors {
    push_unique_error_line(&mut lines, line);
  }
  let nix_error_bodies = state
    .nix_errors
    .iter()
    .map(|line| strip_ansi_for_matching(line))
    .collect::<Vec<_>>();
  for line in logs.iter().filter(|line| is_error_log_line(line)) {
    if log_line_is_covered_by_nix_error(line, &nix_error_bodies) {
      continue;
    }
    push_unique_error_line(&mut lines, line);
  }

  if lines.is_empty() {
    for line in logs {
      push_unique_error_line(&mut lines, line);
    }
  }

  tail_with_omission(lines, POST_TUI_ERROR_LINE_LIMIT)
}

fn push_unique_error_line(lines: &mut Vec<String>, line: &str) {
  let line = line.trim_end();
  if line.is_empty() {
    return;
  }
  if !lines.iter().any(|existing| existing == line) {
    lines.push(line.to_string());
  }
}

fn log_line_is_covered_by_nix_error(line: &str, nix_errors: &[String]) -> bool {
  let normalized = strip_ansi_for_matching(line);
  let normalized = normalized.trim();
  if normalized.is_empty() {
    return true;
  }

  normalized
    .lines()
    .map(str::trim)
    .filter(|fragment| !fragment.is_empty())
    .all(|fragment| log_fragment_is_covered_by_nix_error(fragment, nix_errors))
}

fn log_fragment_is_covered_by_nix_error(
  fragment: &str,
  nix_errors: &[String],
) -> bool {
  let mut candidates = vec![fragment];
  if let Some(unprefixed_error) = fragment.strip_prefix("error:") {
    candidates.push(unprefixed_error.trim());
  }
  if let Some((_, unprefixed_log)) = fragment.split_once("> ") {
    let unprefixed_log = unprefixed_log.trim();
    candidates.push(unprefixed_log);
    if let Some(unprefixed_error) = unprefixed_log.strip_prefix("error:") {
      candidates.push(unprefixed_error.trim());
    }
  }

  candidates.into_iter().any(|candidate| {
    !candidate.is_empty()
      && nix_errors.iter().any(|error| error.contains(candidate))
  })
}

fn is_error_log_line(line: &str) -> bool {
  let normalized = strip_ansi_for_matching(line).to_ascii_lowercase();
  normalized.starts_with("error")
    || normalized.contains(" error:")
    || normalized.contains("error[")
    || normalized.contains("failed")
    || normalized.contains("failure")
}

fn strip_ansi_for_matching(line: &str) -> String {
  let mut stripped = String::with_capacity(line.len());
  let mut chars = line.chars().peekable();

  while let Some(ch) = chars.next() {
    if ch == '\x1b' && chars.peek() == Some(&'[') {
      chars.next();
      for code in chars.by_ref() {
        if ('@'..='~').contains(&code) {
          break;
        }
      }
      continue;
    }
    stripped.push(ch);
  }

  stripped
}

fn tail_with_omission(lines: Vec<String>, limit: usize) -> Vec<String> {
  if lines.len() <= limit {
    return lines;
  }

  let omitted = lines.len() - limit;
  let mut tail = Vec::with_capacity(limit + 1);
  tail.push(format!("... {omitted} earlier error line(s) omitted"));
  tail.extend(lines.into_iter().skip(omitted));
  tail
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn post_tui_failure_errors_include_nix_and_build_errors() {
    let mut state = rom_core::state::State::new();
    state.nix_errors.push(
      "error: builder for '/nix/store/foo.drv' failed with exit code 1"
        .to_string(),
    );
    let logs = vec![
      "checking inputs".to_string(),
      "src/main.rs:7: error[E0425]: cannot find value `x`".to_string(),
      "error: builder for '/nix/store/foo.drv' failed with exit code 1"
        .to_string(),
    ];

    let lines = post_tui_failure_error_lines(&state, &logs);

    assert_eq!(lines, vec![
      "error: builder for '/nix/store/foo.drv' failed with exit code 1",
      "src/main.rs:7: error[E0425]: cannot find value `x`",
    ]);
  }

  #[test]
  fn post_tui_failure_errors_match_ansi_colored_errors() {
    let state = rom_core::state::State::new();
    let logs = vec!["\x1b[31merror:\x1b[0m configure failed".to_string()];

    let lines = post_tui_failure_error_lines(&state, &logs);

    assert_eq!(lines, vec!["\x1b[31merror:\x1b[0m configure failed"]);
  }

  #[test]
  fn post_tui_failure_errors_skip_logs_covered_by_nix_error() {
    let mut state = rom_core::state::State::new();
    state.nix_errors.push(
      "Cannot build '/nix/store/foo.drv'.\nReason: builder failed with exit \
       code 23.\nLast 2 log lines:\n> rom test builder line\n> error: rom \
       test builder failed on stderr"
        .to_string(),
    );
    let logs = vec![
      "drv> error: rom test builder failed on stderr".to_string(),
      "error: Cannot build '/nix/store/foo.drv'.".to_string(),
      "Reason: builder failed with exit code 23.".to_string(),
    ];

    let lines = post_tui_failure_error_lines(&state, &logs);

    assert_eq!(lines, vec![state.nix_errors[0].clone()]);
  }

  #[test]
  fn post_tui_failure_errors_fall_back_to_bounded_log_tail() {
    let state = rom_core::state::State::new();
    let logs = (0..70)
      .map(|index| format!("log line {index:02}"))
      .collect::<Vec<_>>();

    let lines = post_tui_failure_error_lines(&state, &logs);

    assert_eq!(lines.len(), POST_TUI_ERROR_LINE_LIMIT + 1);
    assert_eq!(
      lines.first().unwrap(),
      "... 10 earlier error line(s) omitted"
    );
    assert_eq!(lines[1], "log line 10");
    assert_eq!(lines.last().unwrap(), "log line 69");
  }

  #[test]
  fn live_log_snapshot_follows_bounded_tail() {
    let view = TuiView::default();

    assert_eq!(
      live_log_snapshot_len(&view, DEFAULT_TUI_LIVE_LOG_TAIL + 100),
      Some(DEFAULT_TUI_LIVE_LOG_TAIL)
    );
  }

  #[test]
  fn live_log_snapshot_expands_for_scrollback_and_search() {
    let mut view = TuiView {
      log_scroll: 100,
      ..TuiView::default()
    };

    assert_eq!(
      live_log_snapshot_len(&view, DEFAULT_TUI_LIVE_LOG_TAIL + 1000),
      Some(DEFAULT_TUI_LIVE_LOG_TAIL + 100)
    );

    view.search_query = "configure".to_string();
    assert_eq!(live_log_snapshot_len(&view, 10_000), None);
  }
}
