use ratatui_core::{
  style::Color,
  text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::{Renderer, fit_line, format_duration, spans_width};
use crate::types::{LegendStyle, SummaryStyle};

impl Renderer<'_> {
  pub(super) fn legend(&self) -> Vec<Line<'static>> {
    match self.config.legend_style {
      LegendStyle::Compact => self.compact_legend(),
      LegendStyle::Table => self.table_legend(),
      LegendStyle::Verbose => self.verbose_legend(),
    }
  }

  fn compact_legend(&self) -> Vec<Line<'static>> {
    vec![fit_line(self.legend_counts("┗━ ", false), self.width)]
  }

  fn table_legend(&self) -> Vec<Line<'static>> {
    // Prefer the labelled grid; terminals too narrow for it get the same rows
    // without the header and with tighter spacing.
    let rows = self.status_rows();
    let labelled =
      self.status_grid(&[vec![self.status_header()], rows.clone()].concat(), 3);
    let fits = labelled
      .iter()
      .all(|line| spans_width(line) <= usize::from(self.width));
    let grid = if fits {
      labelled
    } else {
      self.status_grid(&rows, 1)
    };
    let mut lines: Vec<_> = grid
      .into_iter()
      .map(|spans| fit_line(spans, self.width))
      .collect();
    lines.push(fit_line(
      vec![
        self.span("┗━ ", self.config.theme.connector),
        self.span("Elapsed ", self.config.theme.muted),
        self.span(
          format!(
            "{} {}",
            self.icons.clock,
            format_duration(self.now - self.snapshot.start_time)
          ),
          self.config.theme.muted,
        ),
      ],
      self.width,
    ));
    lines
  }

  fn status_header(&self) -> StatusRow {
    let theme = &self.config.theme;
    [
      ("Status", theme.text),
      ("Running", theme.running),
      ("Completed", theme.completed),
      ("Waiting", theme.planned),
      ("Failed", theme.failed),
      ("Total", theme.text),
    ]
    .map(|(text, color)| (text.to_string(), color))
  }

  fn status_rows(&self) -> Vec<StatusRow> {
    let theme = &self.config.theme;
    let icons = self.icons;
    let counts = &self.snapshot.counts;
    let row = |label: &str, cells: [Option<(&str, usize)>; 4], total| {
      let cell = |index: usize, color| {
        let text = cells[index]
          .map_or_else(String::new, |(icon, count)| format!("{icon} {count}"));
        (text, color)
      };
      [
        (label.to_string(), theme.connector),
        cell(0, theme.running),
        cell(1, theme.completed),
        cell(2, theme.planned),
        cell(3, theme.failed),
        (format!("{} {total}", icons.summary), theme.text),
      ]
    };

    let builds = counts.builds;
    let mut rows = vec![row(
      "Builds",
      [
        Some((icons.running, builds.running)),
        Some((icons.done, builds.completed)),
        Some((icons.planned, builds.waiting)),
        Some((icons.failed, builds.failed)),
      ],
      builds.total(),
    )];
    let downloads = counts.downloads;
    if !downloads.is_empty() {
      rows.push(row(
        "Downloads",
        [
          Some((icons.download, downloads.running)),
          Some((icons.download, downloads.completed)),
          Some((icons.planned, downloads.waiting)),
          None,
        ],
        downloads.total(),
      ));
    }
    let uploads = counts.uploads;
    if !uploads.is_empty() {
      rows.push(row(
        "Uploads",
        [
          Some((icons.upload, uploads.running)),
          Some((icons.upload, uploads.completed)),
          None,
          None,
        ],
        uploads.total(),
      ));
    }
    rows
  }

  /// Lays out rows as aligned columns separated by `gap` spaces.
  fn status_grid(
    &self,
    rows: &[StatusRow],
    gap: usize,
  ) -> Vec<Vec<Span<'static>>> {
    let mut widths: [usize; 6] = std::array::from_fn(|column| {
      rows
        .iter()
        .map(|row| row[column].0.width())
        .max()
        .unwrap_or(0)
    });
    // Size labels for the widest row kind so columns stay put while transfer
    // rows come and go.
    widths[0] = widths[0].max("Downloads".len());
    rows
      .iter()
      .enumerate()
      .map(|(index, row)| {
        let prefix = if index == 0 { "┣━ " } else { "┃  " };
        let mut spans = vec![self.span(prefix, self.config.theme.connector)];
        for (column, (text, color)) in row.iter().enumerate() {
          let padding = if column + 1 == row.len() {
            0
          } else {
            widths[column] - text.width() + gap
          };
          spans
            .push(self.span(format!("{text}{}", " ".repeat(padding)), *color));
        }
        spans
      })
      .collect()
  }

  fn verbose_legend(&self) -> Vec<Line<'static>> {
    let mut lines = vec![fit_line(
      vec![
        self.span("┣━ ", self.config.theme.connector),
        self.span("Build Summary", self.config.theme.text),
      ],
      self.width,
    )];
    let mut builds: Vec<_> = self
      .snapshot
      .builds
      .running
      .iter()
      .filter_map(|(id, build)| {
        Some((self.snapshot.derivation(*id)?.name.name.clone(), *build))
      })
      .collect();
    builds.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, build) in builds {
      let host = build.host.name();
      let mut spans = vec![
        self.span("┃  ", self.config.theme.connector),
        self.span(
          format!(
            "{} {name}  {}",
            self.icons.running,
            format_duration(self.now - build.start)
          ),
          self.config.theme.running,
        ),
      ];
      if let Some(phase) = &build.phase {
        spans.push(self.span(format!("  ({phase})"), self.config.theme.muted));
      }
      if host != "localhost" {
        spans.push(self.span(format!("  {host}"), self.config.theme.host));
      }
      lines.push(fit_line(spans, self.width));
    }
    lines.push(fit_line(self.legend_counts("┗━ ", true), self.width));
    lines
  }

  fn legend_counts(
    &self,
    prefix: &'static str,
    verbose: bool,
  ) -> Vec<Span<'static>> {
    let mut spans = vec![self.span(prefix, self.config.theme.connector)];
    for (icon, count, label, color) in [
      (
        self.icons.running,
        self.snapshot.counts.builds.running,
        "building",
        self.config.theme.running,
      ),
      (
        self.icons.done,
        self.snapshot.counts.builds.completed,
        "completed",
        self.config.theme.completed,
      ),
      (
        self.icons.failed,
        self.snapshot.counts.builds.failed,
        "failed",
        self.config.theme.failed,
      ),
      (
        self.icons.planned,
        self.snapshot.counts.builds.waiting,
        "waiting",
        self.config.theme.planned,
      ),
    ] {
      let label = if verbose {
        format!(" {icon} {count} {label}")
      } else {
        format!(" {icon} {count}")
      };
      spans.push(self.span(label, color));
    }
    spans
  }

  pub(super) fn final_summary(&self, connected: bool) -> Vec<Line<'static>> {
    let (text, color) = self.final_status();
    let failed = self.snapshot.counts.builds.failed;
    match self.config.summary_style {
      SummaryStyle::Concise => {
        vec![Line::from(vec![
          self.span(
            if connected { "┗━ " } else { "" },
            self.config.theme.connector,
          ),
          self.span(text, color),
        ])]
      },
      SummaryStyle::Table => {
        let (summary_prefix, status_prefix) = if connected {
          ("┣━ ∑ ", "┗━ ")
        } else {
          ("∑ ", "")
        };
        vec![
          Line::from(vec![
            self.span(summary_prefix, self.config.theme.connector),
            self.span(
              format!(
                "{} {}  {} {}  {} {}  {} {}",
                self.icons.done,
                self.snapshot.counts.builds.completed,
                self.icons.failed,
                failed,
                self.icons.download,
                self.snapshot.counts.downloads.completed,
                self.icons.upload,
                self.snapshot.counts.uploads.completed,
              ),
              self.config.theme.text,
            ),
          ]),
          Line::from(vec![
            self.span(status_prefix, self.config.theme.connector),
            self.span(text, color),
          ]),
        ]
      },
      SummaryStyle::Full => {
        let mut lines = vec![Line::from(vec![
          self.span(
            if connected { "┣━ " } else { "" },
            self.config.theme.connector,
          ),
          self.span("Build Summary", self.config.theme.text),
        ])];
        lines.push(Line::from(vec![
          self.span(
            if connected { "┃  " } else { "  " },
            self.config.theme.connector,
          ),
          self.span(
            format!(
              "Builds: {} completed, {failed} failed",
              self.snapshot.counts.builds.completed,
            ),
            self.config.theme.text,
          ),
        ]));
        let downloads = self.snapshot.counts.downloads.completed;
        let uploads = self.snapshot.counts.uploads.completed;
        if downloads + uploads > 0 {
          lines.push(Line::from(vec![
            self.span(
              if connected { "┃  " } else { "  " },
              self.config.theme.connector,
            ),
            self.span(
              format!("Transfers: {downloads} downloaded, {uploads} uploaded"),
              self.config.theme.text,
            ),
          ]));
        }
        lines.push(Line::from(vec![
          self.span(
            if connected { "┗━ " } else { "" },
            self.config.theme.connector,
          ),
          self.span(text, color),
        ]));
        lines
      },
    }
  }

  pub(super) fn final_status(&self) -> (String, Color) {
    let failed = self.snapshot.builds.failed.len();
    let active = self.snapshot.counts.builds.running
      + self.snapshot.counts.downloads.running
      + self.snapshot.counts.uploads.running;
    let elapsed = format_duration(self.now - self.snapshot.start_time);
    if failed > 0 {
      (
        format!("Exited with {failed} failed build(s) after {elapsed}"),
        self.config.theme.failed,
      )
    } else if self.snapshot.error_count > 0 {
      (
        format!(
          "Exited with {} Nix error(s) after {elapsed}",
          self.snapshot.error_count
        ),
        self.config.theme.failed,
      )
    } else if active > 0 {
      let noun = if active == 1 {
        "activity"
      } else {
        "activities"
      };
      (
        format!("Input ended with {active} unfinished {noun} after {elapsed}"),
        self.config.theme.running,
      )
    } else if self.snapshot.counts.builds.waiting > 0 {
      let waiting = self.snapshot.counts.builds.waiting;
      let noun = if waiting == 1 { "build" } else { "builds" };
      (
        format!("Input ended with {waiting} planned {noun} after {elapsed}"),
        self.config.theme.planned,
      )
    } else {
      (
        format!("Finished after {elapsed}"),
        self.config.theme.completed,
      )
    }
  }
}

/// Label, running, completed, waiting, failed, and total cells.
type StatusRow = [(String, Color); 6];
