use ratatui::{
  style::{Color, Style},
  text::{Line, Span},
};

use super::{
  BranchConnector,
  CollapsedDependencies,
  TransferActivity,
  TransferLookup,
  active_activity_status,
  derivation_transfer_activity,
};
use crate::{
  display::format_duration,
  state::{BuildInfo, BuildStatus, DerivationId, RenderSnapshot, StorePathId},
  tui::{
    BUILT_GREEN,
    DOWNLOAD_BLUE,
    MOSS_GREEN,
    MUTED_RED,
    MUTED_YELLOW,
    hierarchy_style,
    secondary_style,
    spinner_frame,
  },
};

#[derive(Clone, Copy, Default)]
pub(super) struct GraphCell(u8);

const EDGE_UP: u8 = 0b0001;
const EDGE_DOWN: u8 = 0b0010;
const EDGE_LEFT: u8 = 0b0100;
const EDGE_RIGHT: u8 = 0b1000;
const EDGE_VERTICAL: u8 = EDGE_UP | EDGE_DOWN;
const EDGE_HORIZONTAL: u8 = EDGE_LEFT | EDGE_RIGHT;
const EDGE_TOP_LEFT_CORNER: u8 = EDGE_DOWN | EDGE_RIGHT;
const EDGE_BOTTOM_LEFT_CORNER: u8 = EDGE_UP | EDGE_RIGHT;
const EDGE_TOP_RIGHT_CORNER: u8 = EDGE_DOWN | EDGE_LEFT;
const EDGE_BOTTOM_RIGHT_CORNER: u8 = EDGE_UP | EDGE_LEFT;
const EDGE_RIGHT_TEE: u8 = EDGE_UP | EDGE_DOWN | EDGE_RIGHT;
const EDGE_LEFT_TEE: u8 = EDGE_UP | EDGE_DOWN | EDGE_LEFT;
const EDGE_TOP_TEE: u8 = EDGE_LEFT | EDGE_RIGHT | EDGE_DOWN;
const EDGE_BOTTOM_TEE: u8 = EDGE_LEFT | EDGE_RIGHT | EDGE_UP;
const EDGE_CROSS: u8 = EDGE_UP | EDGE_DOWN | EDGE_LEFT | EDGE_RIGHT;

impl GraphCell {
  fn add(&mut self, edges: u8) {
    self.0 |= edges;
  }

  pub(super) fn clear(&mut self) {
    self.0 = 0;
  }

  pub(super) fn has_up_edge(self) -> bool {
    self.0 & EDGE_UP != 0
  }

  pub(super) fn has_down_edge(self) -> bool {
    self.0 & EDGE_DOWN != 0
  }

  pub(super) fn is_vertical_rail(self) -> bool {
    matches!(self.0, EDGE_VERTICAL | EDGE_UP | EDGE_DOWN)
  }

  fn symbol(self) -> &'static str {
    match self.0 {
      0 => " ",
      EDGE_VERTICAL => "│",
      EDGE_HORIZONTAL => "─",
      EDGE_TOP_LEFT_CORNER => "┌",
      EDGE_BOTTOM_LEFT_CORNER => "└",
      EDGE_TOP_RIGHT_CORNER => "┐",
      EDGE_BOTTOM_RIGHT_CORNER => "┘",
      EDGE_RIGHT_TEE => "├",
      EDGE_LEFT_TEE => "┤",
      EDGE_TOP_TEE => "┬",
      EDGE_BOTTOM_TEE => "┴",
      EDGE_CROSS => "┼",
      EDGE_LEFT => "─",
      EDGE_RIGHT => "─",
      EDGE_UP => "│",
      EDGE_DOWN => "│",
      _ => "┼",
    }
  }
}

const MAX_ACTIVITY_NAME_CHARS: usize = 56;

#[derive(Clone)]
enum RowActivity {
  Build,
  Download(TransferActivity),
}

fn row_activity(
  transfer_lookup: &TransferLookup,
  drv_id: DerivationId,
  info: &crate::state::DerivationInfo,
  now: f64,
) -> RowActivity {
  if active_activity_status(&info.build_status, now) {
    return RowActivity::Build;
  }
  if let Some(transfer) = derivation_transfer_activity(transfer_lookup, drv_id)
  {
    return RowActivity::Download(transfer);
  }
  RowActivity::Build
}

pub(super) struct ActivityLine<'a> {
  pub(super) state:            &'a RenderSnapshot,
  pub(super) transfer_lookup:  &'a TransferLookup,
  pub(super) drv_id:           DerivationId,
  pub(super) info:             &'a crate::state::DerivationInfo,
  pub(super) transfer_path_id: Option<StorePathId>,
  pub(super) collapsed_deps:   CollapsedDependencies,
  pub(super) branch_rails:     &'a [bool],
  pub(super) connector:        Option<BranchConnector>,
  pub(super) has_children:     bool,
  pub(super) now:              f64,
  pub(super) width:            usize,
}

#[derive(Clone)]
pub(super) struct RenderedActivityLine {
  pub(super) graph_cells:      Vec<GraphCell>,
  pub(super) body:             Vec<Span<'static>>,
  pub(super) transfer_path_id: Option<StorePathId>,
}

impl RenderedActivityLine {
  pub(super) fn to_line(&self) -> Line<'static> {
    let mut spans =
      Vec::with_capacity(self.graph_cells.len() + self.body.len() + 1);
    spans.extend(
      self
        .graph_cells
        .iter()
        .map(|cell| Span::styled(cell.symbol(), hierarchy_style())),
    );
    if !self.graph_cells.is_empty() {
      spans.push(Span::raw(" "));
    }
    spans.extend(self.body.iter().cloned());
    Line::from(spans)
  }
}

pub(super) fn activity_line(args: ActivityLine<'_>) -> RenderedActivityLine {
  let ActivityLine {
    state,
    transfer_lookup,
    drv_id,
    info,
    transfer_path_id,
    collapsed_deps,
    branch_rails,
    connector,
    has_children,
    now,
    width,
  } = args;
  let graph_cells =
    activity_prefix_cells(branch_rails, connector, has_children);
  let prefix_width = graph_cells.len() + usize::from(!graph_cells.is_empty());
  let row_activity = row_activity(transfer_lookup, drv_id, info, now);
  let (status, status_style) =
    status_indicator(&row_activity, &info.build_status, now);
  let status_prefix_width = if status.is_empty() {
    0
  } else {
    status.chars().count() + 1
  };
  let suffix = activity_suffix(state, info, collapsed_deps, &row_activity);
  let elapsed = activity_elapsed(&row_activity, &info.build_status, now);
  let name = fit_activity_name(
    &info.name.name,
    width,
    prefix_width,
    status_prefix_width,
    suffix.as_deref(),
    &elapsed,
  );

  let mut body = Vec::new();
  if !status.is_empty() {
    body.push(Span::styled(status, status_style));
    body.push(Span::raw(" "));
  }
  body.push(Span::styled(
    name,
    name_style(&row_activity, &info.build_status, branch_rails.len()),
  ));

  if let Some(suffix) = suffix {
    body.push(Span::raw(" "));
    body.push(Span::styled(suffix, secondary_style()));
  }

  if !elapsed.is_empty() {
    body.push(Span::raw(" "));
    body.push(Span::styled(elapsed, secondary_style()));
  }

  RenderedActivityLine {
    graph_cells,
    body,
    transfer_path_id,
  }
}

pub(super) fn transfer_activity_line(
  state: &RenderSnapshot,
  transfer: &TransferActivity,
  now: f64,
  width: usize,
) -> Option<Line<'static>> {
  let name = state
    .get_store_path_info(transfer.path_id())?
    .name
    .name
    .clone();
  let row_activity = RowActivity::Download(transfer.clone());
  let (status, status_style) =
    status_indicator(&row_activity, &BuildStatus::Unknown, now);
  let status_prefix_width = if status.is_empty() {
    0
  } else {
    status.chars().count() + 1
  };
  let suffix = download_suffix(transfer);
  let elapsed = activity_elapsed(&row_activity, &BuildStatus::Unknown, now);
  let name = fit_activity_name(
    &name,
    width,
    0,
    status_prefix_width,
    suffix.as_deref(),
    &elapsed,
  );

  let mut spans = Vec::new();
  if !status.is_empty() {
    spans.push(Span::styled(status, status_style));
    spans.push(Span::raw(" "));
  }
  spans.push(Span::styled(
    name,
    name_style(&row_activity, &BuildStatus::Unknown, 0),
  ));

  if let Some(suffix) = suffix {
    spans.push(Span::raw(" "));
    spans.push(Span::styled(suffix, secondary_style()));
  }

  if !elapsed.is_empty() {
    spans.push(Span::raw(" "));
    spans.push(Span::styled(elapsed, secondary_style()));
  }

  Some(Line::from(spans))
}

fn activity_prefix_cells(
  branch_rails: &[bool],
  connector: Option<BranchConnector>,
  has_children: bool,
) -> Vec<GraphCell> {
  let Some(connector) = connector else {
    return Vec::new();
  };

  let connector_col = branch_rails.len().saturating_sub(1) * 2;
  let bridges_to_children = has_children && !branch_rails.is_empty();
  let graph_width = connector_col + if bridges_to_children { 4 } else { 2 };
  let mut cells = vec![GraphCell::default(); graph_width];

  for rail in branch_rails
    .iter()
    .take(branch_rails.len().saturating_sub(1))
    .enumerate()
  {
    let (index, rail) = rail;
    if *rail {
      cells[index * 2].add(EDGE_UP | EDGE_DOWN);
    }
  }

  let vertical_edges = match connector {
    BranchConnector::Start => EDGE_DOWN,
    BranchConnector::Continue => EDGE_UP | EDGE_DOWN,
    BranchConnector::End => EDGE_UP,
  };
  cells[connector_col].add(vertical_edges);
  connect_cells(&mut cells, connector_col, connector_col + 1);

  if bridges_to_children {
    connect_cells(&mut cells, connector_col + 1, connector_col + 2);
    cells[connector_col + 2].add(EDGE_UP);
    connect_cells(&mut cells, connector_col + 2, connector_col + 3);
  }

  cells
}

fn connect_cells(cells: &mut [GraphCell], left: usize, right: usize) {
  cells[left].add(EDGE_RIGHT);
  cells[right].add(EDGE_LEFT);
}

fn status_indicator(
  row_activity: &RowActivity,
  status: &BuildStatus,
  now: f64,
) -> (String, Style) {
  match row_activity {
    RowActivity::Download(TransferActivity::Running { .. }) => {
      (
        format!("↓ {}", spinner_frame(now)),
        Style::default().fg(DOWNLOAD_BLUE),
      )
    },
    RowActivity::Download(TransferActivity::Planned { .. }) => {
      ("↓".to_string(), Style::default().fg(DOWNLOAD_BLUE))
    },
    RowActivity::Build => {
      match status {
        BuildStatus::Building(_) => {
          (
            spinner_frame(now).to_string(),
            Style::default().fg(MOSS_GREEN),
          )
        },
        BuildStatus::Built { .. } => {
          ("✓".to_string(), Style::default().fg(BUILT_GREEN))
        },
        BuildStatus::Failed { .. } => {
          ("✗".to_string(), Style::default().fg(MUTED_RED))
        },
        BuildStatus::Planned => {
          ("".to_string(), Style::default().fg(MUTED_YELLOW))
        },
        BuildStatus::Unknown => (" ".to_string(), Style::default()),
      }
    },
  }
}

fn activity_suffix(
  state: &RenderSnapshot,
  info: &crate::state::DerivationInfo,
  collapsed_deps: CollapsedDependencies,
  row_activity: &RowActivity,
) -> Option<String> {
  let status_suffix = match row_activity {
    RowActivity::Download(transfer) => download_suffix(transfer),
    RowActivity::Build => {
      match &info.build_status {
        BuildStatus::Building(build) => running_suffix(state, build),
        BuildStatus::Failed { info: build, fail } => {
          Some(failed_suffix(state, build, fail))
        },
        BuildStatus::Built { .. } if info.cached => Some("cached".to_string()),
        BuildStatus::Built { .. } => None,
        BuildStatus::Planned => None,
        BuildStatus::Unknown => None,
      }
    },
  };
  combine_suffixes(status_suffix, collapsed_deps_suffix(collapsed_deps))
}

fn download_suffix(transfer: &TransferActivity) -> Option<String> {
  match transfer {
    TransferActivity::Running { transfer, .. } => {
      transfer.total_bytes.map(|total| {
        format!(
          "{} / {}",
          format_bytes(transfer.bytes_transferred),
          format_bytes(total)
        )
      })
    },
    TransferActivity::Planned { .. } => None,
  }
}

fn format_bytes(bytes: u64) -> String {
  const KIB: f64 = 1024.0;
  const MIB: f64 = KIB * 1024.0;
  const GIB: f64 = MIB * 1024.0;

  let bytes = bytes as f64;
  if bytes >= GIB {
    format!("{:.1} GiB", bytes / GIB)
  } else if bytes >= MIB {
    format!("{:.1} MiB", bytes / MIB)
  } else if bytes >= KIB {
    format!("{:.1} KiB", bytes / KIB)
  } else {
    format!("{bytes:.0} B")
  }
}

fn combine_suffixes(
  first: Option<String>,
  second: Option<String>,
) -> Option<String> {
  match (first, second) {
    (Some(first), Some(second)) => Some(format!("{first}, {second}")),
    (Some(first), None) => Some(first),
    (None, Some(second)) => Some(second),
    (None, None) => None,
  }
}

fn collapsed_deps_suffix(deps: CollapsedDependencies) -> Option<String> {
  let mut parts = Vec::new();
  match deps.built {
    0 => {},
    1 => parts.push("1 dep built".to_string()),
    built => parts.push(format!("{built} deps built")),
  }
  match deps.waiting {
    0 => {},
    1 => parts.push("1 waiting".to_string()),
    waiting => parts.push(format!("{waiting} waiting")),
  }
  match deps.shared {
    0 => {},
    1 => parts.push("1 shared".to_string()),
    shared => parts.push(format!("{shared} shared")),
  }

  if parts.is_empty() {
    None
  } else {
    Some(parts.join(", "))
  }
}

fn running_suffix(state: &RenderSnapshot, build: &BuildInfo) -> Option<String> {
  let phase = build
    .activity_id
    .and_then(|id| state.activities.get(&id))
    .and_then(|activity| activity.phase.as_deref());
  let host = remote_host(&build.host);

  match (phase, host) {
    (Some(phase), Some(host)) => Some(format!("{phase} on {host}")),
    (Some(phase), None) => Some(phase.to_string()),
    (None, Some(host)) => Some(format!("on {host}")),
    (None, None) => None,
  }
}

fn failed_suffix(
  state: &RenderSnapshot,
  build: &BuildInfo,
  fail: &crate::state::BuildFail,
) -> String {
  let mut suffix = match &fail.fail_type {
    crate::state::FailType::BuildFailed(code) => {
      format!("failed with exit code {code}")
    },
    crate::state::FailType::Timeout => "timed out".to_string(),
    crate::state::FailType::HashMismatch => "hash mismatch".to_string(),
    crate::state::FailType::DependencyFailed => "dependency failed".to_string(),
    crate::state::FailType::Unknown => "failed".to_string(),
  };

  if let Some(phase) = build
    .activity_id
    .and_then(|id| state.activities.get(&id))
    .and_then(|activity| activity.phase.as_deref())
  {
    suffix.push_str(" in ");
    suffix.push_str(phase);
  }

  if let Some(host) = remote_host(&build.host) {
    suffix.push_str(" on ");
    suffix.push_str(host);
  }

  suffix
}

fn remote_host(host: &cognos::Host) -> Option<&str> {
  match host {
    cognos::Host::Remote(host) => Some(host),
    cognos::Host::Localhost => None,
  }
}

fn activity_elapsed(
  row_activity: &RowActivity,
  status: &BuildStatus,
  now: f64,
) -> String {
  match row_activity {
    RowActivity::Download(TransferActivity::Running { transfer, .. }) => {
      elapsed_since(transfer.start, now)
    },
    RowActivity::Download(TransferActivity::Planned { .. }) => String::new(),
    RowActivity::Build => {
      match status {
        BuildStatus::Building(build) => elapsed_since(build.start, now),
        BuildStatus::Built { info, end } => elapsed_since(info.start, *end),
        BuildStatus::Failed { info, fail } => {
          elapsed_since(info.start, fail.at)
        },
        BuildStatus::Planned | BuildStatus::Unknown => String::new(),
      }
    },
  }
}

fn elapsed_since(start: f64, end: f64) -> String {
  let elapsed = (end - start).max(0.0);
  if elapsed < 0.3 {
    String::new()
  } else {
    format_duration(elapsed)
  }
}

fn fit_activity_name(
  name: &str,
  width: usize,
  prefix_width: usize,
  status_prefix_width: usize,
  suffix: Option<&str>,
  elapsed: &str,
) -> String {
  let suffix_width =
    suffix.map(|suffix| suffix.chars().count() + 1).unwrap_or(0);
  let elapsed_width = if elapsed.is_empty() {
    0
  } else {
    elapsed.chars().count() + 1
  };
  let fixed_width =
    prefix_width + status_prefix_width + suffix_width + elapsed_width;
  let available = width
    .saturating_sub(fixed_width)
    .clamp(1, MAX_ACTIVITY_NAME_CHARS);

  truncate_start_chars(name, available)
}

fn truncate_start_chars(text: &str, max_chars: usize) -> String {
  if text.chars().count() <= max_chars {
    return text.to_string();
  }
  if max_chars <= 1 {
    return "…".to_string();
  }

  let skip = text.chars().count().saturating_sub(max_chars - 1);
  format!("…{}", text.chars().skip(skip).collect::<String>())
}

fn name_style(
  row_activity: &RowActivity,
  status: &BuildStatus,
  depth: usize,
) -> Style {
  match row_activity {
    RowActivity::Download(TransferActivity::Running { .. }) => {
      Style::default().fg(DOWNLOAD_BLUE)
    },
    RowActivity::Download(TransferActivity::Planned { .. }) => {
      Style::default().fg(MUTED_YELLOW)
    },
    RowActivity::Build => {
      match status {
        BuildStatus::Failed { .. } => Style::default().fg(MUTED_RED),
        BuildStatus::Planned | BuildStatus::Unknown => {
          Style::default().fg(MUTED_YELLOW)
        },
        BuildStatus::Built { .. } => Style::default().fg(Color::Gray),
        BuildStatus::Building(_) if depth == 0 => {
          Style::default().fg(MOSS_GREEN)
        },
        BuildStatus::Building(_) => Style::default().fg(MOSS_GREEN),
      }
    },
  }
}
