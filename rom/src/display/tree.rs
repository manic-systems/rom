//! Dependency tree planning and rendering.

use std::{
  cell::RefCell,
  cmp::Reverse,
  collections::{BinaryHeap, VecDeque},
  path::Path,
};

use cognos::Host;
use foldhash::{HashMap, HashMapExt, HashSet, HashSetExt};
use ratatui_core::{
  style::Color,
  text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::{
  Direction,
  MIN_TIMER_SECONDS,
  Renderer,
  Transfer,
  aggregate_transfers,
  fit_line,
  format_bytes,
  format_duration,
  format_timer,
  model::{RenderSnapshot, TRANSFER_DISPLAY_DELAY, TransferPlacement},
  progress_bar,
  span,
  spans_width,
  truncate_middle,
};
use crate::state::{
  Blocker,
  BuildInfo,
  BuildStatus,
  DerivationId,
  DerivationInfo,
  Membership,
  SourceFetch,
  elapsed,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum RowId {
  Build(DerivationId),
  Transfer(usize),
  SourceFetch(usize),
  TransferGroup,
  /// Unmatched transfers of unknown size in one direction, shown together.
  Unsized(Direction),
}

/// Seconds without progress before a transfer counts as stalled.
const STALL_AFTER: f64 = 3.0;
/// Widest a bar gets.
const BAR_WIDTH: usize = 28;
/// Narrowest bar worth drawing; narrower terminals drop the bar column.
const MIN_BAR_WIDTH: usize = 8;
/// Widest the name column gets, connectors included.
const NAME_WIDTH: usize = 44;
/// Columns kept for status text right of the bars.
const STATUS_WIDTH: usize = 24;

/// How far a row's work has come, drawn in the bar column.
#[derive(Clone, Copy)]
enum Bar {
  /// `done` of `total`, in one color.
  Progress {
    done:  u64,
    total: u64,
    color: Color,
  },
  /// Work of unknown size, drawn as a moving segment.
  Busy(Color),
}

/// A tree row before its columns are laid out.
struct Row {
  /// Connectors and the status icon.
  lead:   Vec<Span<'static>>,
  name:   String,
  color:  Color,
  bar:    Option<Bar>,
  /// Details right of the bar, each starting with its own gap.
  status: Vec<Span<'static>>,
}

struct PlannedRow {
  priority:     u8,
  active:       bool,
  active_below: usize,
  /// The producer folded into this row.
  producer:     Option<DerivationId>,
  children:     Vec<RowId>,
  inline:       Vec<usize>,
  consumers:    usize,
}

impl PlannedRow {
  const fn new(priority: u8) -> Self {
    Self {
      priority,
      active: priority >= 3,
      active_below: 0,
      producer: None,
      children: Vec::new(),
      inline: Vec::new(),
      consumers: 0,
    }
  }
}

#[derive(Default)]
pub(super) struct TreePlan {
  rows:         HashMap<RowId, PlannedRow>,
  roots:        Vec<RowId>,
  hidden:       usize,
  /// Unmatched transfers drawn together as [`RowId::Unsized`].
  unknown_size: Vec<usize>,
}

impl TreePlan {
  pub(super) fn line_count(&self) -> usize {
    // The Builds header is not represented by a row.
    self
      .rows
      .len()
      .saturating_add(self.hidden)
      .saturating_add(1)
  }

  fn focus_active(&self) -> bool {
    self.rows.values().any(|row| row.active)
  }

  fn row(&self, id: RowId) -> &PlannedRow {
    &self.rows[&id]
  }

  fn row_mut(&mut self, id: RowId) -> &mut PlannedRow {
    self.rows.get_mut(&id).expect("planned row exists")
  }
}

struct TreeSelection<'a> {
  plan:    &'a TreePlan,
  rows:    HashSet<RowId>,
  roots:   Vec<RowId>,
  parents: HashMap<RowId, Option<RowId>>,
  maximum: usize,
}

/// Places each unit under its consumer furthest from the package, lazily.
struct UnitParents<'r, 'a> {
  snapshot: &'r RenderSnapshot<'a>,
  staged:   HashSet<(DerivationId, DerivationId)>,
  depths:   RefCell<HashMap<DerivationId, usize>>,
  parents:  RefCell<HashMap<DerivationId, Option<DerivationId>>>,
}

impl UnitParents<'_, '_> {
  fn get(&self, id: DerivationId) -> Option<DerivationId> {
    if let Some(&parent) = self.parents.borrow().get(&id) {
      return parent;
    }
    let package = match self.snapshot.derivation(id)?.membership {
      Membership::Unit(package) => package,
      Membership::Outside | Membership::Package => return None,
    };

    let parent = self
      .consumers(id, package)
      .into_iter()
      .map(|consumer| (consumer, self.depth(consumer, package)))
      .fold(None, |best, (consumer, depth)| {
        match best {
          Some((_, best_depth)) if best_depth >= depth => best,
          _ => Some((consumer, depth)),
        }
      })
      .map(|(consumer, _)| consumer);
    self.parents.borrow_mut().insert(id, parent);
    parent
  }

  fn consumers(
    &self,
    id: DerivationId,
    package: DerivationId,
  ) -> Vec<DerivationId> {
    self.snapshot.derivation(id).map_or_else(Vec::new, |info| {
      info
        .derivation_parents
        .iter()
        .copied()
        .filter(|&parent| {
          !self.staged.contains(&(package, parent))
            && self.snapshot.derivation(parent).is_some_and(|parent| {
              parent.membership == Membership::Unit(package)
            })
        })
        .collect()
    })
  }

  fn depth(&self, root: DerivationId, package: DerivationId) -> usize {
    let mut visiting = HashSet::new();
    let mut stack = vec![(root, false)];
    while let Some((current, expanded)) = stack.pop() {
      if self.depths.borrow().contains_key(&current)
        || (!expanded && !visiting.insert(current))
      {
        continue;
      }
      let consumers = self.consumers(current, package);
      if expanded {
        let depths = self.depths.borrow();
        let depth = consumers
          .iter()
          .map(|consumer| depths.get(consumer).copied().unwrap_or(0))
          .max()
          .map_or(1, |deepest| deepest + 1);
        drop(depths);
        self.depths.borrow_mut().insert(current, depth);
        continue;
      }

      stack.push((current, true));
      let depths = self.depths.borrow();
      stack.extend(
        consumers
          .into_iter()
          .filter(|consumer| !depths.contains_key(consumer))
          .map(|consumer| (consumer, false)),
      );
    }
    self.depths.borrow().get(&root).copied().unwrap_or(0)
  }
}

#[expect(
  clippy::multiple_inherent_impl,
  reason = "rendering is split across modules by section"
)]
impl Renderer<'_> {
  /// Builds one typed row graph.
  ///
  /// Selection and rendering operate on these same rows, so transfer placement
  /// cannot be lost in parallel index maps.
  pub(super) fn tree_plan(&self) -> TreePlan {
    let slots = self
      .snapshot
      .derivations
      .keys()
      .max()
      .map_or(0, |id| id + 1);
    let mut relevance = vec![0_u8; slots];
    let mut pending = VecDeque::new();

    for (&id, info) in self.snapshot.derivations {
      let priority = match info.build_status {
        BuildStatus::Failed { .. } => 4,
        BuildStatus::Building(_) => 3,
        // Dependency failures are useful context, but must not displace
        // builds that are still running.
        BuildStatus::DependencyFailed | BuildStatus::Planned => 2,
        BuildStatus::Available => 1,
        BuildStatus::Unknown | BuildStatus::Built { .. } => 0,
      };
      let units = self.snapshot.packages.get(&id);
      relevance[id] = match info.membership {
        Membership::Unit(_)
          if priority < 3
            && !matches!(info.build_status, BuildStatus::DependencyFailed) =>
        {
          0
        },
        Membership::Package
          if units.is_some_and(|counts| counts.running > 0) =>
        {
          priority.max(3)
        },
        Membership::Package
          if units.is_some_and(|counts| counts.failed > 0) =>
        {
          priority.max(2)
        },
        Membership::Outside | Membership::Package | Membership::Unit(_) => {
          priority
        },
      };
    }
    // Finished transfers count toward their group's bar, not rows.
    for item in &self.snapshot.placed_transfers {
      if let Some(id) = item.placement.derivation()
        && !item.transfer.completed
      {
        relevance[id] = relevance[id].max(3);
      }
    }
    pending.extend(
      relevance
        .iter()
        .enumerate()
        .filter_map(|(id, &priority)| (priority > 0).then_some(id)),
    );
    let active = relevance
      .iter()
      .map(|&priority| priority >= 3)
      .collect::<Vec<bool>>();

    let unit_parents = self.unit_parents();
    while let Some(id) = pending.pop_front() {
      let priority = relevance[id];
      for parent in self.row_parents(id, &unit_parents) {
        // Nix announces everything it will build, so an unannounced or
        // settled parent never waits on its inputs and only active work keeps
        // it on the path. Units are the exception, since they only appear once
        // their producer runs.
        let settled =
          self.snapshot.derivation(parent).is_some_and(|derivation| {
            match derivation.build_status {
              BuildStatus::Unknown => {
                !matches!(derivation.membership, Membership::Unit(_))
              },
              BuildStatus::Planned => {
                self.snapshot.unconfirmed.contains(&parent)
              },
              BuildStatus::Available
              | BuildStatus::Built { .. }
              | BuildStatus::DependencyFailed => true,
              BuildStatus::Building(_) | BuildStatus::Failed { .. } => false,
            }
          });
        if priority < 3 && settled {
          continue;
        }
        let previous = relevance[parent];
        if priority > previous {
          relevance[parent] = priority;
          pending.push_back(parent);
        }
      }
    }

    let mut pruned = true;
    while pruned {
      pruned = false;
      for (&id, info) in self.snapshot.derivations {
        let orphan = relevance[id] == 1
          && matches!(info.build_status, BuildStatus::Available)
          && self
            .row_parents(id, &unit_parents)
            .into_iter()
            .all(|parent| relevance[parent] == 0);
        if orphan {
          relevance[id] = 0;
          pruned = true;
        }
      }
    }

    let mut plan = TreePlan::default();
    for (id, &priority) in relevance.iter().enumerate() {
      if priority == 0 {
        continue;
      }
      let mut row = PlannedRow::new(priority);
      row.active = active[id];
      plan.rows.insert(RowId::Build(id), row);
    }
    for id in (0..slots).filter(|&id| active[id]) {
      let mut seen = HashSet::new();
      let mut above = vec![id];
      while let Some(current) = above.pop() {
        for parent in self.row_parents(current, &unit_parents) {
          if seen.insert(parent)
            && let Some(row) = plan.rows.get_mut(&RowId::Build(parent))
          {
            row.active_below += 1;
            above.push(parent);
          }
        }
      }
    }
    for (id, &priority) in relevance.iter().enumerate() {
      if priority == 0 {
        continue;
      }
      let Some(info) = self.snapshot.derivation(id) else {
        continue;
      };
      let children = info
        .input_derivations
        .iter()
        .filter(|&&child| {
          self.snapshot.derivation(child).is_none_or(|child| {
            !matches!(child.membership, Membership::Unit(_))
          })
        })
        .map(|&child| RowId::Build(child))
        .filter(|child| plan.rows.contains_key(child))
        .collect::<Vec<RowId>>();
      plan.row_mut(RowId::Build(id)).children.extend(children);
    }
    for (id, &priority) in relevance.iter().enumerate() {
      if priority == 0 {
        continue;
      }
      let Some(info) = self.snapshot.derivation(id) else {
        continue;
      };
      let hosts = match info.membership {
        Membership::Unit(package) => {
          vec![unit_parents.get(id).unwrap_or(package)]
        },
        Membership::Outside | Membership::Package => {
          info
            .derivation_parents
            .iter()
            .filter_map(|&parent| {
              match self.snapshot.derivation(parent)?.membership {
                Membership::Unit(package) => Some(package),
                Membership::Outside | Membership::Package => None,
              }
            })
            .collect::<Vec<DerivationId>>()
        },
      };
      for host in hosts {
        if host != id
          && let Some(row) = plan.rows.get_mut(&RowId::Build(host))
          && !row.children.contains(&RowId::Build(id))
        {
          row.children.push(RowId::Build(id));
        }
      }
    }

    let shown = self.shown_unmatched();
    for (index, item) in self.snapshot.placed_transfers.iter().enumerate() {
      let priority = 3;
      if item.transfer.completed {
        continue;
      }
      match item.placement {
        TransferPlacement::Inline(id) => {
          if let Some(row) = plan.rows.get_mut(&RowId::Build(id)) {
            row.inline.push(index);
          }
        },
        TransferPlacement::Source {
          consumer,
          consumer_count,
        } => {
          if !plan.rows.contains_key(&RowId::Build(consumer)) {
            plan.hidden += 1;
            continue;
          }
          let id = RowId::Transfer(index);
          let mut row = PlannedRow::new(priority);
          row.consumers = consumer_count;
          plan.rows.insert(id, row);
          plan.row_mut(RowId::Build(consumer)).children.push(id);
        },
        TransferPlacement::Unmatched => {
          if elapsed(item.transfer.start, self.now) < TRANSFER_DISPLAY_DELAY {
            continue;
          }
          plan
            .rows
            .entry(RowId::TransferGroup)
            .or_insert_with(|| PlannedRow::new(priority));
          let id = if shown.contains(&index) {
            RowId::Transfer(index)
          } else if item.transfer.total.is_none() {
            plan.unknown_size.push(index);
            RowId::Unsized(item.transfer.direction)
          } else {
            continue;
          };
          if plan.rows.contains_key(&id) {
            continue;
          }
          plan.rows.insert(id, PlannedRow::new(priority));
          plan.row_mut(RowId::TransferGroup).children.push(id);
        },
      }
    }
    // Keep a group for planned transfers even before the first one starts.
    if !plan.rows.contains_key(&RowId::TransferGroup)
      && (self.snapshot.placed_transfers.iter().any(|item| {
        !item.transfer.completed
          && matches!(item.placement, TransferPlacement::Unmatched)
          && elapsed(item.transfer.start, self.now) >= TRANSFER_DISPLAY_DELAY
      }) || !self.snapshot.summary.planned_downloads.is_empty())
      && !self.snapshot.finished
    {
      plan.rows.insert(RowId::TransferGroup, PlannedRow::new(1));
    }

    for index in 0..self.snapshot.source_fetches.len() {
      let id = RowId::SourceFetch(index);
      plan.rows.insert(id, PlannedRow::new(3));
      let group = plan
        .rows
        .entry(RowId::TransferGroup)
        .or_insert_with(|| PlannedRow::new(3));
      group.priority = 3;
      group.active = true;
      group.children.push(id);
    }

    // A producer stands for the package it wrote.
    let mut folded = HashSet::new();
    for (&id, info) in self.snapshot.derivations {
      let Some(written) = info.writes else {
        continue;
      };
      let (producer, package) = (RowId::Build(id), RowId::Build(written));
      if !plan
        .rows
        .get(&package)
        .is_some_and(|row| row.children.contains(&producer))
      {
        continue;
      }
      let Some(row) = plan.rows.remove(&producer) else {
        continue;
      };

      let parent = plan.row_mut(package);
      parent.children.retain(|child| *child != producer);
      for child in row.children {
        if !parent.children.contains(&child) {
          parent.children.push(child);
        }
      }
      parent.inline.extend(row.inline);
      parent.priority = parent.priority.max(row.priority);
      parent.active |= row.active;
      parent.producer = Some(id);
      folded.insert(id);
    }

    plan.roots = (0..slots)
      .filter(|&id| plan.rows.contains_key(&RowId::Build(id)))
      .filter(|&id| {
        !self
          .row_parents(id, &unit_parents)
          .into_iter()
          .any(|parent| {
            folded.contains(&parent)
              || plan.rows.contains_key(&RowId::Build(parent))
          })
      })
      .map(RowId::Build)
      .collect();
    let rows = &plan.rows;
    plan.roots.sort_by_key(|root| {
      (
        Reverse(rows[root].priority),
        Reverse(rows[root].active_below),
        Reverse(rows[root].children.len()),
      )
    });
    if plan.rows.contains_key(&RowId::TransferGroup) {
      plan.roots.push(RowId::TransferGroup);
    }
    plan
  }

  /// Picks the unmatched downloads that get rows of their own: every stalled
  /// one and the largest that is moving. The group's bar covers the rest.
  fn shown_unmatched(&self) -> HashSet<usize> {
    let active =
      self
        .snapshot
        .placed_transfers
        .iter()
        .enumerate()
        .filter(|(_, item)| {
          !item.transfer.completed
            && matches!(item.placement, TransferPlacement::Unmatched)
            && elapsed(item.transfer.start, self.now) >= TRANSFER_DISPLAY_DELAY
        });
    let mut shown = active
      .clone()
      .filter(|(_, item)| item.transfer.idle >= STALL_AFTER)
      .map(|(index, _)| index)
      .collect::<HashSet<usize>>();
    shown.extend(
      active
        .filter(|(index, item)| {
          !shown.contains(index) && item.transfer.total.is_some()
        })
        .max_by_key(|(index, item)| (item.transfer.total, Reverse(*index)))
        .map(|(index, _)| index),
    );
    shown
  }

  /// Returns the derivations whose rows a derivation's row hangs from.
  ///
  /// A unit hangs from the unit [`Self::unit_parents`] places it under, or
  /// else its package, and anything else hangs from the package of a unit it
  /// feeds.
  fn row_parents(
    &self,
    id: DerivationId,
    unit_parents: &UnitParents<'_, '_>,
  ) -> Vec<DerivationId> {
    let Some(info) = self.snapshot.derivation(id) else {
      return Vec::new();
    };
    if let Membership::Unit(package) = info.membership {
      return vec![unit_parents.get(id).unwrap_or(package)];
    }
    let mut parents = Vec::new();
    for &parent in &info.derivation_parents {
      let host =
        match self.snapshot.derivation(parent).map(|info| info.membership) {
          Some(Membership::Unit(package)) => package,
          Some(Membership::Outside | Membership::Package) | None => parent,
        };
      if host != id && !parents.contains(&host) {
        parents.push(host);
      }
    }
    parents
  }

  fn unit_parents(&self) -> UnitParents<'_, '_> {
    let staged = self
      .snapshot
      .derivations
      .values()
      .filter_map(|info| {
        match info.membership {
          Membership::Unit(package) => Some(package),
          Membership::Outside | Membership::Package => None,
        }
      })
      .collect::<HashSet<DerivationId>>()
      .into_iter()
      .filter_map(|package| Some((package, self.snapshot.derivation(package)?)))
      .flat_map(|(package, info)| {
        info
          .input_derivations
          .iter()
          .map(move |&unit| (package, unit))
      })
      .collect::<HashSet<(DerivationId, DerivationId)>>();
    UnitParents {
      snapshot: &self.snapshot,
      staged,
      depths: RefCell::new(HashMap::new()),
      parents: RefCell::new(HashMap::new()),
    }
  }

  pub(super) fn render_tree(
    &mut self,
    plan: &TreePlan,
    maximum: usize,
  ) -> Vec<Line<'static>> {
    if maximum == 0 {
      return Vec::new();
    }

    self.seen.clear();
    let mut truncated = plan.line_count() > maximum || plan.hidden > 0;
    let mut content_limit = if truncated && maximum > 1 {
      maximum - 1
    } else {
      maximum
    };
    let mut selection = self.select_rows(plan, content_limit);
    if !truncated && selection.rows.len() + 1 < plan.line_count() {
      truncated = true;
      if maximum > 1 {
        content_limit = maximum - 1;
        selection = self.select_rows(plan, content_limit);
      }
    }
    // The status box below continues the tree's line, so no row closes it.
    let mut lines = vec![Row {
      lead:   vec![span("╭─ ", self.config.theme.connector)],
      name:   "Builds".to_owned(),
      color:  self.config.theme.text,
      bar:    None,
      status: Vec::new(),
    }];
    for &root in &selection.roots {
      self.push_row(&mut lines, root, &[], false, &selection);
      if lines.len() >= content_limit {
        break;
      }
    }
    if truncated && maximum > 1 {
      let hidden = plan.line_count().saturating_sub(lines.len());
      let active = plan
        .rows
        .iter()
        .filter(|(id, row)| row.active && !selection.rows.contains(id))
        .count()
        .min(hidden);
      let label = match (active, hidden - active) {
        (0, rest) => format!("… {rest} more"),
        (running, 0) => format!("… {running} active"),
        (running, rest) => format!("… {running} active, {rest} more"),
      };
      lines.push(Row {
        lead:   vec![span("├─ ", self.config.theme.connector)],
        name:   label,
        color:  self.config.theme.muted,
        bar:    None,
        status: Vec::new(),
      });
    }
    self.lay_out(lines)
  }

  /// Lines rows up in columns: names, then bars, then status.
  ///
  /// Every bar starts in the same column, after the longest name that fits.
  /// Terminals too narrow for a useful bar keep the status after the name.
  fn lay_out(&self, rows: Vec<Row>) -> Vec<Line<'static>> {
    let width = usize::from(self.width);
    // A fixed column keeps bars still while rows come and go.
    let column = (width * 2 / 5).min(NAME_WIDTH);
    let bar_width = width
      .saturating_sub(column + 1 + STATUS_WIDTH)
      .min(BAR_WIDTH);
    let bars =
      rows.iter().any(|row| row.bar.is_some()) && bar_width >= MIN_BAR_WIDTH;

    rows
      .into_iter()
      .map(|row| {
        let mut spans = row.lead;
        let detailed = row.bar.is_some() || !row.status.is_empty();
        if !bars || !detailed || row.bar.is_none() {
          // Rows without bars use the reclaimed column for their status when
          // the bar column exists; narrow terminals still protect the name.
          let status = if bars {
            spans_width(&row.status)
          } else {
            spans_width(&row.status).min(STATUS_WIDTH / 2)
          };
          let room = width.saturating_sub(spans_width(&spans) + status);
          spans.push(span(truncate_middle(&row.name, room), row.color));
          spans.extend(row.status);
          return fit_line(spans, self.width);
        }
        let room = column.saturating_sub(spans_width(&spans));
        let name = truncate_middle(&row.name, room);
        let gap = room - name.width() + 1;
        spans.push(span(name, row.color));
        spans.push(Span::raw(" ".repeat(gap)));
        match row.bar {
          Some(bar) => spans.extend(self.bar(bar, bar_width)),
          None => spans.push(Span::raw(" ".repeat(bar_width))),
        }
        spans.extend(row.status);
        fit_line(spans, self.width)
      })
      .collect()
  }

  fn bar(&self, bar: Bar, width: usize) -> Vec<Span<'static>> {
    let theme = &self.config.theme;
    match bar {
      Bar::Progress { done, total, color } => {
        progress_bar(done, total.max(1), width, color, theme.progress_track)
      },
      Bar::Busy(color) => {
        let segment = (width / 5).max(2);
        let travel = width - segment;
        #[expect(
          clippy::cast_possible_truncation,
          clippy::cast_sign_loss,
          reason = "the frame index is clamped to zero"
        )]
        let step = (self.now.max(0.0) * 8.0) as usize % (travel * 2).max(1);
        let start = if step > travel {
          travel * 2 - step
        } else {
          step
        };
        vec![
          span("─".repeat(start), theme.progress_track),
          span("━".repeat(segment), color),
          span("─".repeat(travel - start), theme.progress_track),
        ]
      },
    }
  }

  /// Selects active build and group rows first, then shares the remaining rows
  /// fairly among transfer children.
  ///
  /// Completed grace rows never displace active work.
  fn select_rows<'a>(
    &self,
    plan: &'a TreePlan,
    maximum: usize,
  ) -> TreeSelection<'a> {
    let mut selection = TreeSelection {
      plan,
      rows: HashSet::new(),
      roots: Vec::new(),
      parents: HashMap::new(),
      maximum,
    };
    if maximum <= 1 {
      return selection;
    }
    let focus_active = plan.focus_active();
    let mut frontier = BinaryHeap::new();
    let mut sequence = 0_usize;
    for &root in &plan.roots {
      let row = plan.row(root);
      frontier.push((row.priority, row.active_below, Reverse(sequence), root));
      selection.parents.insert(root, None);
      sequence += 1;
    }

    let mut order = Vec::new();
    let mut candidates = 0;
    while let Some((_, _, _, id)) = frontier.pop() {
      order.push(id);
      let row = plan.row(id);
      candidates += usize::from(!focus_active || row.active);
      for &child in &row.children {
        if selection.parents.contains_key(&child) {
          continue;
        }
        selection.parents.insert(child, Some(id));
        match child {
          RowId::Build(_) => {
            let row = plan.row(child);
            frontier.push((
              row.priority,
              row.active_below,
              Reverse(sequence),
              child,
            ));
            sequence += 1;
          },
          RowId::Transfer(_)
          | RowId::SourceFetch(_)
          | RowId::TransferGroup
          | RowId::Unsized(_) => {},
        }
      }
      if candidates >= maximum - 1 {
        break;
      }
    }

    let mut remaining = maximum.saturating_sub(1); // Builds header
    // A row without its parent draws at the top level.
    for &id in order
      .iter()
      .filter(|id| !focus_active || plan.row(**id).active)
    {
      if selection.rows.contains(&id) {
        continue;
      }
      let mut chain = vec![id];
      let mut above = selection.parents[&id];
      while let Some(parent) =
        above.filter(|parent| !selection.rows.contains(parent))
      {
        chain.push(parent);
        above = selection.parents[&parent];
      }
      if chain.len() > remaining {
        continue;
      }
      remaining -= chain.len();
      selection.rows.extend(chain);
    }
    if focus_active {
      let mut ancestors = order
        .iter()
        .filter(|id| selection.rows.contains(id))
        .filter_map(|id| selection.parents[id])
        .collect::<VecDeque<RowId>>();
      while remaining > 0 {
        let Some(ancestor) = ancestors.pop_front() else {
          break;
        };
        if !selection.rows.insert(ancestor) {
          continue;
        }
        remaining -= 1;
        if let Some(parent) = selection.parents[&ancestor] {
          ancestors.push_back(parent);
        }
      }
    }

    let fill_transfers =
      |rows: &mut HashSet<RowId>, remaining: &mut usize, completed: bool| {
        let mut groups = order
          .iter()
          .filter(|id| rows.contains(id))
          .map(|&id| {
            plan.row(id).children.iter().copied().filter(move |child| {
              match child {
                RowId::Transfer(index) => {
                  self.snapshot.placed_transfers[*index].transfer.completed
                    == completed
                },
                RowId::SourceFetch(_) | RowId::Unsized(_) => !completed,
                RowId::Build(_) | RowId::TransferGroup => false,
              }
            })
          })
          .collect::<VecDeque<_>>();
        while *remaining > 0 {
          let Some(mut group) = groups.pop_front() else {
            break;
          };
          if let Some(id) = group.next() {
            rows.insert(id);
            *remaining -= 1;
            groups.push_back(group);
          }
        }
      };
    fill_transfers(&mut selection.rows, &mut remaining, false);
    let anchored = selection.rows.clone();
    let waiting = |id: RowId| !focus_active || plan.row(id).priority >= 2;

    let mut nearby = order
      .iter()
      .copied()
      .filter(|id| selection.rows.contains(id))
      .collect::<VecDeque<RowId>>();
    while remaining > 0
      && let Some(id) = nearby.pop_front()
    {
      let mut children = plan
        .row(id)
        .children
        .iter()
        .copied()
        .filter(|&child| matches!(child, RowId::Build(_)) && waiting(child))
        .filter(|child| selection.parents.get(child) == Some(&Some(id)))
        .collect::<Vec<RowId>>();
      children.sort_by_key(|child| {
        (Reverse(plan.row(*child).priority), !self.ready(*child))
      });
      for child in children.into_iter().take(remaining) {
        if selection.rows.insert(child) {
          remaining -= 1;
          nearby.push_back(child);
        }
      }
    }
    for &id in &order {
      if remaining == 0 {
        break;
      }
      let blocked_elsewhere = focus_active && plan.row(id).priority >= 3;
      let attached = selection.parents[&id]
        .is_none_or(|parent| selection.rows.contains(&parent));
      if waiting(id)
        && attached
        && !blocked_elsewhere
        && selection.rows.insert(id)
      {
        remaining -= 1;
      }
    }
    fill_transfers(&mut selection.rows, &mut remaining, true);

    selection.roots.extend(order.iter().copied().filter(|id| {
      selection.rows.contains(id)
        && selection.parents[id]
          .is_none_or(|parent| !selection.rows.contains(&parent))
    }));
    selection.roots.sort_by_key(|root| {
      (*root == RowId::TransferGroup, !anchored.contains(root))
    });
    selection
  }

  /// Returns whether a waiting build will start next.
  ///
  /// That is the case once all its inputs are done or running.
  fn ready(&self, id: RowId) -> bool {
    let RowId::Build(id) = id else {
      return false;
    };
    self.snapshot.derivation(id).is_some_and(|info| {
      info.input_derivations.iter().all(|&input| {
        self.snapshot.derivation(input).is_none_or(|derivation| {
          !matches!(derivation.build_status, BuildStatus::Planned)
        })
      })
    })
  }

  fn push_row(
    &mut self,
    rows: &mut Vec<Row>,
    id: RowId,
    ancestors: &[bool],
    last: bool,
    selection: &TreeSelection<'_>,
  ) {
    if rows.len() >= selection.maximum || !selection.rows.contains(&id) {
      return;
    }

    let prefix = self.prefix(ancestors, last);
    match id {
      RowId::Build(derivation) => {
        self.push_build(rows, derivation, ancestors, last, selection);
      },
      RowId::Transfer(index) => {
        let row = selection.plan.row(id);
        rows.push(self.transfer_row(
          prefix,
          &self.snapshot.placed_transfers[index].transfer,
          row.consumers,
        ));
      },
      RowId::Unsized(direction) => {
        let transfers = selection
          .plan
          .unknown_size
          .iter()
          .map(|&index| &self.snapshot.placed_transfers[index].transfer)
          .filter(|transfer| transfer.direction == direction)
          .collect::<Vec<&Transfer>>();
        let mut aggregate = aggregate_transfers(transfers.iter().copied());
        aggregate.name = transfers
          .iter()
          .map(|transfer| transfer.name.as_str())
          .collect::<Vec<&str>>()
          .join(", ");
        aggregate.total = None;
        rows.push(self.transfer_row(prefix, &aggregate, 0));
      },
      RowId::SourceFetch(index) => {
        rows.push(
          self.source_fetch_row(prefix, self.snapshot.source_fetches[index]),
        );
      },
      RowId::TransferGroup => {
        self.push_transfer_group(rows, ancestors, last, selection);
      },
    }
  }

  fn push_build(
    &mut self,
    rows: &mut Vec<Row>,
    id: DerivationId,
    ancestors: &[bool],
    last: bool,
    selection: &TreeSelection<'_>,
  ) {
    if !self.seen.insert(id) {
      return;
    }

    let Some(info) = self.snapshot.derivation(id) else {
      return;
    };

    let row = selection.plan.row(RowId::Build(id));

    let mut lead = self.prefix(ancestors, last);
    if selection.parents[&RowId::Build(id)]
      .is_some_and(|parent| !selection.rows.contains(&parent))
    {
      lead.push(span("… ", self.config.theme.muted));
    }

    let (icon, color, mut bar, mut status) =
      self.build_status(id, info, row, selection);
    lead.push(span(format!("{icon}{}", self.icons.gap), color));

    // Copies of the row's own outputs stand in for its build progress.
    if !row.inline.is_empty() {
      let aggregate = aggregate_transfers(
        row
          .inline
          .iter()
          .map(|&index| &self.snapshot.placed_transfers[index].transfer),
      );
      bar = Some(self.transfer_bar(&aggregate));
      status = self.transfer_status(&aggregate);
    }

    let children = self.visible_children(id, row, selection);
    status.extend(self.hidden_sources(row, &children));
    rows.push(Row {
      lead,
      name: self.row_name(info).to_owned(),
      color,
      bar,
      status,
    });
    self.push_children(rows, children, ancestors, last, selection);
  }

  /// Draws `children` one level below a row, stopping at the line limit.
  fn push_children(
    &mut self,
    rows: &mut Vec<Row>,
    children: Vec<RowId>,
    ancestors: &[bool],
    last: bool,
    selection: &TreeSelection<'_>,
  ) {
    let mut descendants = ancestors.to_vec();
    descendants.push(!last);
    let count = children.len();
    for (position, child) in children.into_iter().enumerate() {
      self.push_row(
        rows,
        child,
        &descendants,
        position + 1 == count,
        selection,
      );
      if rows.len() >= selection.maximum {
        break;
      }
    }
  }

  fn hidden_sources(
    &self,
    row: &PlannedRow,
    children: &[RowId],
  ) -> Option<Span<'static>> {
    let source_count = row
      .children
      .iter()
      .filter(|child| matches!(child, RowId::Transfer(_)))
      .count();
    let shown_sources = children
      .iter()
      .filter(|child| matches!(child, RowId::Transfer(_)))
      .count();
    let hidden_sources = source_count.saturating_sub(shown_sources);
    if hidden_sources == 0 {
      return None;
    }

    let plural = if hidden_sources == 1 { "" } else { "s" };
    Some(span(
      format!("  ({hidden_sources} source download{plural} hidden)"),
      self.config.theme.muted,
    ))
  }

  /// Returns the icon, color, bar and status for a build row.
  ///
  /// A running or failed producer stands in for the derivation it writes.
  fn build_status(
    &self,
    id: DerivationId,
    info: &DerivationInfo,
    row: &PlannedRow,
    selection: &TreeSelection<'_>,
  ) -> (&'static str, Color, Option<Bar>, Vec<Span<'static>>) {
    let theme = &self.config.theme;
    let units = self.snapshot.packages.get(&id);
    let unit_start = units
      .filter(|counts| counts.running > 0)
      .and_then(|counts| counts.start);
    let status = row
      .producer
      .and_then(|producer| self.snapshot.derivation(producer))
      .map(|producer| &producer.build_status)
      .filter(|status| {
        matches!(
          status,
          BuildStatus::Building(_) | BuildStatus::Failed { .. }
        )
      })
      .unwrap_or(&info.build_status);

    let (icon, color, bar, mut suffix) = match status {
      BuildStatus::Unknown | BuildStatus::Planned if unit_start.is_some() => {
        (self.icons.running, theme.running, None, Vec::new())
      },
      BuildStatus::Unknown
        if units.is_some()
          || row.active_below > 0
          || matches!(info.membership, Membership::Unit(_)) =>
      {
        (self.icons.planned, theme.planned, None, Vec::new())
      },
      BuildStatus::Unknown => {
        (self.icons.planned, theme.muted, None, Vec::new())
      },
      BuildStatus::Planned => {
        let suffix = info.blocked.map(|blocker| {
          let label = match blocker {
            Blocker::Lock => "  locked by another build",
            Blocker::Machine => "  waiting for a machine",
          };
          span(label, theme.muted)
        });
        let suffix = suffix.into_iter().collect();
        (self.icons.planned, theme.planned, None, suffix)
      },
      BuildStatus::Building(build) => {
        let suffix = self.running_suffix(id, info, build, selection);
        let bar = self.build_bar(info, build);
        (self.icons.running, theme.running, Some(bar), suffix)
      },
      BuildStatus::Built {
        info: build, end, ..
      } => {
        let took = (self.config.show_timers
          && end - build.start >= MIN_TIMER_SECONDS)
          .then(|| {
            span(
              format!("  {}", format_duration(end - build.start)),
              theme.muted,
            )
          })
          .into_iter()
          .collect();
        (self.icons.done, theme.completed, None, took)
      },
      BuildStatus::Available => {
        (self.icons.done, theme.completed, None, Vec::new())
      },
      BuildStatus::Failed { .. } => {
        let label = span("  failed", theme.failed);
        (self.icons.failed, theme.failed, None, vec![label])
      },
      BuildStatus::DependencyFailed => {
        let label = span("  dependency failed", theme.muted);
        (self.icons.failed, theme.muted, None, vec![label])
      },
    };

    if let Some(progress) = self.unit_progress(id) {
      suffix.insert(0, progress);
    }

    if info.name.name.strip_suffix(".drv").is_some()
      && matches!(info.membership, Membership::Unit(_))
    {
      suffix.insert(0, span("  (scan)", theme.muted));
    }

    (icon, color, bar, suffix)
  }

  /// A running build's time against its estimate, or a moving segment
  /// without one.
  #[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "elapsed time is clamped to zero and far below u64::MAX"
  )]
  fn build_bar(&self, info: &DerivationInfo, build: &BuildInfo) -> Bar {
    let color = self.config.theme.running;
    build
      .estimate
      .filter(|&estimate| {
        estimate > 0 && !self.snapshot.state.name_is_shared(&info.name.name)
      })
      .map_or(Bar::Busy(color), |estimate| {
        Bar::Progress {
          done: (elapsed(build.start, self.now).max(0.0) * 1000.0) as u64,
          total: estimate.saturating_mul(1000),
          color,
        }
      })
  }

  fn running_suffix(
    &self,
    id: DerivationId,
    info: &DerivationInfo,
    build: &BuildInfo,
    selection: &TreeSelection<'_>,
  ) -> Vec<Span<'static>> {
    let mut suffix = Vec::new();

    if let Some(phase) = &build.phase {
      suffix.push(span(format!("  ({phase})"), self.config.theme.muted));
    }

    let average = build
      .estimate
      .filter(|_| !self.snapshot.state.name_is_shared(&info.name.name));
    suffix.extend(self.clock(build.start, average));

    suffix.extend(build.cpu.map(|cpu| self.cpu(cpu)));

    if let Host::Remote(machine) = &build.host {
      suffix.push(span(format!(" @{machine}"), self.config.theme.host));
    }

    let waiting = self.hidden_waiting(id, selection);
    if waiting > 0 {
      suffix.push(span(
        format!("  (+{waiting} waiting)"),
        self.config.theme.planned,
      ));
    }

    suffix
  }

  /// Counts the planned derivations above `id`, transitively, that have no
  /// row of their own.
  fn hidden_waiting(
    &self,
    id: DerivationId,
    selection: &TreeSelection<'_>,
  ) -> usize {
    let mut seen = HashSet::new();
    let mut hidden = 0;
    let mut above = vec![id];

    while let Some(current) = above.pop() {
      let Some(node) = self.snapshot.derivation(current) else {
        continue;
      };

      for &parent in &node.derivation_parents {
        let waiting =
          self.snapshot.derivation(parent).is_some_and(|derivation| {
            matches!(derivation.build_status, BuildStatus::Planned)
          });
        if waiting && seen.insert(parent) {
          hidden +=
            usize::from(!selection.rows.contains(&RowId::Build(parent)));
          above.push(parent);
        }
      }
    }

    hidden
  }

  fn clock(&self, start: f64, average: Option<u64>) -> Option<Span<'static>> {
    let elapsed = elapsed(start, self.now);
    self
      .config
      .show_timers
      .then_some(())
      .filter(|()| elapsed >= MIN_TIMER_SECONDS)
      .map(|()| {
        span(
          format!(
            "  {}{}{}",
            self.icons.clock,
            self.icons.gap,
            format_timer(elapsed, average)
          ),
          self.config.theme.timer,
        )
      })
  }

  /// Drops a unit's package name from its own, which the row above shows.
  fn row_name<'info>(&self, info: &'info DerivationInfo) -> &'info str {
    let name = &info.name.name;
    let stem = name.strip_suffix(".drv").unwrap_or(name);
    let Membership::Unit(package) = info.membership else {
      return stem;
    };
    self.snapshot.derivation(package).map_or(stem, |package| {
      stem
        .strip_prefix(package.name.name.as_str())
        .and_then(|name| name.strip_prefix('-'))
        .unwrap_or(stem)
    })
  }

  /// Child rows drawn under `id`, sources first, then by priority.
  fn visible_children(
    &self,
    id: DerivationId,
    row: &PlannedRow,
    selection: &TreeSelection<'_>,
  ) -> Vec<RowId> {
    let mut children = row
      .children
      .iter()
      .copied()
      .filter(|child| selection.rows.contains(child))
      .filter(|child| selection.parents[child] == Some(RowId::Build(id)))
      .filter(|child| {
        !matches!(child, RowId::Build(child) if self.seen.contains(child))
      })
      .collect::<Vec<RowId>>();
    children.sort_by_key(|child| {
      (
        !matches!(child, RowId::Transfer(_)),
        Reverse(selection.plan.row(*child).priority),
      )
    });
    children
  }

  /// Returns how many of the units a unit waits on are done, like `(12/15)`.
  fn unit_progress(&self, id: DerivationId) -> Option<Span<'static>> {
    let info = self.snapshot.derivation(id)?;
    let Membership::Unit(package) = info.membership else {
      return None;
    };
    let (mut done, mut total) = (0, 0);
    for &input in &info.input_derivations {
      let Some(input) = self.snapshot.derivation(input) else {
        continue;
      };
      if input.membership != Membership::Unit(package) || input.writes.is_some()
      {
        continue;
      }
      total += 1;
      done += usize::from(matches!(
        input.build_status,
        BuildStatus::Built { .. } | BuildStatus::Available
      ));
    }
    if total < 2
      || done == total && matches!(info.build_status, BuildStatus::Building(_))
    {
      return None;
    }
    let state = match info.build_status {
      BuildStatus::Building(_) => ", running",
      BuildStatus::Failed { .. } => ", failed",
      BuildStatus::Unknown
      | BuildStatus::Planned
      | BuildStatus::Available
      | BuildStatus::Built { .. }
      | BuildStatus::DependencyFailed => "",
    };
    Some(span(
      format!("  ({done}/{total}{state})"),
      self.config.theme.muted,
    ))
  }

  /// Draws unmatched transfers as one quiet parent above selected details.
  fn push_transfer_group(
    &mut self,
    rows: &mut Vec<Row>,
    ancestors: &[bool],
    last: bool,
    selection: &TreeSelection<'_>,
  ) {
    let theme = &self.config.theme;
    let row = selection.plan.row(RowId::TransferGroup);
    let shown = row
      .children
      .iter()
      .copied()
      .filter(|child| selection.rows.contains(child))
      .collect::<Vec<RowId>>();

    let (mut downloads, mut uploads) = (false, false);
    let mut active = self.snapshot.source_fetches.len();
    for item in &self.snapshot.placed_transfers {
      if !matches!(item.placement, TransferPlacement::Unmatched) {
        continue;
      }
      downloads |= item.transfer.direction == Direction::Download;
      uploads |= item.transfer.direction == Direction::Upload;
      if !item.transfer.completed {
        active += 1;
      }
    }
    let waiting = self.snapshot.counts.downloads.waiting;

    let (icon, color, name) = if active == 0 && waiting > 0 {
      (self.icons.planned, theme.planned, "fetching")
    } else if uploads && !downloads && waiting == 0 {
      (self.icons.upload, theme.upload, "transfers")
    } else {
      (self.icons.download, theme.download, "fetching")
    };
    let mut lead = self.prefix(ancestors, last);
    lead.push(span(format!("{icon}{}", self.icons.gap), color));
    rows.push(Row {
      lead,
      name: name.to_owned(),
      color: theme.text,
      bar: None,
      status: Vec::new(),
    });

    self.push_children(rows, shown, ancestors, last, selection);
  }

  fn prefix(&self, ancestors: &[bool], last: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for &continues in ancestors {
      spans.push(span(
        if continues { "│  " } else { "   " },
        self.config.theme.connector,
      ));
    }
    spans.push(span(
      if last { "╰─ " } else { "├─ " },
      self.config.theme.connector,
    ));
    spans
  }

  fn transfer_row(
    &self,
    mut lead: Vec<Span<'static>>,
    transfer: &Transfer,
    consumers: usize,
  ) -> Row {
    let color = self.transfer_color(transfer);
    let arrow = match transfer.direction {
      Direction::Download => self.icons.download,
      Direction::Upload => self.icons.upload,
    };
    lead.push(span(format!("{arrow}{}", self.icons.gap), color));
    let mut status = self.transfer_status(transfer);
    if consumers > 1 {
      // Ahead of the host, which matters least.
      let at = status.len() - usize::from(Self::shows_host(transfer));
      status.insert(
        at,
        span(
          format!("  used by {consumers} builds"),
          self.config.theme.muted,
        ),
      );
    }
    Row {
      lead,
      name: transfer.name.clone(),
      color,
      bar: Some(self.transfer_bar(transfer)),
      status,
    }
  }

  fn source_fetch_row(
    &self,
    mut lead: Vec<Span<'static>>,
    fetch: &SourceFetch,
  ) -> Row {
    let color = self.config.theme.running;
    lead.push(span(
      format!("{}{}", self.icons.download, self.icons.gap),
      color,
    ));
    let name = Path::new(&fetch.source)
      .file_name()
      .and_then(|name| name.to_str())
      .unwrap_or(&fetch.source);
    Row {
      lead,
      name: name.to_owned(),
      color,
      bar: Some(Bar::Busy(color)),
      status: vec![span(
        format!(
          "  copying  {}",
          format_duration(elapsed(fetch.start, self.now))
        ),
        self.config.theme.muted,
      )],
    }
  }

  fn shows_host(transfer: &Transfer) -> bool {
    !transfer.host.is_empty() && transfer.host != "localhost"
  }

  fn stalled(transfer: &Transfer) -> bool {
    !transfer.completed && transfer.idle >= STALL_AFTER
  }

  fn transfer_bar(&self, transfer: &Transfer) -> Bar {
    let color = if Self::stalled(transfer) {
      self.config.theme.failed
    } else {
      self.transfer_color(transfer)
    };
    transfer.total.filter(|total| *total > 0).map_or(
      Bar::Busy(color),
      |total| {
        Bar::Progress {
          done: transfer.done,
          total,
          color,
        }
      },
    )
  }

  /// Percentage, stall, bytes and host, most important first so a narrow
  /// terminal cuts the rest.
  fn transfer_status(&self, transfer: &Transfer) -> Vec<Span<'static>> {
    let theme = &self.config.theme;
    let mut status = Vec::new();
    let total = transfer.total.filter(|total| *total > 0);
    if let Some(total) = total {
      let percent = (transfer.done.saturating_mul(100) / total).min(100);
      status.push(span(
        format!("  {percent:>3}%"),
        self.transfer_color(transfer),
      ));
    }
    if Self::stalled(transfer) {
      status.push(span(
        format!("  stalled {}", format_duration(transfer.idle)),
        theme.failed,
      ));
    }
    let bytes = total.map_or_else(
      || format_bytes(transfer.done),
      |total| {
        format!("{}/{}", format_bytes(transfer.done), format_bytes(total))
      },
    );
    status.push(span(format!("  {bytes}"), theme.muted));
    if Self::shows_host(transfer) {
      status.push(span(format!("  {}", transfer.host), theme.host));
    }
    status
  }

  pub(super) fn transfer_suffix(
    &self,
    transfer: &Transfer,
    occupied: usize,
  ) -> Vec<Span<'static>> {
    let color = self.transfer_color(transfer);
    let arrow = match transfer.direction {
      Direction::Download => self.icons.download,
      Direction::Upload => self.icons.upload,
    };
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
