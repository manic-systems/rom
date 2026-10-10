//! Pure Ratatui rendering.
//!
//! This module does not own a terminal. It turns immutable state into a full
//! buffer; terminal admission, synchronized commits, and log insertion live in
//! [`crate::terminal`]. Keeping that boundary makes rendering deterministic in
//! ordinary Rust tests.

mod formats;
mod frame;
mod model;
mod summary;
mod tree;

use std::{collections::HashSet, path::Path};

pub use frame::{Frame, format_log, write_final};
use ratatui_core::{
  buffer::Buffer,
  layout::Rect,
  style::{Color, Style},
  text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use self::model::{Direction, RenderSnapshot, Transfer, aggregate_transfers};
use crate::{
  icons::Icons,
  state::{DerivationId, SourceFetch, State},
  types::{DisplayFormat, RenderConfig},
};

/// Formats a duration without introducing sub-second redraw noise.
#[must_use]
pub fn format_duration(secs: f64) -> String {
  let secs = secs.max(0.0) as u64;
  if secs < 60 {
    format!("{secs}s")
  } else if secs < 3600 {
    format!("{}m{}s", secs / 60, secs % 60)
  } else {
    format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
  }
}

/// Formats a byte count with binary units, such as `1.5 MiB`.
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
  const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
  let mut value = bytes as f64;
  let mut unit = 0;
  while value >= 1024.0 && unit + 1 < UNITS.len() {
    value /= 1024.0;
    unit += 1;
  }
  if unit == 0 {
    format!("{bytes} B")
  } else if value >= 10.0 {
    format!("{value:.0} {}", UNITS[unit])
  } else {
    format!("{value:.1} {}", UNITS[unit])
  }
}

struct Renderer<'a> {
  snapshot:   RenderSnapshot<'a>,
  config:     &'a RenderConfig,
  icons:      &'static Icons,
  now:        f64,
  width:      u16,
  max_height: u16,
  seen:       HashSet<DerivationId>,
}

impl<'a> Renderer<'a> {
  fn new(
    state: &'a State,
    config: &'a RenderConfig,
    icons: &'static Icons,
    now: f64,
    width: u16,
    max_height: u16,
  ) -> Self {
    Self {
      snapshot: RenderSnapshot::new(state, now),
      config,
      icons,
      now,
      width,
      max_height,
      seen: HashSet::new(),
    }
  }

  fn render(mut self, final_render: bool) -> Vec<Line<'static>> {
    if final_render {
      self
        .snapshot
        .placed_transfers
        .retain(|item| !item.transfer.completed);
    }

    if matches!(self.config.format, DisplayFormat::Tree) {
      if final_render && self.max_height <= 1 {
        let (text, color) = self.final_status();
        return vec![fit_line(vec![self.span(text, color)], self.width)];
      }
      let tail = if final_render {
        self.final_summary(true)
      } else {
        self.legend()
      };
      let plan = self.tree_plan();
      let (tail, graph_budget) =
        self.tree_layout(tail, plan.line_count(), final_render);
      let mut lines = self.render_tree(&plan, graph_budget);
      lines.extend(tail);
      return lines;
    }

    let mut lines = match self.config.format {
      DisplayFormat::Tree => unreachable!("tree frames are budgeted above"),
      DisplayFormat::Plain => self.plain(),
      DisplayFormat::Dashboard => self.dashboard(),
    };

    if final_render && matches!(self.config.format, DisplayFormat::Plain) {
      lines.extend(self.final_summary(false));
    }

    let maximum = usize::from(self.max_height.max(1));
    if lines.len() > maximum {
      if final_render && maximum == 1 {
        let (text, color) = self.final_status();
        return vec![fit_line(vec![self.span(text, color)], self.width)];
      }
      if final_render {
        let final_line = lines.pop().expect("final render has a summary");
        lines.truncate(maximum.saturating_sub(1));
        lines.push(final_line);
        return lines;
      }
      let hidden = lines.len() - maximum + 1;
      lines.truncate(maximum.saturating_sub(1));
      lines.push(Line::from(vec![
        self.span(
          if matches!(self.config.format, DisplayFormat::Plain) {
            ""
          } else {
            "\u{2570}\u{2500} " // ╰─
          },
          self.config.theme.connector,
        ),
        self.span(format!("… {hidden} hidden"), self.config.theme.muted),
      ]));
    }
    lines
  }

  fn tree_layout(
    &self,
    tail: Vec<Line<'static>>,
    graph_lines: usize,
    final_render: bool,
  ) -> (Vec<Line<'static>>, usize) {
    let maximum = usize::from(self.max_height.max(1));
    // Final status always gets a row; live legends retain their usual policy.
    let minimum_graph =
      graph_lines.min(2).min(maximum - usize::from(final_render));
    let tail = preserve_last(tail, maximum.saturating_sub(minimum_graph));
    let graph_limit = if final_render {
      maximum
    } else {
      (maximum.saturating_mul(2) / 3).max(1)
    };
    let graph_budget = graph_limit.min(maximum.saturating_sub(tail.len()));

    (tail, graph_budget)
  }

  const fn transfer_color(&self, transfer: &Transfer) -> Color {
    if transfer.completed {
      return self.config.theme.completed;
    }
    match transfer.direction {
      Direction::Download => self.config.theme.download,
      Direction::Upload => self.config.theme.upload,
    }
  }

  fn spinner(&self) -> &'static str {
    ["\u{25d0}", "\u{25d3}", "\u{25d1}", "\u{25d2}"] // ◐ ◓ ◑ ◒
      [((self.now * 4.0).max(0.0) as usize) % 4]
  }

  /// Renders a source fetch after `prefix`.
  ///
  /// The row shows the source's file name and how long it has been copying.
  fn source_fetch_line(
    &self,
    mut spans: Vec<Span<'static>>,
    fetch: &SourceFetch,
  ) -> Line<'static> {
    let detail = format!(
      "  {} copying  {}",
      self.spinner(),
      format_duration(self.now - fetch.start)
    );
    let name = Path::new(&fetch.source)
      .file_name()
      .map_or(fetch.source.as_str(), |name| {
        name.to_str().unwrap_or(&fetch.source)
      });
    let available = usize::from(self.width)
      .saturating_sub(spans_width(&spans) + detail.width());
    spans.push(
      self.span(truncate_text(name, available), self.config.theme.running),
    );
    spans.push(self.span(detail, self.config.theme.muted));
    fit_line(spans, self.width)
  }

  fn span(&self, value: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(value.into(), Style::default().fg(color))
  }
}

fn preserve_last(
  mut lines: Vec<Line<'static>>,
  maximum: usize,
) -> Vec<Line<'static>> {
  if lines.len() <= maximum {
    return lines;
  }
  if maximum == 0 {
    return Vec::new();
  }
  let last = lines.pop().expect("non-empty truncated section");
  lines.truncate(maximum - 1);
  lines.push(last);
  lines
}

/// Draws a progress bar: heavy `━` for the finished part, with `╸` for a
/// half cell, and a light `─` track for the rest.
fn progress_bar(
  done: u64,
  total: u64,
  width: usize,
  fill: Color,
  track: Color,
) -> Vec<Span<'static>> {
  if width == 0 || total == 0 {
    return Vec::new();
  }
  let halves = u128::from(done.min(total)).saturating_mul((width * 2) as u128)
    / u128::from(total);
  let full = (halves / 2) as usize;
  let half = halves % 2 == 1;
  let empty = width - full - usize::from(half);
  let mut spans = Vec::new();
  if full > 0 {
    spans.push(Span::styled(
      "\u{2501}".repeat(full), // ━
      Style::default().fg(fill),
    ));
  }
  if half {
    spans.push(Span::styled("\u{2578}", Style::default().fg(fill))); // ╸
  }
  if empty > 0 {
    spans.push(Span::styled(
      "\u{2500}".repeat(empty), // ─
      Style::default().fg(track),
    ));
  }
  spans
}

fn spans_width(spans: &[Span<'_>]) -> usize {
  spans.iter().map(ratatui_core::text::Span::width).sum()
}

fn truncate_text(value: &str, width: usize) -> String {
  let mut result = String::new();
  let mut remaining = width;
  for character in value.chars() {
    let character_width = character.width().unwrap_or(0);
    if character_width > remaining {
      break;
    }
    result.push(character);
    remaining = remaining.saturating_sub(character_width);
  }
  result
}

fn fit_line(spans: Vec<Span<'static>>, width: u16) -> Line<'static> {
  let mut remaining = usize::from(width);
  let mut result = Vec::new();
  for span in spans {
    if remaining == 0 {
      break;
    }
    let mut text = String::new();
    for character in span.content.chars() {
      let character_width = character.width().unwrap_or(0);
      if character_width > remaining {
        break;
      }
      text.push(character);
      remaining = remaining.saturating_sub(character_width);
    }
    if !text.is_empty() {
      result.push(Span::styled(text, span.style));
    }
  }
  Line::from(result)
}

/// Renders a complete frame.
///
/// `width` is already expected to exclude the terminal's last physical column.
#[must_use]
pub fn render_frame(
  state: &State,
  config: &RenderConfig,
  now: f64,
  width: u16,
  max_height: u16,
  final_render: bool,
) -> Frame {
  let lines = Renderer::new(
    state,
    config,
    crate::icons::select(config.icons),
    now,
    width.max(1),
    max_height.max(1),
  )
  .render(final_render);
  let height = u16::try_from(lines.len())
    .unwrap_or(u16::MAX)
    .max(1)
    .min(max_height.max(1));
  let mut buffer = Buffer::empty(Rect::new(0, 0, width.max(1), height));
  for (y, line) in lines.iter().take(usize::from(height)).enumerate() {
    buffer.set_line(0, y as u16, line, width.max(1));
  }
  Frame { buffer, height }
}

#[cfg(test)]
#[expect(
  clippy::non_ascii_literal,
  reason = "expected frames contain the glyphs they check"
)]
mod tests {
  use super::*;
  use crate::state::TransferInfo;

  #[test]
  fn line_bar_uses_half_cell_glyphs() {
    let spans = progress_bar(6, 12, 3, Color::Cyan, Color::DarkGray);
    assert_eq!(spans[0].style.fg, Some(Color::Cyan));
    assert_eq!(spans[1].style.fg, Some(Color::Cyan));
    assert_eq!(spans.last().unwrap().style.fg, Some(Color::DarkGray));
    let text = spans
      .into_iter()
      .map(|span| span.content.into_owned())
      .collect::<String>();
    assert_eq!(text, "━╸─");

    let empty = progress_bar(0, 12, 3, Color::Cyan, Color::DarkGray);
    assert_eq!(empty.len(), 1);
    assert_eq!(empty[0].content, "───");
    assert_eq!(empty[0].style.fg, Some(Color::DarkGray));
  }

  #[test]
  fn formatter_is_stable() {
    assert_eq!(format_duration(65.0), "1m5s");
    assert_eq!(format_bytes(1_572_864), "1.5 MiB");
  }

  #[test]
  fn transfer_bar_uses_semantic_theme_colors() {
    let mut state = State::new();
    state.start_download(
      crate::state::StorePath::parse(
        "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-download",
      )
      .unwrap(),
      TransferInfo {
        start:             0.0,
        host:              cognos::Host::Localhost,
        activity_id:       1,
        parent:            None,
        bytes_transferred: 50,
        total_bytes:       Some(100),
      },
    );
    let config = RenderConfig {
      icons: crate::types::IconMode::Unicode,
      theme: crate::types::Theme {
        download: Color::LightRed,
        progress_track: Color::LightBlue,
        ..crate::types::Theme::default()
      },
      ..RenderConfig::default()
    };
    let frame = render_frame(&state, &config, 1.0, 79, 20, false);
    assert!(
      frame
        .buffer
        .content()
        .iter()
        .any(|cell| { cell.symbol() == "━" && cell.fg == Color::LightRed })
    );
    assert!(
      frame
        .buffer
        .content()
        .iter()
        .any(|cell| { cell.symbol() == "─" && cell.fg == Color::LightBlue })
    );
  }
}
