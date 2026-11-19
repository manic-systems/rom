//! Legend, status box, and final summary rendering.

use std::collections::{BTreeMap, BTreeSet};

use cognos::Host;
use ratatui_core::{
  style::{Color, Modifier, Style},
  text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::{Renderer, fit_line, format_duration, span, spans_width};
use crate::{
  state::{BuildInfo, elapsed},
  types::{HostSort, LegendStyle, SummaryStyle},
};

#[expect(
  clippy::multiple_inherent_impl,
  reason = "rendering is split across modules by section"
)]
impl Renderer<'_> {
  pub(super) fn legend(&self) -> Vec<Line<'static>> {
    match self.config.legend_style {
      LegendStyle::Compact => self.compact_legend(),
      LegendStyle::Table => self.table_legend(),
      LegendStyle::Verbose => self.verbose_legend(),
      LegendStyle::Nom => self.nom_table(false, true),
    }
  }

  fn compact_legend(&self) -> Vec<Line<'static>> {
    self.status_box(vec![self.legend_counts(false)])
  }

  /// Hangs rows off the tree's line as the status box.
  ///
  /// The light vertical line continues from the tree: the first row branches
  /// off it with a heavy line, the last row ends it with a heavy turn, and rows
  /// in between sit beside it.
  fn status_box(&self, rows: Vec<Vec<Span<'static>>>) -> Vec<Line<'static>> {
    let count = rows.len();
    rows
      .into_iter()
      .enumerate()
      .map(|(index, row)| {
        // The light tree trunk turns into a heavy, square-cornered box.
        let corner = match index {
          _ if count == 1 => "┕━ ",
          0 => "┢━ ",
          _ if index + 1 == count => "┗━ ",
          _ => "┃  ",
        };
        let mut spans = vec![span(corner, self.config.theme.connector)];
        spans.extend(row);
        fit_line(spans, self.width)
      })
      .collect()
  }

  fn table_legend(&self) -> Vec<Line<'static>> {
    // Prefer the labelled grid; terminals too narrow for it get the same rows
    // without the header and with tighter spacing.
    let show_failed = self.snapshot.counts.builds.failed > 0;
    let rows = self.status_rows();
    let labelled = status_grid(
      &[vec![self.status_header()], rows.clone()].concat(),
      3,
      show_failed,
    );
    // Every box row starts with a three-column corner or vertical line.
    let fits = labelled
      .iter()
      .all(|line| spans_width(line) + 3 <= usize::from(self.width));
    let mut grid = if fits {
      labelled
    } else {
      status_grid(&rows, 1, show_failed)
    };
    let mut elapsed_row = vec![
      span("Elapsed ", self.config.theme.text),
      span(
        format!(
          "{}{}{}",
          self.icons.clock,
          self.icons.gap,
          format_duration(elapsed(self.snapshot.start_time, self.now))
        ),
        self.config.theme.text,
      ),
    ];
    elapsed_row.extend(self.machine_cpu());
    grid.push(elapsed_row);
    self.status_box(grid)
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
    .map(|(text, color)| (text.to_owned(), color))
  }

  fn status_rows(&self) -> Vec<StatusRow> {
    let theme = &self.config.theme;
    let icons = self.icons;
    let counts = &self.snapshot.counts;
    let row = |label: &str, cells: [Option<(&str, usize)>; 4], total| {
      let cell = |index: usize, color| {
        let text = cells[index].map_or_else(String::new, |(icon, count)| {
          format!("{icon}{}{count}", icons.gap)
        });
        (text, color)
      };
      [
        (label.to_owned(), theme.text),
        cell(0, theme.running),
        cell(1, theme.completed),
        cell(2, theme.planned),
        cell(3, theme.failed),
        (format!("{}{}{total}", icons.summary, icons.gap), theme.text),
      ]
    };

    let builds = counts.builds;
    let mut build_row = row(
      "Builds",
      [
        Some((icons.running, builds.running)),
        Some((icons.done, builds.completed)),
        Some((icons.planned, builds.waiting)),
        Some((icons.failed, builds.failed)),
      ],
      builds.total(),
    );
    if builds.checking > 0 {
      build_row[3].0 = format!(
        "{}{}{} + {} checking",
        icons.planned, icons.gap, builds.waiting, builds.checking
      );
    }
    let mut rows = vec![build_row];
    let downloads = counts.downloads;
    if self.snapshot.summary.downloads_announced || !downloads.is_empty() {
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

  fn verbose_legend(&self) -> Vec<Line<'static>> {
    let mut rows = vec![vec![span("Build Summary", self.config.theme.text)]];
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
      let host = build.host.name();
      let mut spans = vec![span(
        format!(
          "{}{}{name}  {}",
          self.icons.running,
          self.icons.gap,
          format_duration(elapsed(build.start, self.now))
        ),
        self.config.theme.running,
      )];
      if let Some(phase) = &build.phase {
        spans.push(span(format!("  ({phase})"), self.config.theme.muted));
      }
      if host != "localhost" {
        spans.push(span(format!("  {host}"), self.config.theme.host));
      }
      rows.push(spans);
    }
    rows.push(self.legend_counts(true));
    self.status_box(rows)
  }

  fn legend_counts(&self, verbose: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
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
      let label =
        if label == "waiting" && self.snapshot.counts.builds.checking > 0 {
          let checking = self.snapshot.counts.builds.checking;
          if verbose {
            format!(
              " {icon}{}{count} waiting + {checking} checking",
              self.icons.gap
            )
          } else {
            format!(" {icon}{}{count} + {checking} checking", self.icons.gap)
          }
        } else if verbose {
          format!(" {icon}{}{count} {label}", self.icons.gap)
        } else {
          format!(" {icon}{}{count}", self.icons.gap)
        };
      spans.push(span(label, color));
    }
    spans.extend(self.machine_cpu());
    spans
  }

  pub(super) fn final_summary(&self, connected: bool) -> Vec<Line<'static>> {
    let (text, color) = self.final_status();
    let failed = self.snapshot.counts.builds.failed;
    let status = vec![span(text, color)];
    let rows = match self.config.summary_style {
      SummaryStyle::Nom => return self.nom_table(true, connected),
      SummaryStyle::Concise => vec![status],
      SummaryStyle::Table => {
        vec![
          vec![span(
            format!(
              "∑ {}{gap}{}  {}{gap}{}  {}{gap}{}  {}{gap}{}",
              self.icons.done,
              self.snapshot.counts.builds.completed,
              self.icons.failed,
              failed,
              self.icons.download,
              self.snapshot.counts.downloads.completed,
              self.icons.upload,
              self.snapshot.counts.uploads.completed,
              gap = self.icons.gap,
            ),
            self.config.theme.text,
          )],
          status,
        ]
      },
      SummaryStyle::Full => {
        let mut rows =
          vec![vec![span("Build Summary", self.config.theme.text)]];
        rows.push(vec![span(
          format!(
            "Builds: {} completed, {failed} failed",
            self.snapshot.counts.builds.completed,
          ),
          self.config.theme.text,
        )]);
        let downloads = self.snapshot.counts.downloads.completed;
        let uploads = self.snapshot.counts.uploads.completed;
        if downloads + uploads > 0 {
          rows.push(vec![span(
            format!("Transfers: {downloads} downloaded, {uploads} uploaded"),
            self.config.theme.text,
          )]);
        }
        rows.push(status);
        rows
      },
    };
    if connected {
      return self.status_box(rows);
    }
    // Unboxed, rows between the first and last are indented instead.
    let count = rows.len();
    rows
      .into_iter()
      .enumerate()
      .map(|(index, row)| {
        let indent = if index == 0 || index + 1 == count {
          ""
        } else {
          "  "
        };
        let mut spans = vec![span(indent, self.config.theme.connector)];
        spans.extend(row);
        Line::from(spans)
      })
      .collect()
  }

  fn nom_table(
    &self,
    final_render: bool,
    connected: bool,
  ) -> Vec<Line<'static>> {
    let aggregated = self.aggregate_nom_hosts();
    let columns = NomColumns {
      builds:    aggregated.total_builds() > 0,
      failed:    aggregated.total_failed_builds > 0,
      downloads: self.snapshot.summary.downloads_announced
        || aggregated.total_downloads() > 0,
      uploads:   aggregated.total_uploads() > 0,
    };

    let active_hosts = aggregated
      .hosts
      .values()
      .filter(|stats| stats.is_active())
      .collect::<Vec<&NomHostStats>>();
    let show_hosts = active_hosts.len() > 1;

    let mut rows = Vec::<Vec<NomCell>>::new();

    // Header row
    let mut header = Vec::new();
    if columns.builds {
      header.push(
        NomCell::left("Builds", self.config.theme.text)
          .bold()
          .span(if columns.failed { 4 } else { 3 }),
      );
    }
    if columns.downloads {
      header.push(
        NomCell::left("Downloads", self.config.theme.text)
          .bold()
          .span(4),
      );
    }
    if columns.uploads {
      header.push(
        NomCell::left("Uploads", self.config.theme.text)
          .bold()
          .span(2),
      );
    }
    if show_hosts {
      header.push(NomCell::left("Host", self.config.theme.text).bold());
    }
    if !header.is_empty() {
      rows.push(header);
    }

    // Per-host rows
    if show_hosts {
      let display_hosts = self.prepare_nom_display_hosts(&active_hosts);
      for host in display_hosts {
        rows.push(self.build_nom_host_row(&host, columns));
      }
    }

    // Totals row
    rows.push(self.build_nom_totals_row(&aggregated, columns, final_render));

    let sep = if self.width < 80 { " │" } else { " │ " };
    let aligned = align_nom_table(&rows, sep, self.config.theme.connector);
    let count = aligned.len();
    let summary_prefix = format!("┗━ {} ", self.icons.summary);

    aligned
      .into_iter()
      .enumerate()
      .map(|(index, row)| {
        let mut spans = Vec::new();
        if connected {
          let prefix = if count == 1 {
            "┕━ "
          } else if index == 0 {
            "┢━━━ "
          } else if index + 1 == count {
            summary_prefix.as_str()
          } else {
            "┃    "
          };
          spans.push(span(prefix, self.config.theme.connector));
        } else if index + 1 == count && count > 1 {
          spans.push(span(
            format!("{} ", self.icons.summary),
            self.config.theme.connector,
          ));
        } else if index > 0 {
          spans.push(span("  ", self.config.theme.connector));
        }
        spans.extend(row);
        fit_line(spans, self.width)
      })
      .collect()
  }

  fn aggregate_nom_hosts(&self) -> NomAggregated {
    let mut aggregated = NomAggregated::default();
    let counts = &self.snapshot.counts;

    let local = aggregated
      .hosts
      .entry("localhost".to_owned())
      .or_insert_with(|| NomHostStats::new("localhost".to_owned()));
    local.waiting_builds = counts.builds.waiting;

    let mut accounted_completed_builds = 0_usize;
    let mut accounted_failed_builds = 0_usize;

    for (_, build) in &self.snapshot.builds.running {
      let host_entry =
        aggregated.host_mut(&build.host, build.protocol.as_deref());
      host_entry.running_builds += 1;
      aggregated.total_running_builds += 1;

      if let Some(curl) = &build.curl {
        let done = curl.bytes_transferred;
        let expected = curl.total_bytes.unwrap_or(0);
        let curl_entry =
          aggregated.host_mut(&curl.host, curl.protocol.as_deref());
        curl_entry.running_downloads += 1;
        curl_entry.download_done =
          curl_entry.download_done.saturating_add(done);
        curl_entry.download_expected =
          curl_entry.download_expected.saturating_add(expected);
        aggregated.total_running_downloads += 1;
        aggregated.total_done = aggregated.total_done.saturating_add(done);
        aggregated.total_expected =
          aggregated.total_expected.saturating_add(expected);
      }
    }

    for (_, build) in &self.snapshot.builds.completed {
      let host_entry =
        aggregated.host_mut(&build.host, build.protocol.as_deref());
      host_entry.completed_builds += 1;
      accounted_completed_builds += 1;

      if let Some(curl) = &build.curl {
        let done = curl.total_bytes.map_or(curl.bytes_transferred, |total| {
          total.max(curl.bytes_transferred)
        });
        let expected = curl.total_bytes.unwrap_or(done);
        let curl_entry =
          aggregated.host_mut(&curl.host, curl.protocol.as_deref());
        curl_entry.completed_downloads += 1;
        curl_entry.download_done =
          curl_entry.download_done.saturating_add(done);
        curl_entry.download_expected =
          curl_entry.download_expected.saturating_add(expected);
        aggregated.total_completed_downloads += 1;
        aggregated.total_done = aggregated.total_done.saturating_add(done);
        aggregated.total_expected =
          aggregated.total_expected.saturating_add(expected);
      }
    }

    for (_, build, _) in &self.snapshot.builds.failed {
      let host_entry =
        aggregated.host_mut(&build.host, build.protocol.as_deref());
      host_entry.failed_builds += 1;
      accounted_failed_builds += 1;
    }

    // Account for available derivations and dependency failures on localhost so
    // per-host counts match totals.
    let extra_completed = counts
      .builds
      .completed
      .saturating_sub(accounted_completed_builds);
    let extra_failed =
      counts.builds.failed.saturating_sub(accounted_failed_builds);
    if extra_completed > 0 || extra_failed > 0 {
      let local = aggregated
        .hosts
        .entry("localhost".to_owned())
        .or_insert_with(|| NomHostStats::new("localhost".to_owned()));
      local.completed_builds += extra_completed;
      local.failed_builds += extra_failed;
    }
    aggregated.total_completed_builds = counts.builds.completed;
    aggregated.total_waiting_builds = counts.builds.waiting;
    aggregated.total_failed_builds = counts.builds.failed;

    for download in self.snapshot.summary.running_downloads.values() {
      let done = download.bytes_transferred;
      let expected = download.total_bytes.unwrap_or(0);
      let entry =
        aggregated.host_mut(&download.host, download.protocol.as_deref());
      entry.running_downloads += 1;
      entry.download_done = entry.download_done.saturating_add(done);
      entry.download_expected =
        entry.download_expected.saturating_add(expected);
      aggregated.total_running_downloads += 1;
      aggregated.total_done = aggregated.total_done.saturating_add(done);
      aggregated.total_expected =
        aggregated.total_expected.saturating_add(expected);
    }

    for download in self.snapshot.summary.completed_downloads.values() {
      let bytes = download.total_bytes;
      let entry =
        aggregated.host_mut(&download.host, download.protocol.as_deref());
      entry.completed_downloads += 1;
      entry.download_done = entry.download_done.saturating_add(bytes);
      entry.download_expected = entry.download_expected.saturating_add(bytes);
      aggregated.total_completed_downloads += 1;
      aggregated.total_done = aggregated.total_done.saturating_add(bytes);
      aggregated.total_expected =
        aggregated.total_expected.saturating_add(bytes);
    }
    aggregated.total_waiting_downloads = counts.downloads.waiting;

    for upload in self.snapshot.summary.running_uploads.values() {
      let entry = aggregated.host_mut(&upload.host, upload.protocol.as_deref());
      entry.running_uploads += 1;
      aggregated.total_running_uploads += 1;
    }

    for upload in self.snapshot.summary.completed_uploads.values() {
      let entry = aggregated.host_mut(&upload.host, upload.protocol.as_deref());
      entry.completed_uploads += 1;
      aggregated.total_completed_uploads += 1;
    }

    aggregated
  }

  fn prepare_nom_display_hosts(
    &self,
    active_hosts: &[&NomHostStats],
  ) -> Vec<NomDisplayHost> {
    let mut localhost = None;
    let mut remotes = Vec::new();
    for &stats in active_hosts {
      if stats.hostname == "localhost" {
        localhost = Some(stats);
      } else {
        remotes.push(stats);
      }
    }

    sort_nom_hosts(&mut remotes, self.config.host_sort, self.config.max_hosts);

    let mut result = Vec::new();
    if let Some(local) = localhost {
      result.push(NomDisplayHost::from_stats(local));
    }

    if let Some(cap) = self.config.max_hosts
      && remotes.len() > cap
    {
      let overflow = remotes.len() - cap;
      let mut other = NomDisplayHost {
        label: "(other)".to_owned(),
        ..NomDisplayHost::default()
      };
      for stats in &remotes[..overflow] {
        other.add(stats);
      }
      result.push(other);
      for stats in &remotes[overflow..] {
        result.push(NomDisplayHost::from_stats(stats));
      }
    } else {
      for stats in remotes {
        result.push(NomDisplayHost::from_stats(stats));
      }
    }

    result
  }

  fn build_nom_host_row(
    &self,
    host: &NomDisplayHost,
    columns: NomColumns,
  ) -> Vec<NomCell> {
    let theme = &self.config.theme;
    let icons = self.icons;
    let mut row = Vec::new();

    if columns.builds {
      row.push(nom_nonzero_opt(
        icons.running,
        icons.gap,
        host.running_builds,
        theme.running,
      ));
      row.push(nom_nonzero_opt(
        icons.done,
        icons.gap,
        host.completed_builds,
        theme.completed,
      ));
      row.push(nom_nonzero_opt(
        icons.planned,
        icons.gap,
        host.waiting_builds,
        theme.planned,
      ));
      if columns.failed {
        row.push(nom_nonzero_opt(
          icons.failed,
          icons.gap,
          host.failed_builds,
          theme.failed,
        ));
      }
    }

    if columns.downloads {
      row.push(nom_nonzero_opt(
        icons.download,
        icons.gap,
        host.running_downloads,
        theme.running,
      ));
      row.push(nom_nonzero_opt(
        icons.download,
        icons.gap,
        host.completed_downloads,
        theme.completed,
      ));
      row.push(NomCell::empty());
      if host.download_expected > 0 || host.download_done > 0 {
        let expected = host.download_expected.max(host.download_done);
        row.push(NomCell::right(
          format!(
            "{}{}{}/{}",
            icons.download,
            icons.gap,
            self.bytes(host.download_done),
            self.bytes(expected),
          ),
          theme.completed,
        ));
      } else {
        row.push(NomCell::empty());
      }
    }

    if columns.uploads {
      row.push(nom_nonzero_opt(
        icons.upload,
        icons.gap,
        host.running_uploads,
        theme.running,
      ));
      row.push(nom_nonzero_opt(
        icons.upload,
        icons.gap,
        host.completed_uploads,
        theme.completed,
      ));
    }

    row.push(NomCell::left(host.label.clone(), theme.host));
    row
  }

  fn build_nom_totals_row(
    &self,
    aggregated: &NomAggregated,
    columns: NomColumns,
    final_render: bool,
  ) -> Vec<NomCell> {
    let theme = &self.config.theme;
    let icons = self.icons;
    let mut row = Vec::new();

    if columns.builds {
      row.push(nom_count_cell(
        icons.running,
        icons.gap,
        aggregated.total_running_builds,
        theme.running,
      ));
      row.push(nom_count_cell(
        icons.done,
        icons.gap,
        aggregated.total_completed_builds,
        theme.completed,
      ));
      row.push(nom_count_cell(
        icons.planned,
        icons.gap,
        aggregated.total_waiting_builds,
        theme.planned,
      ));
      if columns.failed {
        row.push(nom_count_cell(
          icons.failed,
          icons.gap,
          aggregated.total_failed_builds,
          theme.failed,
        ));
      }
    }

    if columns.downloads {
      row.push(nom_count_cell(
        icons.download,
        icons.gap,
        aggregated.total_running_downloads,
        theme.running,
      ));
      row.push(nom_count_cell(
        icons.download,
        icons.gap,
        aggregated.total_completed_downloads,
        theme.completed,
      ));
      row.push(nom_count_cell(
        icons.planned,
        icons.gap,
        aggregated.total_waiting_downloads,
        theme.planned,
      ));
      if aggregated.total_expected > 0 || aggregated.total_done > 0 {
        let expected = aggregated.total_expected.max(aggregated.total_done);
        row.push(NomCell::right(
          format!(
            "{}{}{}/{}",
            icons.download,
            icons.gap,
            self.bytes(aggregated.total_done),
            self.bytes(expected),
          ),
          theme.completed,
        ));
      } else if let Some(planned) = self.snapshot.summary.planned_download_bytes
      {
        row.push(NomCell::right(
          format!(
            "{}{}{}/{}",
            icons.download,
            icons.gap,
            self.bytes(0),
            self.bytes(planned),
          ),
          theme.completed,
        ));
      } else {
        row.push(NomCell::empty());
      }
    }

    if columns.uploads {
      row.push(nom_count_cell(
        icons.upload,
        icons.gap,
        aggregated.total_running_uploads,
        theme.running,
      ));
      row.push(nom_count_cell(
        icons.upload,
        icons.gap,
        aggregated.total_completed_uploads,
        theme.completed,
      ));
    }

    if final_render {
      let (text, color) = self.final_status();
      row.push(NomCell::left(text, color).bold());
    } else {
      let mut time = format!(
        "{}{}{}",
        icons.clock,
        icons.gap,
        format_duration(elapsed(self.snapshot.start_time, self.now))
      );
      if let Some(share) = self.snapshot.state.machine_cpu() {
        use std::fmt::Write;
        let _ =
          write!(time, "  {}{}{:.0}%", icons.cpu, icons.gap, share * 100.0);
      }
      row.push(NomCell::left(time, theme.text).bold());
    }

    row
  }

  pub(super) fn final_status(&self) -> (String, Color) {
    let failed = self.snapshot.counts.builds.failed;
    let active = self.snapshot.counts.builds.running
      + self.snapshot.counts.downloads.running
      + self.snapshot.counts.uploads.running;
    let duration = format_duration(elapsed(self.snapshot.start_time, self.now));
    if failed > 0 {
      (
        format!("Exited with {failed} failed build(s) after {duration}"),
        self.config.theme.failed,
      )
    } else if self.snapshot.error_count > 0 {
      (
        format!(
          "Exited with {} Nix error(s) after {duration}",
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
        format!("Input ended with {active} unfinished {noun} after {duration}"),
        self.config.theme.running,
      )
    } else if self.snapshot.counts.builds.waiting > 0 {
      let waiting = self.snapshot.counts.builds.waiting;
      let noun = if waiting == 1 { "build" } else { "builds" };
      (
        format!("Input ended with {waiting} planned {noun} after {duration}"),
        self.config.theme.planned,
      )
    } else {
      (
        format!("Finished after {duration}"),
        self.config.theme.completed,
      )
    }
  }
}

/// Lays out rows as aligned columns separated by `gap` spaces.
fn status_grid(
  rows: &[StatusRow],
  gap: usize,
  show_failed: bool,
) -> Vec<Vec<Span<'static>>> {
  let mut widths = std::array::from_fn::<usize, 6, _>(|column| {
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
    .map(|row| {
      let mut spans = Vec::new();
      let cells = row
        .iter()
        .enumerate()
        .filter(|&(column, _)| show_failed || column != FAILED_COLUMN);
      for (column, (text, color)) in cells {
        let padding = if column + 1 == row.len() {
          0
        } else {
          widths[column] - text.width() + gap
        };
        spans.push(span(format!("{text}{}", " ".repeat(padding)), *color));
      }
      spans
    })
    .collect()
}

const FAILED_COLUMN: usize = 4;

/// Label, running, completed, waiting, failed, and total cells.
type StatusRow = [(String, Color); 6];

#[derive(Clone, Copy)]
#[expect(
  clippy::struct_excessive_bools,
  reason = "independent column-visibility flags for the nom summary table"
)]
struct NomColumns {
  builds:    bool,
  failed:    bool,
  downloads: bool,
  uploads:   bool,
}

#[derive(Default)]
struct NomAggregated {
  hosts:                     BTreeMap<String, NomHostStats>,
  total_running_builds:      usize,
  total_completed_builds:    usize,
  total_waiting_builds:      usize,
  total_failed_builds:       usize,
  total_running_downloads:   usize,
  total_completed_downloads: usize,
  total_waiting_downloads:   usize,
  total_running_uploads:     usize,
  total_completed_uploads:   usize,
  total_done:                u64,
  total_expected:            u64,
}

impl NomAggregated {
  fn host_mut(
    &mut self,
    host: &Host,
    protocol: Option<&str>,
  ) -> &mut NomHostStats {
    let key = host.name().to_owned();
    let entry = self
      .hosts
      .entry(key.clone())
      .or_insert_with(|| NomHostStats::new(key));
    if !matches!(host, Host::Localhost)
      && let Some(protocol) = protocol
      && !protocol.is_empty()
    {
      entry.protocols.insert(protocol.to_owned());
    }
    entry
  }

  const fn total_builds(&self) -> usize {
    self.total_running_builds
      + self.total_completed_builds
      + self.total_waiting_builds
      + self.total_failed_builds
  }

  const fn total_downloads(&self) -> usize {
    self.total_running_downloads
      + self.total_completed_downloads
      + self.total_waiting_downloads
  }

  const fn total_uploads(&self) -> usize {
    self.total_running_uploads + self.total_completed_uploads
  }
}

struct NomHostStats {
  hostname:            String,
  protocols:           BTreeSet<String>,
  running_builds:      usize,
  completed_builds:    usize,
  waiting_builds:      usize,
  failed_builds:       usize,
  running_downloads:   usize,
  completed_downloads: usize,
  running_uploads:     usize,
  completed_uploads:   usize,
  download_done:       u64,
  download_expected:   u64,
}

impl NomHostStats {
  const fn new(hostname: String) -> Self {
    Self {
      hostname,
      protocols: BTreeSet::new(),
      running_builds: 0,
      completed_builds: 0,
      waiting_builds: 0,
      failed_builds: 0,
      running_downloads: 0,
      completed_downloads: 0,
      running_uploads: 0,
      completed_uploads: 0,
      download_done: 0,
      download_expected: 0,
    }
  }

  const fn is_active(&self) -> bool {
    self.running_builds > 0
      || self.completed_builds > 0
      || self.waiting_builds > 0
      || self.failed_builds > 0
      || self.running_downloads > 0
      || self.completed_downloads > 0
      || self.running_uploads > 0
      || self.completed_uploads > 0
      || self.download_done > 0
      || self.download_expected > 0
  }

  const fn total_builds(&self) -> usize {
    self.running_builds
      + self.completed_builds
      + self.waiting_builds
      + self.failed_builds
  }

  const fn total_downloads(&self) -> usize {
    self.running_downloads + self.completed_downloads
  }

  const fn download_size(&self) -> u64 {
    if self.download_expected >= self.download_done {
      self.download_expected
    } else {
      self.download_done
    }
  }

  fn display_label(&self) -> String {
    if self.hostname == "localhost" || self.protocols.is_empty() {
      self.hostname.clone()
    } else {
      let protocols = self
        .protocols
        .iter()
        .map(String::as_str)
        .collect::<Vec<&str>>()
        .join(", ");
      format!("{} ({protocols})", self.hostname)
    }
  }
}

#[derive(Default)]
struct NomDisplayHost {
  label:               String,
  running_builds:      usize,
  completed_builds:    usize,
  waiting_builds:      usize,
  failed_builds:       usize,
  running_downloads:   usize,
  completed_downloads: usize,
  running_uploads:     usize,
  completed_uploads:   usize,
  download_done:       u64,
  download_expected:   u64,
}

impl NomDisplayHost {
  fn from_stats(stats: &NomHostStats) -> Self {
    Self {
      label:               stats.display_label(),
      running_builds:      stats.running_builds,
      completed_builds:    stats.completed_builds,
      waiting_builds:      stats.waiting_builds,
      failed_builds:       stats.failed_builds,
      running_downloads:   stats.running_downloads,
      completed_downloads: stats.completed_downloads,
      running_uploads:     stats.running_uploads,
      completed_uploads:   stats.completed_uploads,
      download_done:       stats.download_done,
      download_expected:   stats.download_expected,
    }
  }

  const fn add(&mut self, stats: &NomHostStats) {
    self.running_builds += stats.running_builds;
    self.completed_builds += stats.completed_builds;
    self.waiting_builds += stats.waiting_builds;
    self.failed_builds += stats.failed_builds;
    self.running_downloads += stats.running_downloads;
    self.completed_downloads += stats.completed_downloads;
    self.running_uploads += stats.running_uploads;
    self.completed_uploads += stats.completed_uploads;
    self.download_done = self.download_done.saturating_add(stats.download_done);
    self.download_expected = self
      .download_expected
      .saturating_add(stats.download_expected);
  }
}

fn sort_nom_hosts(
  hosts: &mut [&NomHostStats],
  sort: HostSort,
  max_hosts: Option<usize>,
) {
  match sort {
    HostSort::DownloadSize => {
      hosts.sort_by(|left, right| {
        left
          .download_size()
          .cmp(&right.download_size())
          .then_with(|| left.total_builds().cmp(&right.total_builds()))
          .then_with(|| left.total_downloads().cmp(&right.total_downloads()))
          .then_with(|| left.hostname.cmp(&right.hostname))
      });
    },
    HostSort::DownloadCount => {
      hosts.sort_by(|left, right| {
        left
          .total_downloads()
          .cmp(&right.total_downloads())
          .then_with(|| left.download_size().cmp(&right.download_size()))
          .then_with(|| left.total_builds().cmp(&right.total_builds()))
          .then_with(|| left.hostname.cmp(&right.hostname))
      });
    },
    HostSort::Builds => {
      hosts.sort_by(|left, right| {
        left
          .total_builds()
          .cmp(&right.total_builds())
          .then_with(|| left.download_size().cmp(&right.download_size()))
          .then_with(|| left.total_downloads().cmp(&right.total_downloads()))
          .then_with(|| left.hostname.cmp(&right.hostname))
      });
    },
    HostSort::None => {
      if max_hosts.is_some() {
        hosts.sort_by(|left, right| {
          left
            .download_size()
            .cmp(&right.download_size())
            .then_with(|| left.total_builds().cmp(&right.total_builds()))
            .then_with(|| left.total_downloads().cmp(&right.total_downloads()))
            .then_with(|| left.hostname.cmp(&right.hostname))
        });
      } else {
        hosts.sort_by(|left, right| {
          left
            .hostname
            .split('.')
            .rev()
            .cmp(right.hostname.split('.').rev())
        });
      }
    },
  }
}

struct NomCell {
  left:  String,
  right: String,
  color: Color,
  bold:  bool,
  span:  usize,
}

impl NomCell {
  const fn empty() -> Self {
    Self {
      left:  String::new(),
      right: String::new(),
      color: Color::Reset,
      bold:  false,
      span:  1,
    }
  }

  fn left(text: impl Into<String>, color: Color) -> Self {
    Self {
      left: text.into(),
      right: String::new(),
      color,
      bold: false,
      span: 1,
    }
  }

  fn right(text: impl Into<String>, color: Color) -> Self {
    Self {
      left: String::new(),
      right: text.into(),
      color,
      bold: false,
      span: 1,
    }
  }

  fn labeled(
    left: impl Into<String>,
    right: impl Into<String>,
    color: Color,
  ) -> Self {
    Self {
      left: left.into(),
      right: right.into(),
      color,
      bold: false,
      span: 1,
    }
  }

  const fn bold(mut self) -> Self {
    self.bold = true;
    self
  }

  const fn span(mut self, columns: usize) -> Self {
    self.span = columns;
    self
  }

  fn content_width(&self) -> usize {
    self.left.width() + self.right.width()
  }
}

fn nom_count_cell(
  icon: &str,
  gap: &str,
  count: usize,
  color: Color,
) -> NomCell {
  let cell = NomCell::labeled(format!("{icon}{gap}"), count.to_string(), color);
  if count > 0 { cell.bold() } else { cell }
}

fn nom_nonzero_opt(
  icon: &str,
  gap: &str,
  count: usize,
  color: Color,
) -> NomCell {
  if count > 0 {
    nom_count_cell(icon, gap, count, color)
  } else {
    NomCell::empty()
  }
}

fn align_nom_table(
  rows: &[Vec<NomCell>],
  sep: &str,
  sep_color: Color,
) -> Vec<Vec<Span<'static>>> {
  if rows.is_empty() {
    return Vec::new();
  }
  let num_cols = rows
    .iter()
    .map(|row| row.iter().map(|cell| cell.span).sum())
    .max()
    .unwrap_or(0);
  let mut col_widths = vec![0_usize; num_cols];

  for row in rows {
    let mut col = 0;
    for cell in row {
      if cell.span == 1 && col < num_cols {
        col_widths[col] = col_widths[col].max(cell.content_width());
      }
      col += cell.span;
    }
  }

  let sep_width = sep.width();
  for row in rows {
    let mut col = 0;
    for cell in row {
      if cell.span > 1 {
        let current_span = (0..cell.span)
          .map(|offset| col_widths.get(col + offset).copied().unwrap_or(0))
          .sum::<usize>()
          + sep_width * cell.span.saturating_sub(1);
        let needed = cell.content_width();
        if needed > current_span {
          let diff = needed - current_span;
          let extra = diff / cell.span;
          let remainder = diff % cell.span;
          for offset in 0..cell.span {
            if let Some(width) = col_widths.get_mut(col + offset) {
              *width += extra + usize::from(offset < remainder);
            }
          }
        }
      }
      col += cell.span;
    }
  }

  rows
    .iter()
    .map(|row| {
      let mut spans = Vec::new();
      let mut col = 0;
      for (index, cell) in row.iter().enumerate() {
        if index > 0 {
          spans.push(span(sep, sep_color));
        }
        let span_width = (0..cell.span)
          .map(|offset| col_widths.get(col + offset).copied().unwrap_or(0))
          .sum::<usize>()
          + sep_width * cell.span.saturating_sub(1);
        let padding = span_width.saturating_sub(cell.content_width());
        let text =
          format!("{}{}{}", cell.left, " ".repeat(padding), cell.right);
        let mut style = Style::default().fg(cell.color);
        if cell.bold {
          style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(text, style));
        col += cell.span;
      }
      spans
    })
    .collect()
}
