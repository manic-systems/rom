use ratatui_core::text::Line;

use super::{
  Renderer,
  aggregate_transfers,
  fit_line,
  format_duration,
  spans_width,
};

impl Renderer<'_> {
  pub(super) fn plain(&self) -> Vec<Line<'static>> {
    let summary = self.snapshot.summary;
    let mut header = vec![
      self.span("━ ", self.config.theme.connector),
      self.span("Builds", self.config.theme.text),
    ];
    if self.config.show_timers {
      header.push(self.span(
        format!(
          "  {} {}",
          self.icons.clock,
          format_duration(self.now - self.snapshot.start_time)
        ),
        self.config.theme.muted,
      ));
    }
    for (icon, count, label, color) in [
      (
        self.icons.running,
        summary.running_builds.len(),
        "building",
        self.config.theme.running,
      ),
      (
        self.icons.planned,
        summary.planned_builds.len(),
        "planned",
        self.config.theme.planned,
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
    ] {
      if count > 0 {
        header.push(self.span(format!("  {icon} {count} {label}"), color));
      }
    }
    let mut lines = vec![fit_line(header, self.width)];

    let mut builds: Vec<_> = summary
      .running_builds
      .iter()
      .filter_map(|(id, build)| {
        Some((self.snapshot.derivation(*id)?.name.name.clone(), build))
      })
      .collect();
    builds.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, build) in builds {
      let mut spans = vec![
        self.span("  ", self.config.theme.text),
        self.span(
          format!("{} {name}", self.icons.running),
          self.config.theme.running,
        ),
      ];
      if self.config.show_timers {
        spans.push(self.span(
          format!("  {}", format_duration(self.now - build.start)),
          self.config.theme.muted,
        ));
      }
      if let Some(estimate) = build.estimate {
        let elapsed = (self.now - build.start).max(0.0) as u64;
        spans.push(self.span(
          format!(
            "  {} {}",
            self.icons.estimate,
            format_duration(estimate.saturating_sub(elapsed) as f64)
          ),
          self.config.theme.muted,
        ));
      }
      let host = build.host.name();
      if host != "localhost" {
        spans.push(self.span(format!("  {host}"), self.config.theme.host));
      }
      lines.push(fit_line(spans, self.width));
    }

    let mut failed: Vec<_> = summary
      .failed_builds
      .keys()
      .filter_map(|id| {
        self
          .snapshot
          .derivation(*id)
          .map(|info| info.name.name.clone())
      })
      .collect();
    failed.sort();
    for name in failed {
      lines.push(fit_line(
        vec![
          self.span("  ", self.config.theme.text),
          self.span(
            format!("{} {name}", self.icons.failed),
            self.config.theme.failed,
          ),
        ],
        self.width,
      ));
    }

    let mut transfers: Vec<_> = self
      .snapshot
      .transfers_by_drv
      .values()
      .flatten()
      .chain(&self.snapshot.unmatched_transfers)
      .cloned()
      .collect();
    transfers.sort_by(|left, right| left.name.cmp(&right.name));
    for transfer in transfers {
      let mut spans = vec![
        self.span("  ", self.config.theme.text),
        self.span(transfer.name.clone(), self.transfer_color(&transfer)),
      ];
      spans.extend(self.transfer_suffix(&transfer, spans_width(&spans)));
      lines.push(fit_line(spans, self.width));
    }
    lines
  }

  pub(super) fn dashboard(&self) -> Vec<Line<'static>> {
    let summary = self.snapshot.summary;
    let active = summary.running_builds.len();
    let planned = summary.planned_builds.len();
    let done = summary.completed_builds.len();
    let failed = summary.failed_builds.len();
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
    let host = summary
      .running_builds
      .values()
      .map(|build| build.host.name())
      .find(|host| *host != "localhost")
      .or_else(|| {
        summary
          .completed_builds
          .values()
          .map(|build| build.host.name())
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
        self.span("┏━ ", self.config.theme.connector),
        self.span(format!("Build Dashboard: {title}"), self.config.theme.text),
      ],
      self.width,
    )];
    lines.push(fit_line(
      vec![
        self.span("┃  ", self.config.theme.connector),
        self.span("Host      │ ", self.config.theme.muted),
        self.span(host.to_string(), self.config.theme.host),
      ],
      self.width,
    ));
    lines.push(fit_line(
      vec![
        self.span("┃  ", self.config.theme.connector),
        self.span("Status    │ ", self.config.theme.muted),
        self.span(format!("{status_icon} {status}"), status_color),
      ],
      self.width,
    ));
    lines.push(fit_line(
      vec![
        self.span("┃  ", self.config.theme.connector),
        self.span("Duration  │ ", self.config.theme.muted),
        self.span(
          format_duration(self.now - self.snapshot.start_time),
          self.config.theme.muted,
        ),
      ],
      self.width,
    ));
    let transfer_items: Vec<_> = self
      .snapshot
      .transfers_by_drv
      .values()
      .flatten()
      .chain(&self.snapshot.unmatched_transfers)
      .cloned()
      .collect();
    if !transfer_items.is_empty() {
      let transfer = aggregate_transfers(&transfer_items);
      let mut spans = vec![
        self.span("┃  ", self.config.theme.connector),
        self.span("Transfer  │", self.config.theme.muted),
      ];
      spans.extend(self.transfer_suffix(&transfer, spans_width(&spans)));
      lines.push(fit_line(spans, self.width));
    }
    lines.push(fit_line(
      vec![
        self.span("┗━ ", self.config.theme.connector),
        self.span("Summary   │ ", self.config.theme.muted),
        self.span(
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
