use ratatui_core::{
  style::Color,
  text::{Line, Span},
};

use super::{Renderer, StatusCounts, fit_line, format_duration};
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
    let mut lines = vec![fit_line(
      vec![
        self.span("┣━ ", self.config.theme.connector),
        self.span("Status       ", self.config.theme.text),
        self.span("Running    ", self.config.theme.running),
        self.span("Completed   ", self.config.theme.completed),
        self.span("Waiting   ", self.config.theme.planned),
        self.span("Failed    ", self.config.theme.failed),
        self.span("Total", self.config.theme.text),
      ],
      self.width,
    )];
    lines.push(fit_line(
      self.status_row(
        "Builds",
        self.icons.running,
        self.icons.done,
        Some(self.icons.planned),
        Some(self.icons.failed),
        self.snapshot.counts.builds,
      ),
      self.width,
    ));

    if !self.snapshot.counts.downloads.is_empty() {
      lines.push(fit_line(
        self.status_row(
          "Downloads",
          self.icons.download,
          self.icons.download,
          Some(self.icons.planned),
          None,
          self.snapshot.counts.downloads,
        ),
        self.width,
      ));
    }

    if !self.snapshot.counts.uploads.is_empty() {
      lines.push(fit_line(
        self.status_row(
          "Uploads",
          self.icons.upload,
          self.icons.upload,
          None,
          None,
          self.snapshot.counts.uploads,
        ),
        self.width,
      ));
    }
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

  fn status_row(
    &self,
    label: &str,
    running_icon: &str,
    completed_icon: &str,
    waiting_icon: Option<&str>,
    failed_icon: Option<&str>,
    counts: StatusCounts,
  ) -> Vec<Span<'static>> {
    let waiting = waiting_icon.map_or_else(
      || "          ".to_string(),
      |icon| format!("{icon} {:<7} ", counts.waiting),
    );
    let failed = failed_icon.map_or_else(
      || "          ".to_string(),
      |icon| format!("{icon} {:<7} ", counts.failed),
    );
    vec![
      self.span(format!("┃  {label:<13}"), self.config.theme.connector),
      self.span(
        format!("{running_icon} {:<8} ", counts.running),
        self.config.theme.running,
      ),
      self.span(
        format!("{completed_icon} {:<9} ", counts.completed),
        self.config.theme.completed,
      ),
      self.span(waiting, self.config.theme.planned),
      self.span(failed, self.config.theme.failed),
      self.span(counts.total().to_string(), self.config.theme.text),
    ]
  }

  fn verbose_legend(&self) -> Vec<Line<'static>> {
    let summary = self.snapshot.summary;
    let mut lines = vec![fit_line(
      vec![
        self.span("┣━ ", self.config.theme.connector),
        self.span("Build Summary", self.config.theme.text),
      ],
      self.width,
    )];
    let mut builds: Vec<_> = summary
      .running_builds
      .iter()
      .filter_map(|(id, build)| {
        Some((self.snapshot.derivation(*id)?.name.name.clone(), build))
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
    let summary = self.snapshot.summary;
    let mut spans = vec![self.span(prefix, self.config.theme.connector)];
    for (icon, count, label, color) in [
      (
        self.icons.running,
        summary.running_builds.len(),
        "building",
        self.config.theme.running,
      ),
      (
        self.icons.done,
        summary.completed_builds.len(),
        "completed",
        self.config.theme.completed,
      ),
      (
        self.icons.failed,
        summary.failed_builds.len(),
        "failed",
        self.config.theme.failed,
      ),
      (
        self.icons.planned,
        summary.planned_builds.len(),
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
    let failed = self.snapshot.counts.builds.failed;
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
    } else {
      (
        format!("Finished after {elapsed}"),
        self.config.theme.completed,
      )
    }
  }
}
