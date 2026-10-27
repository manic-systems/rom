//! Transfer progress, either inline or with names, bars, and figures sharing
//! columns across rows.

use ratatui_core::{
  style::Color,
  text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::{
  Direction,
  Renderer,
  Transfer,
  elapsed,
  fit_line,
  format_bytes,
  format_duration,
  progress_bar,
  span,
  spans_width,
  truncate_text,
};

/// Bars never shrink below this, so names are truncated first.
const MIN_BAR: usize = 8;
/// Names give way until bars are this wide.
const PREFERRED_BAR: usize = 20;
/// Bars never grow beyond this on wide terminals.
const MAX_BAR: usize = 32;
/// The longest name that still pushes the bars right.
const NAME_LIMIT: usize = 40;
/// Names are truncated no further than this before figures are dropped.
const NAME_FLOOR: usize = 12;
/// Width of `100%`.
const PERCENT: usize = 4;

/// A line that ends in transfer progress, filled in once every such line in
/// the frame is known.
pub(super) struct TransferRow {
  /// Index of the placeholder line to replace.
  pub line:     usize,
  /// Tree connectors and icons before the name.
  pub lead:     Vec<Span<'static>>,
  pub name:     String,
  pub color:    Color,
  pub transfer: Transfer,
  /// Notes after the progress figures.
  pub trailing: Vec<Span<'static>>,
}

/// Shared column widths; a zero width hides that column.
struct Columns {
  /// Column at which the arrow starts.
  name_end: usize,
  /// Width of the arrow, the percentage, and the space around them, without
  /// the bar.
  status:   usize,
  bar:      usize,
  bytes:    usize,
  duration: usize,
  host:     usize,
}

struct Figures {
  percent:  Option<u64>,
  bytes:    String,
  duration: String,
  host:     Option<String>,
}

#[expect(
  clippy::multiple_inherent_impl,
  reason = "rendering is split across modules by section"
)]
impl Renderer<'_> {
  /// Replaces each row's placeholder line with its aligned progress.
  pub(super) fn place_transfers(
    &self,
    lines: &mut [Line<'static>],
    rows: Vec<TransferRow>,
  ) {
    let figures = rows
      .iter()
      .map(|row| self.figures(&row.transfer))
      .collect::<Vec<Figures>>();
    let columns = self.columns(&rows, &figures);
    for (row, figures) in rows.into_iter().zip(figures) {
      let line = self.transfer_line(row, &figures, &columns);
      if let Some(slot) = lines.get_mut(line.0) {
        *slot = line.1;
      }
    }
  }

  fn figures(&self, transfer: &Transfer) -> Figures {
    Figures {
      percent:  transfer
        .total
        .filter(|total| *total > 0)
        .map(|total| (transfer.done.saturating_mul(100) / total).min(100)),
      bytes:    transfer.total.map_or_else(
        || format_bytes(transfer.done),
        |total| {
          format!("{}/{}", format_bytes(transfer.done), format_bytes(total))
        },
      ),
      duration: format_duration(elapsed(transfer.start, self.now)),
      host:     (transfer.host != "localhost").then(|| transfer.host.clone()),
    }
  }

  const fn arrow(&self, transfer: &Transfer) -> &'static str {
    match transfer.direction {
      Direction::Download => self.icons.download,
      Direction::Upload => self.icons.upload,
    }
  }

  /// Fits the columns to the terminal, dropping the host, then the duration,
  /// then the sizes, before truncating names below [`NAME_FLOOR`].
  fn columns(&self, rows: &[TransferRow], figures: &[Figures]) -> Columns {
    let width = usize::from(self.width);
    let leads = rows.iter().map(|row| spans_width(&row.lead));
    let wanted = rows
      .iter()
      .zip(leads.clone())
      .map(|(row, lead)| lead + row.name.width().min(NAME_LIMIT))
      .max()
      .unwrap_or(0);
    let floor = wanted.min(
      leads.max().unwrap_or(0)
        + rows
          .iter()
          .map(|row| row.name.width())
          .max()
          .unwrap_or(0)
          .min(NAME_FLOOR),
    );
    let arrow = 2
      + rows
        .iter()
        .map(|row| self.arrow(&row.transfer).width())
        .max()
        .unwrap_or(0)
      + self.icons.gap.width();
    let has_bar = figures.iter().any(|item| item.percent.is_some());
    // Without any bar, the arrow and spinner follow the names directly.
    let status = if has_bar { arrow + PERCENT } else { arrow + 1 };
    let widest = |measure: fn(&Figures) -> usize| {
      figures.iter().map(measure).max().unwrap_or(0)
    };
    let bytes = widest(|item| item.bytes.width());
    let duration = widest(|item| item.duration.width());
    let host = widest(|item| item.host.as_ref().map_or(0, |host| host.width()));

    let optional = [host, duration, bytes];
    for shown in (0..=optional.len()).rev() {
      let mut kept = [0; 3];
      kept[optional.len() - shown..]
        .copy_from_slice(&optional[optional.len() - shown..]);
      let fixed = status
        + kept
          .iter()
          .filter(|&&column| column > 0)
          .map(|column| column + 2)
          .sum::<usize>();
      let bar_room = |bar: usize| if has_bar { bar + 1 } else { 0 };
      if width >= floor + fixed + bar_room(MIN_BAR) {
        let name_end = width
          .saturating_sub(fixed + bar_room(PREFERRED_BAR))
          .clamp(floor, wanted);
        let bar = if has_bar {
          (width - fixed - name_end - 1).min(MAX_BAR)
        } else {
          0
        };
        return Columns {
          name_end,
          status,
          bar,
          bytes: kept[2],
          duration: kept[1],
          host: kept[0],
        };
      }
    }
    Columns {
      name_end: width.saturating_sub(status),
      status,
      bar: 0,
      bytes: 0,
      duration: 0,
      host: 0,
    }
  }

  fn transfer_line(
    &self,
    row: TransferRow,
    figures: &Figures,
    columns: &Columns,
  ) -> (usize, Line<'static>) {
    let TransferRow {
      line,
      mut lead,
      name,
      color,
      transfer,
      trailing,
    } = row;
    let room = columns.name_end.saturating_sub(spans_width(&lead));
    let name = truncate_text(&name, room);
    let padding = room - name.width();
    lead.push(span(name, color));
    lead.push(Span::raw(" ".repeat(padding)));

    let progress = self.transfer_color(&transfer);
    let arrow = self.arrow(&transfer);
    let bar = if columns.bar > 0 { columns.bar + 1 } else { 0 };
    if let Some(value) = figures.percent {
      let gap =
        columns.status - PERCENT - arrow.width() - self.icons.gap.width();
      lead.push(span(
        format!("{}{arrow}{}", " ".repeat(gap), self.icons.gap),
        progress,
      ));
      if columns.bar > 0 {
        lead.extend(progress_bar(
          transfer.done,
          transfer.total.unwrap_or(0),
          columns.bar,
          progress,
          self.config.theme.progress_track,
        ));
        lead.push(Span::raw(" "));
      }
      lead.push(span(format!("{value:>3}%"), progress));
    } else {
      // Without a total, the arrow sits beside the spinner in the percent
      // column.
      let label = format!("{arrow}{}{}", self.icons.gap, self.spinner());
      let padding = (columns.status + bar).saturating_sub(label.width());
      lead.push(span(format!("{}{label}", " ".repeat(padding)), progress));
    }
    if columns.bytes > 0 {
      lead.push(span(
        format!("  {:>width$}", figures.bytes, width = columns.bytes),
        self.config.theme.muted,
      ));
    }
    if columns.duration > 0 {
      lead.push(span(
        format!("  {:>width$}", figures.duration, width = columns.duration),
        self.config.theme.timer,
      ));
    }
    if columns.host > 0
      && let Some(host) = &figures.host
    {
      lead.push(span(format!("  {host}"), self.config.theme.host));
    }
    lead.extend(trailing);
    (line, fit_line(lead, self.width))
  }

  /// Renders progress right after a transfer's name, for the inline layout.
  pub(super) fn transfer_suffix(
    &self,
    transfer: &Transfer,
    occupied: usize,
  ) -> Vec<Span<'static>> {
    let color = self.transfer_color(transfer);
    let arrow = self.arrow(transfer);
    let percent = transfer
      .total
      .filter(|total| *total > 0)
      .map(|total| (transfer.done.saturating_mul(100) / total).min(100));
    let bytes = transfer.total.map_or_else(
      || format_bytes(transfer.done),
      |total| {
        format!("{}/{}", format_bytes(transfer.done), format_bytes(total))
      },
    );
    let duration = format_duration(elapsed(transfer.start, self.now));
    let essential = percent.map_or(8, |value| 7 + value.to_string().len());
    let available = usize::from(self.width).saturating_sub(occupied);
    let mut spans = vec![span(format!("  {arrow}{}", self.icons.gap), color)];
    if let Some(value) = percent {
      let bar_width = available.saturating_sub(essential).min(32);
      if bar_width >= 4 {
        spans.extend(progress_bar(
          transfer.done,
          transfer.total.unwrap_or(0),
          bar_width,
          color,
          self.config.theme.progress_track,
        ));
        spans.push(Span::raw(" "));
      }
      spans.push(span(format!("{value:>3}%"), color));
    } else {
      spans.push(span(self.spinner(), color));
    }
    if available > essential + bytes.len() + 2 {
      spans.push(span(format!("  {bytes}"), self.config.theme.muted));
    }
    if available > essential + bytes.len() + duration.len() + 5 {
      spans.push(span(format!("  {duration}"), self.config.theme.timer));
    }
    if transfer.host != "localhost"
      && available
        > essential + bytes.len() + duration.len() + transfer.host.len() + 9
    {
      spans.push(span(format!("  {}", transfer.host), self.config.theme.host));
    }
    spans
  }
}
