//! Display rendering for ROM
use std::{
  collections::HashSet,
  io::{self, Write},
};

use crossterm::{
  cursor,
  execute,
  style::{Color, ResetColor, SetForegroundColor},
  terminal,
};
use unicode_width::UnicodeWidthChar;

use crate::{
  icons::Icons,
  state::{BuildStatus, DerivationId, State, current_time},
  types::{LegendStyle, SummaryStyle},
};

/// Format a duration in seconds to a human-readable string
#[must_use]
pub fn format_duration(secs: f64) -> String {
  if secs < 60.0 {
    format!("{secs:.0}s")
  } else if secs < 3600.0 {
    format!("{:.0}m{:.0}s", secs / 60.0, secs % 60.0)
  } else {
    format!("{:.0}h{:.0}m", secs / 3600.0, (secs % 3600.0) / 60.0)
  }
}

pub struct DisplayConfig {
  pub show_timers:       bool,
  pub max_tree_depth:    usize,
  pub max_visible_lines: usize,
  pub use_color:         bool,
  pub format:            crate::types::DisplayFormat,
  pub legend_style:      LegendStyle,
  pub summary_style:     SummaryStyle,
  pub icons:             &'static Icons,
}

impl Default for DisplayConfig {
  fn default() -> Self {
    Self {
      show_timers:       true,
      max_tree_depth:    10,
      max_visible_lines: 100,
      use_color:         true,
      format:            crate::types::DisplayFormat::Tree,
      legend_style:      LegendStyle::Table,
      summary_style:     SummaryStyle::Concise,
      icons:             crate::icons::detect(),
    }
  }
}

pub struct Display<W: Write> {
  writer:            W,
  config:            DisplayConfig,
  /// Number of terminal screen rows printed in the last render.
  last_rows:         usize,
  /// Total log lines already printed (they scroll naturally, never cleared)
  printed_log_lines: usize,
}

struct TreeNode {
  drv_id:   DerivationId,
  children: Vec<Self>,
}

impl<W: Write> Display<W> {
  pub const fn new(writer: W, config: DisplayConfig) -> io::Result<Self> {
    Ok(Self {
      writer,
      config,
      last_rows: 0,
      printed_log_lines: 0,
    })
  }

  /// Get a mutable reference to the underlying writer for passthrough output.
  /// This allows external code to write directly through the display's buffer.
  pub fn writer(&mut self) -> &mut W {
    &mut self.writer
  }

  pub fn clear_previous(&mut self) -> io::Result<()> {
    if self.last_rows > 0 {
      // Move up in a single escape sequence, then clear to end of screen.
      // This is much cheaper than calling MoveUp(1) in a loop because it
      // produces one write + one flush instead of N.
      let rows = self.last_rows.min(u16::MAX as usize) as u16;
      execute!(
        self.writer,
        cursor::MoveToColumn(0),
        cursor::MoveUp(rows),
        cursor::MoveToColumn(0),
        crossterm::terminal::Clear(
          crossterm::terminal::ClearType::FromCursorDown
        )
      )?;
      self.last_rows = 0;
    }
    Ok(())
  }

  pub fn render(&mut self, state: &State, logs: &[String]) -> io::Result<()> {
    // Print any log lines that arrived since last render. These are printed
    // once and scroll up naturally, we never clear them.
    let new_logs = &logs[self.printed_log_lines.min(logs.len())..];
    if !new_logs.is_empty() {
      // Clear the current graph first so new logs appear above it
      self.clear_previous()?;
      let mut log_out = String::with_capacity(new_logs.len() * 80);
      for line in new_logs {
        log_out.push_str(line);
        log_out.push('\n');
      }
      self.writer.write_all(log_out.as_bytes())?;
      self.printed_log_lines = logs.len();
    }

    // Clear only the graph from the previous render
    self.clear_previous()?;

    // Build graph lines
    let mut graph_lines = match self.config.format {
      crate::types::DisplayFormat::Tree => {
        let tree_lines = self.render_tree_view(state);
        let has_tree = !tree_lines.is_empty();
        let mut g = tree_lines;
        g.extend(self.render_legend(state, has_tree));
        g
      },
      crate::types::DisplayFormat::Plain => self.render_plain_view(state),
      crate::types::DisplayFormat::Dashboard => {
        self.render_dashboard_view(state)
      },
    };

    if graph_lines.len() > self.config.max_visible_lines {
      graph_lines.truncate(self.config.max_visible_lines);
    }

    self.last_rows = Self::rendered_rows(&graph_lines);

    let mut out = String::with_capacity(graph_lines.len() * 80);
    for line in &graph_lines {
      out.push_str(line);
      out.push('\n');
    }
    self.writer.write_all(out.as_bytes())?;
    self.writer.flush()
  }

  pub fn render_final(&mut self, state: &State) -> io::Result<()> {
    tracing::debug!("render_final called");

    // Clear any previous render
    self.clear_previous()?;

    let mut lines = Vec::new();

    // Render final output based on format
    match self.config.format {
      crate::types::DisplayFormat::Tree => {
        // render_tree_view already includes its own header line; only extend if
        // there are actually active (building/failed) derivations to show
        let tree_lines = self.render_tree_view(state);
        lines.extend(tree_lines);
        lines.extend(self.render_final_summary(state));
      },
      crate::types::DisplayFormat::Plain => {
        lines.extend(self.render_plain_view(state));
        lines.extend(self.render_final_summary(state));
      },
      crate::types::DisplayFormat::Dashboard => {
        lines.extend(self.render_dashboard_final(state));
      },
    }

    tracing::debug!("render_final: {} lines to print", lines.len());

    // Print final output (don't track last_rows since this is final)
    for line in lines {
      writeln!(self.writer, "{line}")?;
    }

    writeln!(self.writer)?;
    self.writer.flush()?;

    Ok(())
  }

  fn rendered_rows(lines: &[String]) -> usize {
    let width = terminal::size()
      .ok()
      .map(|(cols, _)| cols as usize)
      .filter(|&cols| cols > 0);

    Self::rendered_rows_for_width(lines, width)
  }

  fn rendered_rows_for_width(lines: &[String], width: Option<usize>) -> usize {
    let Some(width) = width else {
      return lines.len();
    };

    lines
      .iter()
      .map(|line| Self::screen_rows_for_line(line, width))
      .sum()
  }

  fn screen_rows_for_line(line: &str, width: usize) -> usize {
    let visible_width = visible_width(line);
    visible_width.div_ceil(width).max(1)
  }

  fn render_final_summary(&self, state: &State) -> Vec<String> {
    match self.config.summary_style {
      SummaryStyle::Concise => self.render_finished_line(state),
      SummaryStyle::Table => self.render_table_summary(state),
      SummaryStyle::Full => self.render_full_summary(state),
    }
  }

  /// Renders the final single-line summary
  fn render_finished_line(&self, state: &State) -> Vec<String> {
    let failed = state.full_summary.failed_builds.len();
    let completed = state.full_summary.completed_builds.len();
    let nix_errors = state.nix_errors.len();
    let duration = current_time() - state.start_time;
    let now = chrono::Local::now();
    let at = now.format("%H:%M:%S");
    let dur = self.format_duration(duration);

    let ic = self.ic();
    let line = if failed > 0 {
      let noun = if failed == 1 { "failure" } else { "failures" };
      format!(
        "{} {} at {} after {}",
        self.colored(ic.failed, Color::DarkRed),
        self.colored(
          &format!("Exited after {failed} build {noun}"),
          Color::DarkRed
        ),
        self.colored(&at.to_string(), Color::DarkRed),
        self.colored(&dur, Color::DarkRed),
      )
    } else if nix_errors > 0 {
      let noun = if nix_errors == 1 { "error" } else { "errors" };
      format!(
        "{} {} at {} after {}",
        self.colored(ic.failed, Color::DarkRed),
        self.colored(
          &format!("Exited with {nix_errors} nix {noun}"),
          Color::DarkRed
        ),
        self.colored(&at.to_string(), Color::DarkRed),
        self.colored(&dur, Color::DarkRed),
      )
    } else {
      let mut s = format!(
        "{} after {}",
        self.colored(&format!("Finished at {at}"), Color::DarkGreen),
        self.colored(&dur, Color::DarkGreen),
      );
      if completed > 0 {
        s.push_str(&format!(
          "  {} {completed}",
          self.colored(ic.done, Color::DarkGreen)
        ));
      }
      s
    };

    vec![line]
  }

  fn render_table_summary(&self, state: &State) -> Vec<String> {
    let completed = state.full_summary.completed_builds.len();
    let failed = state.full_summary.failed_builds.len();
    let dl_done = state.full_summary.completed_downloads.len();
    let ul_done = state.full_summary.completed_uploads.len();
    let duration = current_time() - state.start_time;
    let now = chrono::Local::now();
    let at = now.format("%H:%M:%S");
    let dur = self.format_duration(duration);

    if completed + failed + dl_done + ul_done == 0 {
      return self.render_finished_line(state);
    }

    // Collect host breakdown
    let mut host_map: std::collections::HashMap<String, (usize, usize)> =
      std::collections::HashMap::new();
    for b in state.full_summary.completed_builds.values() {
      host_map.entry(b.host.name().to_string()).or_default().0 += 1;
    }
    for b in state.full_summary.failed_builds.values() {
      host_map.entry(b.host.name().to_string()).or_default().1 += 1;
    }
    let many_hosts = host_map.len() > 1;

    let mut lines = Vec::new();

    // Header
    let mut hdr_parts = Vec::new();
    if completed + failed > 0 {
      hdr_parts.push("Builds");
    }
    if dl_done > 0 {
      hdr_parts.push("Downloads");
    }
    if ul_done > 0 {
      hdr_parts.push("Uploads");
    }
    let ic = self.ic();
    lines.push(format!(
      "{} {}",
      self.colored("┏━━━", Color::DarkBlue),
      hdr_parts.join("  ")
    ));

    // Per-host rows when multiple hosts
    if many_hosts {
      let mut hosts: Vec<_> = host_map.keys().cloned().collect();
      hosts.sort();
      for host in &hosts {
        let (done, fail) = host_map[host];
        let mut parts = Vec::new();
        if done > 0 {
          parts.push(format!(
            "{} {done}",
            self.colored(ic.done, Color::DarkGreen)
          ));
        }
        if fail > 0 {
          parts.push(format!(
            "{} {fail}",
            self.colored(ic.failed, Color::DarkRed)
          ));
        }
        lines.push(format!(
          "{}  {}  {}",
          self.colored("┃", Color::DarkBlue),
          parts.join("  "),
          self.colored(host, Color::DarkMagenta),
        ));
      }
    }

    // Final ∑ line
    let mut sum_parts = Vec::new();
    if completed > 0 {
      sum_parts.push(format!(
        "{} {completed}",
        self.colored(ic.done, Color::DarkGreen)
      ));
    }
    if failed > 0 {
      sum_parts.push(format!(
        "{} {failed}",
        self.colored(ic.failed, Color::DarkRed)
      ));
    }
    if dl_done > 0 {
      sum_parts.push(format!(
        "{} {dl_done}",
        self.colored(ic.download, Color::DarkGreen)
      ));
    }
    if ul_done > 0 {
      sum_parts.push(format!(
        "{} {ul_done}",
        self.colored(ic.upload, Color::DarkGreen)
      ));
    }

    let finish = if failed > 0 || !state.nix_errors.is_empty() {
      self.colored(&format!("Exited at {at} after {dur}"), Color::DarkRed)
    } else {
      self.colored(&format!("Finished at {at} after {dur}"), Color::DarkGreen)
    };
    sum_parts.push(finish);

    lines.push(format!(
      "{} ∑ {}",
      self.colored("┗━", Color::DarkBlue),
      sum_parts.join("  │  ")
    ));

    lines
  }

  fn render_full_summary(&self, state: &State) -> Vec<String> {
    let completed = state.full_summary.completed_builds.len();
    let failed = state.full_summary.failed_builds.len();
    let dl_done = state.full_summary.completed_downloads.len();
    let dl_running = state.full_summary.running_downloads.len();
    let ul_done = state.full_summary.completed_uploads.len();
    let ul_running = state.full_summary.running_uploads.len();
    let duration = current_time() - state.start_time;
    let now = chrono::Local::now();
    let at = now.format("%H:%M:%S");

    let v = self.colored("┃", Color::DarkBlue);

    let mut lines = Vec::new();
    lines.push(format!(
      "{} Build Summary",
      self.colored("┏━━━", Color::DarkBlue)
    ));

    let ic = self.ic();
    if completed > 0 || failed > 0 {
      let mut bp = Vec::new();
      if completed > 0 {
        bp.push(format!(
          "{} {completed} built",
          self.colored(ic.done, Color::DarkGreen)
        ));
      }
      if failed > 0 {
        bp.push(format!(
          "{} {failed} failed",
          self.colored(ic.failed, Color::DarkRed)
        ));
      }
      lines.push(format!("{}  Builds:     {}", v, bp.join("  ")));
    }

    let total_dl = dl_done + dl_running;
    let total_ul = ul_done + ul_running;
    if total_dl > 0 {
      lines.push(format!(
        "{}  Downloads:  {} fetched",
        v,
        self.colored(&total_dl.to_string(), Color::DarkGreen)
      ));
    }
    if total_ul > 0 {
      lines.push(format!(
        "{}  Uploads:    {} pushed",
        v,
        self.colored(&total_ul.to_string(), Color::DarkGreen)
      ));
    }

    if !state.nix_errors.is_empty() {
      lines.push(format!(
        "{}  {} {} nix error(s)",
        v,
        self.colored(ic.failed, Color::DarkRed),
        state.nix_errors.len()
      ));
    }

    let finish_label = if failed > 0 || !state.nix_errors.is_empty() {
      self.colored(&format!("Exited at {at}"), Color::DarkRed)
    } else {
      self.colored(&format!("Finished at {at}"), Color::DarkGreen)
    };
    lines.push(format!(
      "{} {} after {}",
      self.colored("┗━", Color::DarkBlue),
      finish_label,
      self.colored(&self.format_duration(duration), Color::DarkGrey),
    ));

    lines
  }

  fn render_legend(&self, state: &State, has_tree: bool) -> Vec<String> {
    match self.config.legend_style {
      LegendStyle::Compact => self.render_compact_legend(state),
      LegendStyle::Table => self.render_table_legend(state, has_tree),
      LegendStyle::Verbose => self.render_verbose_legend(state, has_tree),
    }
  }

  fn render_compact_legend(&self, state: &State) -> Vec<String> {
    let running = state.full_summary.running_builds.len();
    let completed = state.full_summary.completed_builds.len();
    let failed = state.full_summary.failed_builds.len();
    let planned = state.full_summary.planned_builds.len();
    let dl = state.full_summary.running_downloads.len();
    let ul = state.full_summary.running_uploads.len();

    if running + completed + failed + planned + dl + ul == 0 {
      return vec![];
    }

    let duration = current_time() - state.start_time;
    let ic = self.ic();

    // Always emit ⏵ │ ✔ │ ✗ │ ⏸, dim zeros
    let mut parts: Vec<String> = Vec::new();
    parts.push(self.count_colored(ic.running, running, Color::DarkYellow));
    parts.push(self.count_colored(ic.done, completed, Color::DarkGreen));
    parts.push(self.count_colored(ic.failed, failed, Color::DarkRed));
    parts.push(self.count_colored(ic.planned, planned, Color::DarkBlue));
    if dl > 0 {
      parts.push(format!(
        "{} {dl}",
        self.colored(ic.download, Color::DarkYellow)
      ));
    }
    if ul > 0 {
      parts.push(format!(
        "{} {ul}",
        self.colored(ic.upload, Color::DarkYellow)
      ));
    }
    parts.push(format!(
      "{} {}",
      self.colored(ic.clock, Color::DarkGrey),
      self.colored(&self.format_duration(duration), Color::DarkGrey),
    ));

    vec![format!(
      "{} {}",
      self.colored("┗━", Color::DarkBlue),
      parts.join(" │ ")
    )]
  }

  fn render_table_legend(&self, state: &State, has_tree: bool) -> Vec<String> {
    let running = state.full_summary.running_builds.len();
    let completed = state.full_summary.completed_builds.len();
    let failed = state.full_summary.failed_builds.len();
    let planned = state.full_summary.planned_builds.len();
    let dl_running = state.full_summary.running_downloads.len();
    let dl_done = state.full_summary.completed_downloads.len();
    let ul_running = state.full_summary.running_uploads.len();
    let ul_done = state.full_summary.completed_uploads.len();

    let show_builds = running + completed + failed + planned > 0;
    let show_dl = dl_running + dl_done > 0;
    let show_ul = ul_running + ul_done > 0;

    if !show_builds && !show_dl && !show_ul {
      return vec![];
    }

    let now = current_time();
    let duration = now - state.start_time;
    let v = self.colored("┃", Color::DarkBlue);

    // Build header section label(s)
    let mut header_parts: Vec<&str> = Vec::new();
    if show_builds {
      header_parts.push("Builds");
    }
    if show_dl {
      header_parts.push("Downloads");
    }
    if show_ul {
      header_parts.push("Uploads");
    }

    let mut lines = Vec::new();

    // ┏━━━ header (or ┣━━━ when appended below a tree)
    let header_prefix = if has_tree {
      "┣━━━"
    } else {
      "┏━━━"
    };
    lines.push(format!(
      "{} {}",
      self.colored(header_prefix, Color::DarkBlue),
      header_parts.join("  ")
    ));

    // Per-running-build rows
    let mut running_entries: Vec<(String, f64, String)> = state
      .full_summary
      .running_builds
      .iter()
      .filter_map(|(drv_id, build)| {
        let info = state.get_derivation_info(*drv_id)?;
        let elapsed = now - build.start;
        let host_label = match &build.host {
          cognos::Host::Remote(h) => {
            format!("  on {}", self.colored(h, Color::DarkMagenta))
          },
          _ => String::new(),
        };
        Some((info.name.name.clone(), elapsed, host_label))
      })
      .collect();
    // Longest running first
    running_entries.sort_by(|a, b| {
      b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
    });

    let dl_name_width = state
      .full_summary
      .running_downloads
      .keys()
      .filter_map(|id| {
        state.store_path_infos.get(id).map(|pi| pi.name.name.len())
      })
      .max()
      .unwrap_or(0);

    let name_width = running_entries
      .iter()
      .map(|(n, ..)| n.len())
      .chain(std::iter::once(dl_name_width))
      .max()
      .unwrap_or(0)
      .min(48);

    // Show per-item rows only when not already shown in the tree above.
    // When has_tree=true the active builds are visible there; the legend
    // only needs to supply the ∑ summary line.
    if !has_tree {
      let ic = self.ic();
      for (name, elapsed, host_label) in &running_entries {
        lines.push(format!(
          "{}  {} {:<width$}  {} {}{}",
          v,
          self.colored(ic.running, Color::DarkYellow),
          self.truncate_name(name, name_width),
          self.colored(ic.clock, Color::DarkGrey),
          self.colored(&self.format_duration(*elapsed), Color::DarkGrey),
          host_label,
          width = name_width,
        ));
      }

      // Per-running-download rows
      for (path_id, transfer) in &state.full_summary.running_downloads {
        if let Some(pi) = state.store_path_infos.get(path_id) {
          let elapsed = now - transfer.start;
          let size_str = if let Some(total) = transfer.total_bytes {
            self.format_bytes(transfer.bytes_transferred, total)
          } else {
            format!("{} B", transfer.bytes_transferred)
          };
          lines.push(format!(
            "{}  {} {:<width$}  {} {} {}",
            v,
            self.colored(ic.download, Color::DarkYellow),
            self.truncate_name(&pi.name.name, name_width),
            self.colored(&size_str, Color::DarkGrey),
            self.colored(ic.clock, Color::DarkGrey),
            self.colored(&self.format_duration(elapsed), Color::DarkGrey),
            width = name_width,
          ));
        }
      }

      // Per-running-upload rows
      for (path_id, transfer) in &state.full_summary.running_uploads {
        if let Some(pi) = state.store_path_infos.get(path_id) {
          let elapsed = now - transfer.start;
          lines.push(format!(
            "{}  {} {:<width$}  {} {}",
            v,
            self.colored(ic.upload, Color::DarkYellow),
            self.truncate_name(&pi.name.name, name_width),
            self.colored(ic.clock, Color::DarkGrey),
            self.colored(&self.format_duration(elapsed), Color::DarkGrey),
            width = name_width,
          ));
        }
      }
    }

    // Always emit all three build-state columns; counts are shown
    // even when zero, just dimmed to grey.
    let ic = self.ic();
    let mut sum_parts: Vec<String> = Vec::new();
    if show_builds {
      sum_parts.push(self.count_colored(
        ic.running,
        running,
        Color::DarkYellow,
      ));
      sum_parts.push(self.count_colored(ic.done, completed, Color::DarkGreen));
      sum_parts.push(self.count_colored(ic.failed, failed, Color::DarkRed));
      sum_parts.push(self.count_colored(ic.planned, planned, Color::DarkBlue));
    }
    if show_dl {
      // Two sub-columns: running (yellow) and done (green)
      if dl_running > 0 || dl_done > 0 {
        sum_parts.push(format!(
          "{} {}",
          self.colored(ic.download, Color::DarkGrey),
          [
            (dl_running > 0).then(|| {
              self.count_colored(ic.running, dl_running, Color::DarkYellow)
            }),
            (dl_done > 0)
              .then(|| self.count_colored(ic.done, dl_done, Color::DarkGreen)),
          ]
          .into_iter()
          .flatten()
          .collect::<Vec<_>>()
          .join(" "),
        ));
      }
    }
    if show_ul && (ul_running > 0 || ul_done > 0) {
      sum_parts.push(format!(
        "{} {}",
        self.colored(ic.upload, Color::DarkGrey),
        [
          (ul_running > 0).then(|| {
            self.count_colored(ic.running, ul_running, Color::DarkYellow)
          }),
          (ul_done > 0)
            .then(|| self.count_colored(ic.done, ul_done, Color::DarkGreen)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" "),
      ));
    }
    // Elapsed with clock icon
    sum_parts.push(format!(
      "{} {}",
      self.colored(ic.clock, Color::DarkGrey),
      self.colored(&self.format_duration(duration), Color::DarkGrey),
    ));

    // ┗━ ∑  [summary]
    lines.push(format!(
      "{} {} {}",
      self.colored("┗━", Color::DarkBlue),
      self.colored(ic.summary, Color::DarkGrey),
      sum_parts.join(" │ ")
    ));

    lines
  }

  fn render_verbose_legend(
    &self,
    state: &State,
    has_tree: bool,
  ) -> Vec<String> {
    let running = state.full_summary.running_builds.len();
    let completed = state.full_summary.completed_builds.len();
    let failed = state.full_summary.failed_builds.len();
    let planned = state.full_summary.planned_builds.len();
    let dl_running = state.full_summary.running_downloads.len();
    let ul_running = state.full_summary.running_uploads.len();

    if running + completed + failed + planned + dl_running + ul_running == 0 {
      return vec![];
    }

    let now = current_time();
    let duration = now - state.start_time;
    let prefix = if has_tree {
      "┣━━━"
    } else {
      "┏━━━"
    };
    let v = self.colored("┃", Color::DarkBlue);

    let mut lines = Vec::new();
    lines.push(format!(
      "{} Build Summary:",
      self.colored(prefix, Color::DarkBlue)
    ));

    // One row per running build: name left-aligned, time right
    let mut running_entries: Vec<(String, String, String)> = state
      .full_summary
      .running_builds
      .iter()
      .filter_map(|(drv_id, build)| {
        let info = state.get_derivation_info(*drv_id)?;
        let elapsed = now - build.start;
        let host = match &build.host {
          cognos::Host::Localhost => String::new(),
          cognos::Host::Remote(h) => {
            format!("  {}", self.colored(h, Color::DarkMagenta))
          },
        };
        Some((info.name.name.clone(), self.format_duration(elapsed), host))
      })
      .collect();
    running_entries.sort_by(|a, b| a.0.cmp(&b.0));

    let name_width = running_entries
      .iter()
      .map(|(n, ..)| n.len())
      .max()
      .unwrap_or(0)
      .min(48);

    let ic = self.ic();
    for (name, elapsed, host) in &running_entries {
      lines.push(format!(
        "{}  {} {:<width$}  {}{}",
        v,
        self.colored(ic.running, Color::DarkYellow),
        self.truncate_name(name, name_width),
        self.colored(elapsed, Color::DarkGrey),
        host,
        width = name_width,
      ));
    }

    // Running downloads
    for (path_id, transfer) in &state.full_summary.running_downloads {
      if let Some(pi) = state.store_path_infos.get(path_id) {
        let elapsed = now - transfer.start;
        let size = if let Some(total) = transfer.total_bytes {
          self.format_bytes(transfer.bytes_transferred, total)
        } else {
          format!("{} B", transfer.bytes_transferred)
        };
        lines.push(format!(
          "{}  {} {:<width$}  {} {}",
          v,
          self.colored(ic.download, Color::DarkYellow),
          self.truncate_name(&pi.name.name, name_width),
          self.colored(&size, Color::DarkGrey),
          self.colored(&self.format_duration(elapsed), Color::DarkGrey),
          width = name_width,
        ));
      }
    }

    let ic = self.ic();
    let mut sum_parts: Vec<String> = Vec::new();
    sum_parts.push(format!(
      "{} {running} running",
      self.colored(ic.running, Color::DarkYellow)
    ));
    sum_parts.push(format!(
      "{} {completed} completed",
      self.colored(ic.done, Color::DarkGreen)
    ));
    sum_parts.push(format!(
      "{} {failed} failed",
      self.colored(ic.failed, Color::DarkRed)
    ));
    sum_parts.push(format!(
      "{} {planned} planned",
      self.colored(ic.planned, Color::DarkBlue)
    ));
    if dl_running > 0 {
      sum_parts.push(format!(
        "{} {dl_running} downloading",
        self.colored(ic.download, Color::DarkYellow)
      ));
    }
    if ul_running > 0 {
      sum_parts.push(format!(
        "{} {ul_running} uploading",
        self.colored(ic.upload, Color::DarkYellow)
      ));
    }
    sum_parts.push(format!(
      "{} {}",
      self.colored(ic.clock, Color::DarkGrey),
      self.colored(&self.format_duration(duration), Color::DarkGrey),
    ));

    lines.push(format!(
      "{} {}",
      self.colored("┗━", Color::DarkBlue),
      sum_parts.join(" │ ")
    ));

    lines
  }

  fn render_plain_view(&self, state: &State) -> Vec<String> {
    let now = current_time();
    let duration = now - state.start_time;
    let running = state.full_summary.running_builds.len();
    let planned = state.full_summary.planned_builds.len();
    let completed = state.full_summary.completed_builds.len();
    let downloading = state.full_summary.running_downloads.len();
    let uploading = state.full_summary.running_uploads.len();

    if running + planned + completed + downloading + uploading == 0 {
      return vec![];
    }

    let mut lines = Vec::new();

    // Running builds
    let mut builds: Vec<_> = state
      .full_summary
      .running_builds
      .iter()
      .filter_map(|(drv_id, build)| {
        let info = state.get_derivation_info(*drv_id)?;
        Some((info.name.name.clone(), build.clone()))
      })
      .collect();
    builds.sort_by(|a, b| a.0.cmp(&b.0));

    let ic = self.ic();

    let mut header_parts: Vec<String> = Vec::new();
    if planned > 0 {
      header_parts.push(format!(
        "{} {planned} planned",
        self.colored(ic.planned, Color::DarkBlue)
      ));
    }
    if downloading > 0 {
      header_parts.push(format!(
        "{} {downloading} downloading",
        self.colored(ic.download, Color::DarkYellow)
      ));
    }
    if uploading > 0 {
      header_parts.push(format!(
        "{} {uploading} uploading",
        self.colored(ic.upload, Color::DarkYellow)
      ));
    }
    let duration_str = self.format_duration(duration);
    let header = if header_parts.is_empty() {
      format!(
        "{} {} {}",
        self.colored("━", Color::DarkBlue),
        self.colored(ic.clock, Color::DarkGrey),
        self.colored(&duration_str, Color::DarkGrey),
      )
    } else {
      format!(
        "{} {} {} {}",
        self.colored("━", Color::DarkBlue),
        self.colored(ic.clock, Color::DarkGrey),
        header_parts.join(" "),
        self.colored(&duration_str, Color::DarkGrey),
      )
    };
    lines.push(header);

    for (name, build) in &builds {
      let elapsed = now - build.start;
      let mut suffix = String::new();
      if let Some(est) = build.estimate {
        let remaining = est.saturating_sub(elapsed as u64);
        suffix = format!(
          "  {} {}",
          self.colored(ic.estimate, Color::DarkGrey),
          self
            .colored(&self.format_duration(remaining as f64), Color::DarkGrey)
        );
      }
      let host_label = match &build.host {
        cognos::Host::Remote(h) => {
          format!("  {}", self.colored(h, Color::DarkMagenta))
        },
        _ => String::new(),
      };
      lines.push(format!(
        "  {} {}  {}{}{}",
        self.colored(ic.running, Color::DarkYellow),
        name,
        self.colored(&self.format_duration(elapsed), Color::DarkGrey),
        suffix,
        host_label,
      ));
    }

    // Running downloads
    for (path_id, transfer) in &state.full_summary.running_downloads {
      if let Some(pi) = state.store_path_infos.get(path_id) {
        let size = if let Some(total) = transfer.total_bytes {
          self.format_bytes(transfer.bytes_transferred, total)
        } else {
          format!("{} B", transfer.bytes_transferred)
        };
        lines.push(format!(
          "  {} {}  {}",
          self.colored(ic.download, Color::DarkYellow),
          pi.name.name,
          self.colored(&size, Color::DarkGrey),
        ));
      }
    }

    // Running uploads
    for (path_id, transfer) in &state.full_summary.running_uploads {
      if let Some(pi) = state.store_path_infos.get(path_id) {
        let size = if let Some(total) = transfer.total_bytes {
          self.format_bytes(transfer.bytes_transferred, total)
        } else {
          format!("{} B", transfer.bytes_transferred)
        };
        lines.push(format!(
          "  {} {}  {}",
          self.colored(ic.upload, Color::DarkYellow),
          pi.name.name,
          self.colored(&size, Color::DarkGrey),
        ));
      }
    }

    lines
  }

  fn render_dashboard_view(&self, state: &State) -> Vec<String> {
    let now = current_time();
    let duration = now - state.start_time;
    let running = state.full_summary.running_builds.len();
    let completed = state.full_summary.completed_builds.len();
    let planned = state.full_summary.planned_builds.len();
    let failed = state.full_summary.failed_builds.len();
    let dl = state.full_summary.running_downloads.len();
    let ul = state.full_summary.running_uploads.len();

    if running + completed + planned + failed + dl + ul == 0 {
      return vec![];
    }

    let ic = self.ic();
    let sep = self.colored(&"─".repeat(44), Color::DarkBlue);
    let pipe = self.colored("│", Color::DarkBlue);

    let title = state
      .forest_roots
      .first()
      .and_then(|&id| state.get_derivation_info(id))
      .map_or_else(|| "Build".to_string(), |info| info.name.name.clone());

    let host = state
      .full_summary
      .running_builds
      .values()
      .find_map(|b| {
        match &b.host {
          cognos::Host::Remote(h) => Some(h.clone()),
          _ => None,
        }
      })
      .unwrap_or_else(|| "localhost".to_string());

    let (status_icon, status_color, status_label) = if running > 0 {
      (ic.running, Color::DarkYellow, "building")
    } else if planned > 0 || dl > 0 {
      (ic.planned, Color::DarkBlue, "waiting")
    } else if failed > 0 {
      (ic.failed, Color::DarkRed, "failed")
    } else {
      (ic.done, Color::DarkGreen, "done")
    };
    let status_str =
      format!("{} {status_label}", self.colored(status_icon, status_color));

    let duration_str = self.format_duration(duration);
    let host_s = self.colored(&host, Color::DarkMagenta);
    let dur_s = self.colored(&duration_str, Color::DarkGrey);
    let fail_s = if failed > 0 && self.config.use_color {
      format!(
        "{}\x1b[1m{failed}\x1b[0m{}",
        SetForegroundColor(Color::DarkRed),
        ResetColor
      )
    } else {
      failed.to_string()
    };
    let summary_str = format!(
      "jobs={}  ok={}  failed={fail_s}  total={dur_s}",
      self.num_str(running + completed + planned + failed),
      self.num_str(completed),
    );

    let header = format!(
      "{} BUILD GRAPH: {title}",
      self.colored("┏━", Color::DarkBlue)
    );

    vec![
      header,
      sep.clone(),
      format!("{:<12} {pipe} {host_s}", "Host"),
      format!("{:<12} {pipe} {status_str}", "Status"),
      format!("{:<12} {pipe} {dur_s}", "Duration"),
      sep,
      format!("{:<12} {pipe} {summary_str}", "Summary"),
    ]
  }

  fn render_dashboard_final(&self, state: &State) -> Vec<String> {
    let duration = current_time() - state.start_time;
    let completed = state.full_summary.completed_builds.len();
    let failed = state.full_summary.failed_builds.len();
    let now = chrono::Local::now();
    let at = now.format("%H:%M:%S");

    let ic = self.ic();
    let sep = self.colored(&"─".repeat(44), Color::DarkBlue);
    let pipe = self.colored("│", Color::DarkBlue);

    let title = state
      .forest_roots
      .first()
      .and_then(|&id| state.get_derivation_info(id))
      .map_or_else(|| "Build".to_string(), |info| info.name.name.clone());

    let host = state
      .full_summary
      .completed_builds
      .values()
      .find_map(|b| {
        match &b.host {
          cognos::Host::Remote(h) => Some(h.clone()),
          _ => None,
        }
      })
      .or_else(|| {
        state.full_summary.failed_builds.values().find_map(|b| {
          match &b.host {
            cognos::Host::Remote(h) => Some(h.clone()),
            _ => None,
          }
        })
      })
      .unwrap_or_else(|| "localhost".to_string());

    let (status_icon, status_color, status_label) =
      if failed > 0 || !state.nix_errors.is_empty() {
        (ic.failed, Color::DarkRed, format!("failed at {at}"))
      } else {
        (ic.done, Color::DarkGreen, format!("finished at {at}"))
      };
    let status_str =
      format!("{} {status_label}", self.colored(status_icon, status_color));

    let duration_str = self.format_duration(duration);
    let host_s = self.colored(&host, Color::DarkMagenta);
    let dur_s = self.colored(&duration_str, Color::DarkGrey);
    let jobs = completed + failed;
    let fail_s = if failed > 0 && self.config.use_color {
      format!(
        "{}\x1b[1m{failed}\x1b[0m{}",
        SetForegroundColor(Color::DarkRed),
        ResetColor
      )
    } else {
      failed.to_string()
    };
    let summary_str = format!(
      "jobs={}  ok={}  failed={fail_s}  total={dur_s}",
      self.num_str(jobs),
      self.num_str(completed),
    );

    let header = format!(
      "{} BUILD GRAPH: {title}",
      self.colored("┏━", Color::DarkBlue)
    );

    vec![
      header,
      sep.clone(),
      format!("{:<12} {pipe} {host_s}", "Host"),
      format!("{:<12} {pipe} {status_str}", "Status"),
      format!("{:<12} {pipe} {dur_s}", "Duration"),
      sep,
      format!("{:<12} {pipe} {summary_str}", "Summary"),
    ]
  }

  fn render_tree_view(&self, state: &State) -> Vec<String> {
    // Show roots that have any interesting build activity. This currently
    // consists of:
    //
    // - actively building
    // - failed
    // - planned (with dependencies)
    // - recently completed
    //
    // Which is the same as showing the full dependency forest, which is
    // what we want to do for the tree view.
    let visible_roots: Vec<DerivationId> = state
      .forest_roots
      .iter()
      .copied()
      .filter(|&drv_id| {
        state
          .get_derivation_info(drv_id)
          .map(|info| self.node_is_visible(info))
          .unwrap_or(false)
      })
      .collect();

    if visible_roots.is_empty() {
      return Vec::new();
    }

    let forest = self.build_forest(state, &visible_roots);

    if forest.is_empty() {
      return Vec::new();
    }

    let mut lines = Vec::new();
    lines.push(format!(
      "{} Dependency Graph:",
      self.colored("┏━", Color::DarkBlue)
    ));

    let n = forest.len();
    if n == 1 {
      // Single root: render directly, no cross-tree connector wrapping.
      // render_tree_node already handles ┃ prefix.
      self.render_tree_node(state, &forest[0], &mut lines);
    } else {
      // Multiple roots: render in reverse, apply forest-level connectors.
      for (rev_i, node) in forest.iter().rev().enumerate() {
        // rev_i == 0 <-> this was the LAST root -> rendered first -> top
        let is_top_tree = rev_i == 0;

        let mut tree_lines: Vec<String> = Vec::new();
        self.render_tree_node(state, node, &mut tree_lines);

        // The root-of-tree line is the LAST element in tree_lines (bottom).
        for (line_idx, tree_line) in tree_lines.iter().enumerate() {
          // Topmost line of this tree block.
          let connector = if is_top_tree {
            if line_idx == 0 {
              self.colored("┌─ ", Color::DarkBlue)
            } else {
              "   ".to_string()
            }
          } else if line_idx == 0 {
            self.colored("├─ ", Color::DarkBlue)
          } else {
            self.colored("│  ", Color::DarkBlue)
          };
          lines.push(format!("{connector}{tree_line}"));
        }
      }
    }

    lines
  }

  /// Determine whether a derivation node is interesting enough to appear in
  /// the tree. Basically, show anything whose subtree summary
  /// has at least one non-empty build/transfer count, or whose own status is
  /// not Unknown-and-empty.
  fn node_is_visible(&self, info: &crate::state::DerivationInfo) -> bool {
    use crate::state::DependencySummary;
    let summary_non_empty = |s: &DependencySummary| {
      !s.planned_builds.is_empty()
        || !s.running_builds.is_empty()
        || !s.completed_builds.is_empty()
        || !s.failed_builds.is_empty()
        || !s.running_downloads.is_empty()
        || !s.running_uploads.is_empty()
        || !s.completed_downloads.is_empty()
        || !s.completed_uploads.is_empty()
    };

    match &info.build_status {
      BuildStatus::Unknown => summary_non_empty(&info.dependency_summary),
      _ => true,
    }
  }

  fn build_forest(
    &self,
    state: &State,
    roots: &[DerivationId],
  ) -> Vec<TreeNode> {
    let mut forest = Vec::new();
    let mut visited = HashSet::new();

    for &root_id in roots {
      if let Some(node) = self.build_tree_node(state, root_id, &mut visited, 0)
      {
        forest.push(node);
      }
    }

    forest
  }

  fn build_tree_node(
    &self,
    state: &State,
    drv_id: DerivationId,
    visited: &mut HashSet<DerivationId>,
    depth: usize,
  ) -> Option<TreeNode> {
    if visited.contains(&drv_id) {
      return None;
    }
    visited.insert(drv_id);

    if depth >= self.config.max_tree_depth {
      return Some(TreeNode {
        drv_id,
        children: Vec::new(),
      });
    }

    let drv_info = state.get_derivation_info(drv_id)?;

    let mut children: Vec<TreeNode> = Vec::new();
    for input in &drv_info.input_derivations {
      let child_info = match state.get_derivation_info(input.derivation) {
        Some(i) => i,
        None => continue,
      };

      // Show the child if it has any build activity (own or in its subtree)
      if !self.node_is_visible(child_info) {
        continue;
      }

      if let Some(child) =
        self.build_tree_node(state, input.derivation, visited, depth + 1)
      {
        children.push(child);
      }
    }

    // Failed > Building > Planned/downloads > Done > Unknown
    children.sort_by_key(|c| {
      state
        .get_derivation_info(c.drv_id)
        .map(|i| self.tree_sort_priority(&i.build_status))
        .unwrap_or(u8::MAX)
    });

    Some(TreeNode { drv_id, children })
  }

  /// Returns a sort priority for tree children.
  /// Lower number = shown first (most urgent / most important).
  fn tree_sort_priority(&self, status: &BuildStatus) -> u8 {
    match status {
      BuildStatus::Failed { .. } => 0,
      BuildStatus::Building(_) => 1,
      BuildStatus::Planned => 2,
      BuildStatus::Unknown => 3,
      BuildStatus::Built { .. } => 4,
    }
  }

  /// Render a single tree node (root-of-a-tree position) and all its
  /// children into `lines`.
  ///
  /// Layout (top -> bottom):
  ///   last child's subtree (┌─ connector, 3-space continuation)
  ///   ...earlier children (├─ connector, │ continuation)...
  ///   root node
  ///
  /// The last sibling ends up at the top with a ┌─ connector; children are
  /// rendered in reverse so that the last child appears first in the output.
  fn render_tree_node(
    &self,
    state: &State,
    node: &TreeNode,
    lines: &mut Vec<String>,
  ) {
    let info = match state.get_derivation_info(node.drv_id) {
      Some(info) => info,
      None => return,
    };

    // Children are iterated in reverse so the original last child is rendered
    // first and appears at the top. The top sibling gets ┌─; all others get ├─.
    let n = node.children.len();
    for (rev_i, child) in node.children.iter().rev().enumerate() {
      let is_top = rev_i == 0;
      self.render_tree_child(
        state,
        child,
        lines,
        is_top,
        &self.colored("┃ ", Color::DarkBlue),
      );
    }

    let _ = n;
    let mut line = String::new();
    line.push_str(&self.colored("┃ ", Color::DarkBlue));
    line.push_str(&self.format_node_content(state, info, false));
    lines.push(line);
  }

  /// Render a child node and its subtree.
  ///
  /// `is_top` is true when this node is the topmost sibling in the display
  /// (i.e. the original last child, rendered first due to the reverse).
  /// Top siblings use `┌─` connector and 3-space continuation above them, all
  /// other siblings use `├─` and `│  ` continuation.
  fn render_tree_child(
    &self,
    state: &State,
    node: &TreeNode,
    lines: &mut Vec<String>,
    is_top: bool,
    prefix: &str,
  ) {
    let info = match state.get_derivation_info(node.drv_id) {
      Some(info) => info,
      None => return,
    };

    // The continuation prefix for grandchildren depends on whether this node is
    // the top sibling or not. The incoming prefix already contains colored
    // characters.
    let child_prefix = if is_top {
      format!("{prefix}   ")
    } else {
      format!("{prefix}{}", self.colored("│  ", Color::DarkBlue))
    };

    for (rev_i, child) in node.children.iter().rev().enumerate() {
      let grandchild_is_top = rev_i == 0;
      self.render_tree_child(
        state,
        child,
        lines,
        grandchild_is_top,
        &child_prefix,
      );
    }

    // prefix + connector + content
    let mut line = String::new();
    line.push_str(prefix);

    // ┌─ for the top sibling (was last before reverse), ├─ for all others
    let connector = if is_top { "┌─ " } else { "├─ " };
    line.push_str(&self.colored(connector, Color::DarkBlue));

    let is_leaf = node.children.is_empty();
    line.push_str(&self.format_node_content(state, info, is_leaf));

    lines.push(line);
  }

  /// Format the textual content for a single tree node (without any connector
  /// prefix). `is_leaf` controls whether a "waiting for ..." annotation is
  /// appended for Planned leaf nodes.
  fn format_node_content(
    &self,
    state: &State,
    info: &crate::state::DerivationInfo,
    is_leaf: bool,
  ) -> String {
    let ic = self.ic();
    let mut s = String::new();

    // Unknown nodes have no icon; all others get icon + space prefix.
    if let Some((icon, color)) = self.get_status_icon(&info.build_status) {
      s.push_str(&self.colored(icon, color));
      s.push(' ');
    }
    // Name color varies by build status.
    let raw_name = self.truncate_name(&info.name.name, 50);
    let name_str = match &info.build_status {
      BuildStatus::Building(_) => {
        self.colored_bold(&raw_name, Color::DarkYellow)
      },
      BuildStatus::Failed { .. } => {
        self.colored_bold(&raw_name, Color::DarkRed)
      },
      BuildStatus::Built { .. } => self.colored(&raw_name, Color::DarkGreen),
      _ => raw_name,
    };
    s.push_str(&name_str);

    match &info.build_status {
      BuildStatus::Building(build_info) => {
        // Show host in magenta.
        if let cognos::Host::Remote(ref host_name) = build_info.host {
          s.push_str(
            &self.colored(&format!(" on {host_name}"), Color::Magenta),
          );
        }

        // Show current build phase in bold
        if let Some(activity_id) = build_info.activity_id
          && let Some(activity) = state.activities.get(&activity_id)
          && let Some(phase) = &activity.phase
        {
          s.push_str(
            &self.colored_bold(&format!(" ({phase})"), Color::DarkGrey),
          );
        }

        let elapsed = current_time() - build_info.start;

        // Hide elapsed if under 1s; it is not meaningful at that resolution.
        if self.config.show_timers && elapsed > 1.0 {
          s.push_str(&self.colored(
            &format!(" {} {}", ic.clock, self.format_duration(elapsed)),
            Color::DarkGrey,
          ));
          // Show the total build estimate after elapsed.
          if let Some(estimate_secs) = build_info.estimate {
            s.push_str(&self.colored(
              &format!(
                " ({} {})",
                ic.estimate,
                self.format_duration(estimate_secs as f64)
              ),
              Color::DarkGrey,
            ));
          }
        }
      },
      BuildStatus::Failed {
        info: build_info,
        fail,
      } => {
        // Host is shown uncolored for failed nodes.
        if let cognos::Host::Remote(ref host_name) = build_info.host {
          s.push_str(&format!(" on {host_name}"));
        }

        // Show failure reason.
        let fail_str = match &fail.fail_type {
          crate::state::FailType::BuildFailed(code) => {
            format!(" failed with exit code {code}")
          },
          crate::state::FailType::Timeout => " timed out".to_string(),
          crate::state::FailType::HashMismatch => " hash mismatch".to_string(),
          crate::state::FailType::DependencyFailed => {
            " dependency failed".to_string()
          },
          crate::state::FailType::Unknown => " failed".to_string(),
        };
        s.push_str(&self.colored(&fail_str, Color::DarkRed));

        // Show build phase if known.
        if let Some(activity_id) = build_info.activity_id
          && let Some(activity) = state.activities.get(&activity_id)
          && let Some(phase) = &activity.phase
        {
          s.push_str(&self.colored(&format!(" in {phase}"), Color::DarkGrey));
        }

        // Hide elapsed if under 1s.
        if self.config.show_timers {
          let duration = fail.at - build_info.start;
          if duration > 1.0 {
            s.push_str(&self.colored(
              &format!(" {} {}", ic.clock, self.format_duration(duration)),
              Color::DarkGrey,
            ));
          }
        }
      },
      BuildStatus::Built {
        info: build_info,
        end,
      } => {
        // Show host (if remote)
        if let cognos::Host::Remote(ref host_name) = build_info.host {
          s.push_str(
            &self.colored(&format!(" on {host_name}"), Color::DarkGrey),
          );
        }
        // Hide elapsed if under 1s.
        if self.config.show_timers {
          let duration = end - build_info.start;
          if duration > 1.0 {
            s.push_str(&self.colored(
              &format!(" {} {}", ic.clock, self.format_duration(duration)),
              Color::DarkGrey,
            ));
          }
        }
      },
      BuildStatus::Planned => {
        // Planned leaf nodes show a "waiting for ..." annotation summarising
        // the unfinished work below them.
        if is_leaf {
          let waiting = self.format_waiting_summary(&info.dependency_summary);
          if !waiting.is_empty() {
            s.push_str(
              &self
                .colored(&format!(" waiting for {waiting}"), Color::DarkGrey),
            );
          }
        }
      },
      BuildStatus::Unknown => {},
    }

    s
  }

  /// Render a compact summary of pending/running activity for a planned leaf
  /// node.
  fn format_waiting_summary(
    &self,
    summary: &crate::state::DependencySummary,
  ) -> String {
    let ic = self.ic();
    let mut parts: Vec<String> = Vec::new();

    let failed = summary.failed_builds.len();
    if failed > 0 {
      parts.push(
        self.colored(&format!("{} {}", ic.failed, failed), Color::DarkRed),
      );
    }

    let running = summary.running_builds.len();
    if running > 0 {
      parts.push(
        self.colored(&format!("{} {}", ic.running, running), Color::DarkYellow),
      );
    }

    let planned = summary.planned_builds.len();
    if planned > 0 {
      parts.push(
        self.colored(&format!("{} {}", ic.planned, planned), Color::DarkBlue),
      );
    }

    parts.join(" ")
  }

  fn get_status_icon(
    &self,
    status: &BuildStatus,
  ) -> Option<(&'static str, Color)> {
    let ic = self.ic();
    match status {
      BuildStatus::Building(_) => Some((ic.running, Color::DarkYellow)),
      BuildStatus::Planned => Some((ic.planned, Color::DarkBlue)),
      BuildStatus::Built { .. } => Some((ic.done, Color::DarkGreen)),
      BuildStatus::Failed { .. } => Some((ic.failed, Color::DarkRed)),
      // Unknown nodes have no icon.
      BuildStatus::Unknown => None,
    }
  }

  /// Shorthand accessor for the configured icon set.
  fn ic(&self) -> &'static Icons {
    self.config.icons
  }

  fn colored(&self, text: &str, color: Color) -> String {
    if self.config.use_color {
      format!("{}{}{}", SetForegroundColor(color), text, ResetColor)
    } else {
      text.to_string()
    }
  }

  /// Render text in the given color AND bold weight.
  fn colored_bold(&self, text: &str, color: Color) -> String {
    if self.config.use_color {
      format!(
        "{}\x1b[1m{}\x1b[0m{}",
        SetForegroundColor(color),
        text,
        ResetColor
      )
    } else {
      text.to_string()
    }
  }

  /// Render an icon + count
  fn count_colored(&self, icon: &str, n: usize, active_color: Color) -> String {
    let icon_s = self.colored(icon, active_color);
    let num_s = if n > 0 && self.config.use_color {
      format!("\x1b[1m{n}\x1b[0m")
    } else {
      n.to_string()
    };
    format!("{icon_s} {num_s}")
  }

  /// Render a count as bold-when-nonzero with no icon. This matches the number
  /// semantics of `count_colored` for use in the dashboard summary row.
  fn num_str(&self, n: usize) -> String {
    if n > 0 && self.config.use_color {
      format!("\x1b[1m{n}\x1b[0m")
    } else {
      n.to_string()
    }
  }

  pub fn format_duration(&self, secs: f64) -> String {
    if secs < 60.0 {
      format!("{secs:.0}s")
    } else if secs < 3600.0 {
      format!("{:.0}m{:.0}s", secs / 60.0, secs % 60.0)
    } else {
      format!("{:.0}h{:.0}m", secs / 3600.0, (secs % 3600.0) / 60.0)
    }
  }

  fn truncate_name(&self, name: &str, max_len: usize) -> String {
    if name.len() <= max_len {
      name.to_string()
    } else {
      format!("{}…", &name[..max_len.saturating_sub(1)])
    }
  }

  fn format_bytes(&self, transferred: u64, total: u64) -> String {
    let pct = if total > 0 {
      (transferred as f64 / total as f64 * 100.0) as u64
    } else {
      0
    };
    format_size(total) + &self.colored(&format!(" ({pct}%)"), Color::DarkGrey)
  }
}

fn visible_width(text: &str) -> usize {
  let mut width = 0;
  let mut chars = text.chars().peekable();

  while let Some(ch) = chars.next() {
    if ch == '\x1b' {
      skip_ansi_escape(&mut chars);
    } else {
      width += UnicodeWidthChar::width(ch).unwrap_or(0);
    }
  }

  width
}

fn skip_ansi_escape<I>(chars: &mut std::iter::Peekable<I>)
where
  I: Iterator<Item = char>,
{
  match chars.peek() {
    Some('[') => {
      chars.next();
      for ch in chars.by_ref() {
        if ('@'..='~').contains(&ch) {
          break;
        }
      }
    },
    Some(']') => {
      chars.next();
      let mut saw_escape = false;
      for ch in chars.by_ref() {
        if saw_escape && ch == '\\' {
          break;
        }
        if ch == '\x07' {
          break;
        }
        saw_escape = ch == '\x1b';
      }
    },
    Some(_) => {
      chars.next();
    },
    None => {},
  }
}

fn format_size(bytes: u64) -> String {
  if bytes < 1024 {
    format!("{bytes} B")
  } else if bytes < 1024 * 1024 {
    format!("{:.1} KiB", bytes as f64 / 1024.0)
  } else if bytes < 1024 * 1024 * 1024 {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
  } else {
    format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn rendered_rows_counts_wrapped_screen_rows() {
    let lines = vec!["abc".to_string(), "abcdef".to_string(), String::new()];

    assert_eq!(
      Display::<Vec<u8>>::rendered_rows_for_width(&lines, Some(3)),
      4
    );
  }

  #[test]
  fn visible_width_ignores_ansi_escape_sequences() {
    assert_eq!(visible_width("\x1b[31mabcdef\x1b[0m"), 6);
    assert_eq!(visible_width("\x1b[1mwide 你\x1b[0m"), 7);
  }
}
