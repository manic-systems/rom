use std::collections::{HashMap, HashSet};

mod layout;
mod row;

use layout::combine_activity_lines;
use ratatui::text::Line;
use row::{
  ActivityLine,
  RenderedActivityLine,
  activity_line,
  transfer_activity_line,
};

use crate::{
  display::DisplayConfig,
  state::{
    BuildStatus,
    DerivationId,
    RenderSnapshot,
    StorePathId,
    TransferInfo,
    current_time,
  },
};

const COMPLETED_ACTIVITY_LINGER_SECONDS: f64 = 3.0;

struct ActivityNode {
  drv_id:         DerivationId,
  children:       Vec<Self>,
  collapsed_deps: CollapsedDependencies,
}

#[derive(Clone, Copy)]
enum BranchConnector {
  Start,
  Continue,
  End,
}

#[derive(Clone, Copy, Debug, Default)]
struct CollapsedDependencies {
  built:   usize,
  waiting: usize,
  shared:  usize,
}

#[derive(Default)]
struct ActivityBuildResult {
  node:           Option<ActivityNode>,
  collapsed_deps: CollapsedDependencies,
}

#[derive(Clone)]
enum TransferActivity {
  Running {
    path_id:  StorePathId,
    transfer: TransferInfo,
  },
  Planned {
    path_id: StorePathId,
  },
}

#[derive(Default)]
struct TransferLookup {
  by_derivation: HashMap<DerivationId, TransferActivity>,
}

#[derive(Clone)]
struct TransferLine {
  path_id: StorePathId,
  line:    Line<'static>,
}

impl CollapsedDependencies {
  fn add(&mut self, other: Self) {
    self.built += other.built;
    self.waiting += other.waiting;
    self.shared += other.shared;
  }
}

impl TransferActivity {
  fn path_id(&self) -> StorePathId {
    match self {
      TransferActivity::Running { path_id, .. }
      | TransferActivity::Planned { path_id } => *path_id,
    }
  }
}

impl TransferLookup {
  fn from_state(state: &RenderSnapshot) -> Self {
    let mut lookup = Self::default();

    for (path_id, transfer) in &state.full_summary.running_downloads {
      lookup.insert_path_activity(state, *path_id, TransferActivity::Running {
        path_id:  *path_id,
        transfer: transfer.clone(),
      });
    }

    for path_id in &state.full_summary.planned_downloads {
      lookup.insert_path_activity(state, *path_id, TransferActivity::Planned {
        path_id: *path_id,
      });
    }

    lookup
  }

  fn derivation_ids(&self) -> impl Iterator<Item = DerivationId> + '_ {
    self.by_derivation.keys().copied()
  }

  fn insert_path_activity(
    &mut self,
    state: &RenderSnapshot,
    path_id: StorePathId,
    activity: TransferActivity,
  ) {
    for drv_id in derivation_ids_for_transfer_path(state, path_id) {
      self.insert(drv_id, activity.clone());
    }
  }

  fn insert(&mut self, drv_id: DerivationId, activity: TransferActivity) {
    use std::collections::hash_map::Entry;

    match self.by_derivation.entry(drv_id) {
      Entry::Vacant(entry) => {
        entry.insert(activity);
      },
      Entry::Occupied(mut entry)
        if transfer_activity_priority(&activity)
          < transfer_activity_priority(entry.get()) =>
      {
        entry.insert(activity);
      },
      Entry::Occupied(_) => {},
    }
  }
}

fn transfer_activity_priority(activity: &TransferActivity) -> u8 {
  match activity {
    TransferActivity::Running { .. } => 0,
    TransferActivity::Planned { .. } => 1,
  }
}

pub(super) fn render_activity_graph_lines(
  state: &RenderSnapshot,
  display: DisplayConfig,
  max_lines: usize,
  width: usize,
) -> Vec<Line<'static>> {
  let now = current_time();
  let transfer_lookup = TransferLookup::from_state(state);
  let forest = build_activity_forest(
    state,
    display.max_tree_depth,
    max_lines,
    now,
    &transfer_lookup,
  );
  let mut tree_lines = Vec::new();

  for node in &forest {
    let connector = if node.children.is_empty() {
      None
    } else {
      Some(BranchConnector::End)
    };
    render_activity_node(
      &mut ActivityRenderCtx {
        state,
        transfer_lookup: &transfer_lookup,
        now,
        width,
      },
      node,
      &[],
      connector,
      &mut tree_lines,
    );
  }

  let transfer_lines = transfer_line_candidates(state, now, width);
  combine_activity_lines(tree_lines, &transfer_lines, max_lines)
}

fn transfer_line_candidates(
  state: &RenderSnapshot,
  now: f64,
  width: usize,
) -> Vec<TransferLine> {
  let mut lines = Vec::new();
  for (path_id, transfer) in &state.full_summary.running_downloads {
    if let Some(line) = transfer_activity_line(
      state,
      &TransferActivity::Running {
        path_id:  *path_id,
        transfer: transfer.clone(),
      },
      now,
      width,
    ) {
      lines.push(TransferLine {
        path_id: *path_id,
        line,
      });
    }
  }

  for path_id in &state.full_summary.planned_downloads {
    if let Some(line) = transfer_activity_line(
      state,
      &TransferActivity::Planned { path_id: *path_id },
      now,
      width,
    ) {
      lines.push(TransferLine {
        path_id: *path_id,
        line,
      });
    }
  }

  lines
}

fn build_activity_forest(
  state: &RenderSnapshot,
  max_depth: usize,
  max_lines: usize,
  now: f64,
  transfer_lookup: &TransferLookup,
) -> Vec<ActivityNode> {
  let mut roots = state.forest_roots.clone();
  if roots.is_empty() {
    roots.extend(state.full_summary.failed_builds.keys().copied());
    roots.extend(state.full_summary.running_builds.keys().copied());
    roots.extend(state.full_summary.planned_builds.iter().copied());
    roots.sort_unstable();
    roots.dedup();
  }
  let roots = select_activity_roots(roots, max_lines);

  let focus_ids = activity_focus_ids(state, transfer_lookup, now);
  let ctx = ActivityBuildCtx {
    state,
    transfer_lookup,
    max_depth,
    now,
    focus_ids: &focus_ids,
  };
  let mut visited = HashSet::new();
  roots
    .into_iter()
    .filter_map(|drv_id| {
      build_activity_node(&ctx, drv_id, 0, true, &mut visited).node
    })
    .collect()
}

fn select_activity_roots(
  roots: Vec<DerivationId>,
  max_lines: usize,
) -> Vec<DerivationId> {
  if max_lines == 0 {
    return Vec::new();
  }
  if roots.len() <= max_lines {
    return roots;
  }
  if max_lines == 1 {
    return roots.into_iter().take(1).collect();
  }

  let front_len = max_lines.div_ceil(2).min(roots.len());
  let tail_len = max_lines.saturating_sub(front_len);
  let tail_start = roots.len().saturating_sub(tail_len);
  let mut selected = Vec::with_capacity(max_lines);

  selected.extend(roots.iter().take(front_len).copied());
  selected.extend(roots.iter().skip(tail_start).copied());
  let mut seen = HashSet::new();
  selected.retain(|id| seen.insert(*id));
  selected
}

struct ActivityBuildCtx<'a> {
  state:           &'a RenderSnapshot,
  transfer_lookup: &'a TransferLookup,
  max_depth:       usize,
  now:             f64,
  focus_ids:       &'a HashSet<DerivationId>,
}

fn build_activity_node(
  ctx: &ActivityBuildCtx<'_>,
  drv_id: DerivationId,
  depth: usize,
  force_visible: bool,
  visited: &mut HashSet<DerivationId>,
) -> ActivityBuildResult {
  let Some(info) = ctx.state.get_derivation_info(drv_id) else {
    return ActivityBuildResult::default();
  };

  if !visited.insert(drv_id) {
    return ActivityBuildResult {
      node:           None,
      collapsed_deps: shared_or_collapsed_dependency(info, ctx.now),
    };
  }

  let mut children = Vec::new();
  let mut collapsed_deps = CollapsedDependencies::default();
  if depth < ctx.max_depth {
    let mut visible_input_ids = Vec::new();
    for input in &info.input_derivations {
      let input_id = input.derivation;
      if should_traverse_activity_child(
        ctx.state,
        ctx.transfer_lookup,
        input_id,
        ctx.focus_ids,
        ctx.now,
      ) {
        visible_input_ids.push(input_id);
      } else {
        collapsed_deps
          .add(collapsed_inactive_dependency(ctx.state, input_id, ctx.now));
      }
    }
    visible_input_ids.sort_by_key(|drv_id| activity_previsit_key(ctx, *drv_id));

    for input_id in visible_input_ids {
      let result =
        build_activity_node(ctx, input_id, depth + 1, false, visited);
      if let Some(child) = result.node {
        children.push(child);
      }
      collapsed_deps.add(result.collapsed_deps);
    }
  }

  let has_transfer_activity =
    derivation_transfer_activity(ctx.transfer_lookup, drv_id).is_some();
  let should_render = force_visible
    || active_activity_status(&info.build_status, ctx.now)
    || has_transfer_activity
    || !children.is_empty();

  if !should_render {
    return ActivityBuildResult {
      node:           None,
      collapsed_deps: collapsed_inactive_dependency(ctx.state, drv_id, ctx.now),
    };
  }

  children.sort_by_key(|node| activity_render_key(ctx, node));

  ActivityBuildResult {
    node:           Some(ActivityNode {
      drv_id,
      children,
      collapsed_deps,
    }),
    collapsed_deps: CollapsedDependencies::default(),
  }
}

fn should_traverse_activity_child(
  state: &RenderSnapshot,
  transfer_lookup: &TransferLookup,
  drv_id: DerivationId,
  focus_ids: &HashSet<DerivationId>,
  now: f64,
) -> bool {
  focus_ids.contains(&drv_id)
    || state.get_derivation_info(drv_id).is_some_and(|info| {
      visible_activity_status(&info.build_status, now)
        || derivation_transfer_activity(transfer_lookup, drv_id).is_some()
    })
}

fn activity_focus_ids(
  state: &RenderSnapshot,
  transfer_lookup: &TransferLookup,
  now: f64,
) -> HashSet<DerivationId> {
  let mut focus = HashSet::new();

  focus.extend(state.full_summary.failed_builds.keys().copied());
  focus.extend(state.full_summary.running_builds.keys().copied());
  focus.extend(state.full_summary.completed_builds.iter().filter_map(
    |(drv_id, build)| {
      (now - build.end < COMPLETED_ACTIVITY_LINGER_SECONDS).then_some(*drv_id)
    },
  ));

  focus.extend(transfer_lookup.derivation_ids());

  let mut stack = focus.iter().copied().collect::<Vec<_>>();
  while let Some(drv_id) = stack.pop() {
    let Some(info) = state.get_derivation_info(drv_id) else {
      continue;
    };
    for parent_id in &info.derivation_parents {
      if focus.insert(*parent_id) {
        stack.push(*parent_id);
      }
    }
  }

  focus
}

fn derivation_ids_for_transfer_path(
  state: &RenderSnapshot,
  path_id: crate::state::StorePathId,
) -> Vec<DerivationId> {
  let Some(store_path) = state.get_store_path_info(path_id) else {
    return Vec::new();
  };

  let mut ids = store_path
    .producer
    .into_iter()
    .chain(state.derivation_ids_with_name(&store_path.name.name))
    .collect::<Vec<_>>();
  ids.sort_unstable();
  ids.dedup();
  ids
}

fn derivation_transfer_activity(
  transfer_lookup: &TransferLookup,
  drv_id: DerivationId,
) -> Option<TransferActivity> {
  transfer_lookup.by_derivation.get(&drv_id).cloned()
}

fn activity_previsit_key(
  ctx: &ActivityBuildCtx<'_>,
  drv_id: DerivationId,
) -> (bool, u8, std::cmp::Reverse<usize>, DerivationId) {
  let Some(info) = ctx.state.get_derivation_info(drv_id) else {
    return (true, u8::MAX, std::cmp::Reverse(0), drv_id);
  };
  (
    info.input_derivations.is_empty(),
    activity_sort_priority(ctx.transfer_lookup, drv_id, info),
    std::cmp::Reverse(info.input_derivations.len()),
    drv_id,
  )
}

fn shared_or_collapsed_dependency(
  info: &crate::state::DerivationInfo,
  now: f64,
) -> CollapsedDependencies {
  if active_activity_status(&info.build_status, now)
    || matches!(info.build_status, BuildStatus::Planned)
  {
    CollapsedDependencies {
      built:   0,
      waiting: 0,
      shared:  1,
    }
  } else {
    collapsed_self_dependency(info, now)
  }
}

fn collapsed_inactive_dependency(
  state: &RenderSnapshot,
  drv_id: DerivationId,
  now: f64,
) -> CollapsedDependencies {
  let Some(info) = state.get_derivation_info(drv_id) else {
    return CollapsedDependencies::default();
  };

  let summary = &info.dependency_summary;
  let mut deps = CollapsedDependencies {
    built:   summary
      .completed_builds
      .values()
      .filter(|build| now - build.end >= COMPLETED_ACTIVITY_LINGER_SECONDS)
      .count(),
    waiting: summary.planned_builds.len(),
    shared:  0,
  };

  let own = collapsed_self_dependency(info, now);
  if deps.built == 0 && deps.waiting == 0 {
    deps.add(own);
  }

  deps
}

fn collapsed_self_dependency(
  info: &crate::state::DerivationInfo,
  now: f64,
) -> CollapsedDependencies {
  match &info.build_status {
    BuildStatus::Planned => {
      CollapsedDependencies {
        built:   0,
        waiting: 1,
        shared:  0,
      }
    },
    BuildStatus::Built { end, .. }
      if now - *end >= COMPLETED_ACTIVITY_LINGER_SECONDS =>
    {
      CollapsedDependencies {
        built:   1,
        waiting: 0,
        shared:  0,
      }
    },
    _ => CollapsedDependencies::default(),
  }
}

fn active_activity_status(status: &BuildStatus, now: f64) -> bool {
  match status {
    BuildStatus::Building(_) | BuildStatus::Failed { .. } => true,
    BuildStatus::Built { end, .. } => {
      (0.0..COMPLETED_ACTIVITY_LINGER_SECONDS).contains(&(now - *end))
    },
    BuildStatus::Planned | BuildStatus::Unknown => false,
  }
}

fn visible_activity_status(status: &BuildStatus, now: f64) -> bool {
  match status {
    BuildStatus::Building(_)
    | BuildStatus::Failed { .. }
    | BuildStatus::Planned => true,
    BuildStatus::Built { end, .. } => {
      (0.0..COMPLETED_ACTIVITY_LINGER_SECONDS).contains(&(now - *end))
    },
    BuildStatus::Unknown => false,
  }
}

fn activity_node_priority(
  ctx: &ActivityBuildCtx<'_>,
  node: &ActivityNode,
) -> u8 {
  let own_priority = ctx
    .state
    .get_derivation_info(node.drv_id)
    .map(|info| activity_sort_priority(ctx.transfer_lookup, node.drv_id, info))
    .unwrap_or(u8::MAX);
  node
    .children
    .iter()
    .map(|child| activity_node_priority(ctx, child))
    .min()
    .map_or(own_priority, |child_priority| {
      own_priority.min(child_priority)
    })
}

fn activity_render_key(
  ctx: &ActivityBuildCtx<'_>,
  node: &ActivityNode,
) -> (std::cmp::Reverse<u8>, bool, DerivationId) {
  (
    std::cmp::Reverse(activity_node_priority(ctx, node)),
    !node.children.is_empty(),
    node.drv_id,
  )
}

fn activity_sort_priority(
  transfer_lookup: &TransferLookup,
  drv_id: DerivationId,
  info: &crate::state::DerivationInfo,
) -> u8 {
  if matches!(&info.build_status, BuildStatus::Failed { .. }) {
    return 0;
  }
  if matches!(&info.build_status, BuildStatus::Building(_)) {
    return 1;
  }

  match derivation_transfer_activity(transfer_lookup, drv_id) {
    Some(TransferActivity::Running { .. }) => return 1,
    Some(TransferActivity::Planned { .. }) => return 2,
    None => {},
  }

  match &info.build_status {
    BuildStatus::Built { .. } => 2,
    BuildStatus::Planned => 3,
    BuildStatus::Unknown
    | BuildStatus::Building(_)
    | BuildStatus::Failed { .. } => 4,
  }
}

struct ActivityRenderCtx<'a> {
  state:           &'a RenderSnapshot,
  transfer_lookup: &'a TransferLookup,
  now:             f64,
  width:           usize,
}

fn render_activity_node(
  ctx: &mut ActivityRenderCtx<'_>,
  node: &ActivityNode,
  branch_rails: &[bool],
  connector: Option<BranchConnector>,
  lines: &mut Vec<RenderedActivityLine>,
) {
  let Some(info) = ctx.state.get_derivation_info(node.drv_id) else {
    return;
  };

  let renders_self = visible_activity_status(&info.build_status, ctx.now)
    || derivation_transfer_activity(ctx.transfer_lookup, node.drv_id).is_some();
  let child_count = node.children.len();
  let child_rails = if renders_self {
    let mut rails = branch_rails.to_vec();
    rails.push(child_count > 1);
    rails
  } else {
    branch_rails.to_vec()
  };

  for (index, child) in node.children.iter().enumerate() {
    let child_connector = (!child_rails.is_empty())
      .then(|| child_connector(index, child_count, renders_self));
    render_activity_node(ctx, child, &child_rails, child_connector, lines);
  }

  if renders_self {
    let transfer_path_id =
      derivation_transfer_activity(ctx.transfer_lookup, node.drv_id)
        .map(|transfer| transfer.path_id());
    lines.push(activity_line(ActivityLine {
      state: ctx.state,
      transfer_lookup: ctx.transfer_lookup,
      drv_id: node.drv_id,
      info,
      transfer_path_id,
      collapsed_deps: node.collapsed_deps,
      branch_rails,
      connector,
      has_children: !node.children.is_empty(),
      now: ctx.now,
      width: ctx.width,
    }));
  }
}

fn child_connector(
  index: usize,
  sibling_count: usize,
  parent_renders_self: bool,
) -> BranchConnector {
  if index == 0 {
    BranchConnector::Start
  } else if parent_renders_self || index < sibling_count.saturating_sub(1) {
    BranchConnector::Continue
  } else {
    BranchConnector::End
  }
}
