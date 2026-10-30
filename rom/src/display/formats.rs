//! Plain and dashboard display formats.

use ratatui_core::text::Line;

use super::{
  MIN_TIMER_SECONDS,
  Renderer,
  aggregate_transfers,
  fit_line,
  format_duration,
  format_timer,
  span,
  spans_width,
};
use crate::{
  display::Transfer,
  state::{BuildInfo, elapsed},
};

impl Renderer<'_> {
  pub(super) fn plain(&self) -> Vec<Line<'static>> {
    let counts = self.snapshot.counts.builds;
    let mut header = vec![
      span("─ ", self.config.theme.connector),
      span("Builds", self.config.theme.text),
    ];

    if self.config.show_timers {
      header.push(span(
        format!(
          "  {}{}{}",
          self.icons.clock,
          self.icons.gap,
          format_duration(elapsed(self.snapshot.start_time, self.now))
        ),
        self.config.theme.timer,
      ));
    }

    header.extend(self.machine_cpu());

    for (icon, count, label, color) in [
      (
        self.icons.running,
        counts.running,
        "building",
        self.config.theme.running,
      ),
      (
        self.icons.planned,
        counts.waiting,
        "planned",
        self.config.theme.planned,
      ),
      (
        self.icons.done,
        counts.completed,
        "completed",
        self.config.theme.completed,
      ),
      (
        self.icons.failed,
        counts.failed,
        "failed",
        self.config.theme.failed,
      ),
    ] {
      if count > 0 {
        let gap = self.icons.gap;
        header.push(span(format!("  {icon}{gap}{count} {label}"), color));
      }
    }
    let mut lines = vec![fit_line(header, self.width)];

    let mut builds = self
      .snapshot
      .builds
      .running
      .iter()
      .filter_map(|(id, build)| {
        Some((self.snapshot.derivation(*id)?.name.name.clone(), *build))
      })
      .collect::<Vec<(String, &BuildInfo)>>();
    builds.sort_by(|left, right| left.0.cmp(&right.0));

    for (name, build) in builds {
      let mut spans = vec![
        span("  ", self.config.theme.text),
        span(
          format!("{}{}{name}", self.icons.running, self.icons.gap),
          self.config.theme.running,
        ),
      ];

      if let Some(phase) = &build.phase {
        spans.push(span(format!("  ({phase})"), self.config.theme.muted));
      }

      if self.config.show_timers
        && elapsed(build.start, self.now) >= MIN_TIMER_SECONDS
      {
        spans.push(span(
          format!(
            "  {}",
            format_timer(elapsed(build.start, self.now), build.estimate)
          ),
          self.config.theme.timer,
        ));
      }

      spans.extend(build.cpu.map(|cpu| self.cpu(cpu)));

      let host = build.host.name();
      if host != "localhost" {
        spans.push(span(format!("  {host}"), self.config.theme.host));
      }
      lines.push(fit_line(spans, self.width));
    }

    let mut failed = self
      .snapshot
      .builds
      .failed
      .iter()
      .filter_map(|(id, ..)| {
        self
          .snapshot
          .derivation(*id)
          .map(|info| info.name.name.clone())
      })
      .collect::<Vec<String>>();
    failed.sort();
    for name in failed {
      lines.push(fit_line(
        vec![
          span("  ", self.config.theme.text),
          span(
            format!("{}{}{name}", self.icons.failed, self.icons.gap),
            self.config.theme.failed,
          ),
        ],
        self.width,
      ));
    }

    let mut transfers = self.snapshot.transfers().collect::<Vec<&Transfer>>();
    transfers.sort_by(|left, right| left.name.cmp(&right.name));
    for transfer in transfers {
      let mut spans = vec![
        span("  ", self.config.theme.text),
        span(transfer.name.clone(), self.transfer_color(transfer)),
      ];
      spans.extend(self.transfer_suffix(transfer, spans_width(&spans)));
      lines.push(fit_line(spans, self.width));
    }

    for fetch in &self.snapshot.source_fetches {
      let prefix = vec![span("  ", self.config.theme.text)];
      lines.push(self.source_fetch_line(prefix, fetch));
    }
    lines
  }

  pub(super) fn dashboard(&self) -> Vec<Line<'static>> {
    let summary = self.snapshot.summary;
    let counts = self.snapshot.counts.builds;
    let active = counts.running;
    let planned = counts.waiting;
    let done = counts.completed;
    let failed = counts.failed;
    let running_transfers =
      summary.running_downloads.len() + summary.running_uploads.len();
    let completed_transfers =
      summary.completed_downloads.len() + summary.completed_uploads.len();
    let has_activity = active
      + planned
      + done
      + failed
      + running_transfers
      + completed_transfers
      > 0;
    let title = self
      .snapshot
      .roots
      .first()
      .and_then(|id| self.snapshot.derivation(*id))
      .map_or("Build", |info| info.name.name.as_str());
    let host = self
      .snapshot
      .builds
      .running
      .iter()
      .map(|(_, build)| build.host.name())
      .find(|host| *host != "localhost")
      .or_else(|| {
        self
          .snapshot
          .builds
          .completed
          .iter()
          .map(|(_, build)| build.host.name())
          .find(|host| *host != "localhost")
      })
      .unwrap_or("localhost");
    let (status_icon, status, status_color) = if active > 0 {
      (self.icons.running, "building", self.config.theme.running)
    } else if failed > 0 {
      (self.icons.failed, "failed", self.config.theme.failed)
    } else if planned + running_transfers > 0 || !has_activity {
      (self.icons.planned, "waiting", self.config.theme.planned)
    } else {
      (self.icons.done, "done", self.config.theme.completed)
    };
    let mut lines = vec![fit_line(
      vec![
        span("╭─ ", self.config.theme.connector),
        span(format!("Build Dashboard: {title}"), self.config.theme.text),
      ],
      self.width,
    )];
    lines.push(fit_line(
      vec![
        span("│  ", self.config.theme.connector),
        span("Host      │ ", self.config.theme.muted),
        span(host.to_owned(), self.config.theme.host),
      ],
      self.width,
    ));
    lines.push(fit_line(
      vec![
        span("│  ", self.config.theme.connector),
        span("Status    │ ", self.config.theme.muted),
        span(
          format!("{status_icon}{}{status}", self.icons.gap),
          status_color,
        ),
      ],
      self.width,
    ));
    lines.push(fit_line(
      vec![
        span("│  ", self.config.theme.connector),
        span("Duration  │ ", self.config.theme.muted),
        span(
          format_duration(elapsed(self.snapshot.start_time, self.now)),
          self.config.theme.timer,
        ),
      ],
      self.width,
    ));
    let mut transfers = self.snapshot.transfers().peekable();
    if transfers.peek().is_some() {
      let transfer = aggregate_transfers(transfers);
      let mut spans = vec![
        span("│  ", self.config.theme.connector),
        span("Transfer  │", self.config.theme.muted),
      ];
      spans.extend(self.transfer_suffix(&transfer, spans_width(&spans)));
      lines.push(fit_line(spans, self.width));
    }
    lines.push(fit_line(
      vec![
        span("╰─ ", self.config.theme.connector),
        span("Summary   │ ", self.config.theme.muted),
        span(
          format!(
            "jobs={}  ok={done}  failed={failed}  waiting={planned}",
            active + planned + done + failed,
          ),
          self.config.theme.text,
        ),
      ],
      self.width,
    ));
    lines
  }
}
