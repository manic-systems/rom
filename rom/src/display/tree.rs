//! Dependency tree planning and rendering.

use std::{
  cell::RefCell,
  cmp::Reverse,
  collections::{BinaryHeap, HashMap, HashSet, VecDeque},
};

use cognos::Host;
use ratatui_core::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::{
  Direction,
  Renderer,
  Transfer,
  aggregate_transfers,
  fit_line,
  format_bytes,
  format_duration,
  format_secs,
  model::{RenderSnapshot, TransferPlacement, UnitCounts},
  progress_bar,
  span,
  spans_width,
  truncate_text,
};
use crate::state::{Blocker, BuildStatus, DerivationId, Membership, elapsed};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum RowId {
  Build(DerivationId),
  Transfer(usize),
  SourceFetch(usize),
  TransferGroup,
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
  rows:   HashMap<RowId, PlannedRow>,
  roots:  Vec<RowId>,
  hidden: usize,
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
        BuildStatus::Planned if self.snapshot.unconfirmed.contains(&id) => 0,
        BuildStatus::Planned => 2,
        BuildStatus::Available => 1,
        BuildStatus::Unknown
        | BuildStatus::Built { .. }
        | BuildStatus::DependencyFailed => 0,
      };
      let units = self.snapshot.packages.get(&id);
      let moved = units
        .is_some_and(|counts| counts.done + counts.running + counts.failed > 0);
      relevance[id] = match info.membership {
        Membership::Unit(_) if priority < 3 => 0,
        Membership::Package
          if units.is_some_and(|counts| counts.running > 0) =>
        {
          priority.max(3)
        },
        Membership::Package
          if !self.snapshot.finished
            && (moved
              || matches!(info.build_status, BuildStatus::Planned)
                && !self.snapshot.unconfirmed.contains(&id))
            && units.is_some_and(UnitCounts::unfinished) =>
        {
          priority.max(2)
        },
        Membership::Outside | Membership::Package | Membership::Unit(_) => {
          priority
        },
      };
    }
    for item in &self.snapshot.placed_transfers {
      if let Some(id) = item.placement.derivation() {
        let priority = if item.transfer.completed { 1 } else { 3 };
        relevance[id] = relevance[id].max(priority);
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
      let children: Vec<_> = info
        .input_derivations
        .iter()
        .filter(|&&child| {
          self.snapshot.derivation(child).is_none_or(|child| {
            !matches!(child.membership, Membership::Unit(_))
          })
        })
        .map(|&child| RowId::Build(child))
        .filter(|child| plan.rows.contains_key(child))
        .collect();
      plan.row_mut(RowId::Build(id)).children.extend(children);
    }
    for (id, &priority) in relevance.iter().enumerate() {
      if priority == 0 {
        continue;
      }
      let Some(info) = self.snapshot.derivation(id) else {
        continue;
      };
      let hosts: Vec<_> = match info.membership {
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
            .collect()
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

    for (index, item) in self.snapshot.placed_transfers.iter().enumerate() {
      let priority = if item.transfer.completed { 1 } else { 3 };
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
          let id = RowId::Transfer(index);
          plan.rows.insert(id, PlannedRow::new(priority));
          let group = plan
            .rows
            .entry(RowId::TransferGroup)
            .or_insert_with(|| PlannedRow::new(priority));
          group.priority = group.priority.max(priority);
          group.active |= priority >= 3;
          group.children.push(id);
        },
      }
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
    let mut lines = vec![Line::from(vec![
      span("\u{256d}\u{2500} ", self.config.theme.connector), // ╭─
      span("Builds", self.config.theme.text),
    ])];
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
      lines.push(Line::from(vec![
        span("\u{251c}\u{2500} ", self.config.theme.connector), // ├─
        span(label, self.config.theme.muted),
      ]));
    }
    lines
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
          RowId::Transfer(_) | RowId::SourceFetch(_) | RowId::TransferGroup => {
          },
        }
      }
      if candidates >= maximum - 1 {
        break;
      }
    }

    let mut remaining = maximum.saturating_sub(1); // Builds header
    selection.rows.extend(
      order
        .iter()
        .copied()
        .filter(|id| {
          let row = plan.row(*id);
          !focus_active || row.active
        })
        .take(remaining),
    );
    remaining -= selection.rows.len();
    if focus_active {
      let mut ancestors: VecDeque<_> = order
        .iter()
        .filter(|id| selection.rows.contains(id))
        .filter_map(|id| selection.parents[id])
        .collect();
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
        let mut groups: VecDeque<_> = order
          .iter()
          .filter(|id| rows.contains(id))
          .map(|&id| {
            plan.row(id).children.iter().copied().filter(move |child| {
              match child {
                RowId::Transfer(index) => {
                  self.snapshot.placed_transfers[*index].transfer.completed
                    == completed
                },
                RowId::SourceFetch(_) => !completed,
                RowId::Build(_) | RowId::TransferGroup => false,
              }
            })
          })
          .collect();
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
      let mut children: Vec<_> = plan
        .row(id)
        .children
        .iter()
        .copied()
        .filter(|&child| matches!(child, RowId::Build(_)) && waiting(child))
        .filter(|child| selection.parents.get(child) == Some(&Some(id)))
        .collect();
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
    selection.roots.sort_by_key(|root| !anchored.contains(root));
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
    lines: &mut Vec<Line<'static>>,
    id: RowId,
    ancestors: &[bool],
    last: bool,
    selection: &TreeSelection<'_>,
  ) {
    if lines.len() >= selection.maximum || !selection.rows.contains(&id) {
      return;
    }
    match id {
      RowId::Build(derivation) => {
        self.push_build(lines, derivation, ancestors, last, selection);
      },
      RowId::Transfer(index) => {
        let row = selection.plan.row(id);
        self.push_transfer_row(
          lines,
          ancestors,
          last,
          &self.snapshot.placed_transfers[index].transfer,
          row.consumers,
        );
      },
      RowId::SourceFetch(index) => {
        let prefix = self.prefix(ancestors, last);
        lines.push(
          self.source_fetch_line(prefix, self.snapshot.source_fetches[index]),
        );
      },
      RowId::TransferGroup => {
        self.push_transfer_group(lines, ancestors, last, selection);
      },
    }
  }

  fn push_build(
    &mut self,
    lines: &mut Vec<Line<'static>>,
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

    let mut spans = self.prefix(ancestors, last);
    if selection.parents[&RowId::Build(id)]
      .is_some_and(|parent| !selection.rows.contains(&parent))
    {
      spans.push(span("\u{2026} ", self.config.theme.muted)); // …
    }
    let units = self
      .snapshot
      .packages
      .get(&id)
      .filter(|counts| counts.unfinished() || counts.failed > 0);
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
    let (icon, color, mut suffix) = match status {
      BuildStatus::Unknown | BuildStatus::Planned if unit_start.is_some() => {
        let mut suffix = Vec::new();
        if self.config.show_timers
          && let Some(start) = unit_start
        {
          suffix.push(span(
            format!(
              "  {}{}{}",
              self.icons.clock,
              self.icons.gap,
              format_duration(elapsed(start, self.now))
            ),
            self.config.theme.timer,
          ));
        }
        (self.icons.running, self.config.theme.running, suffix)
      },
      BuildStatus::Unknown
        if units.is_some()
          || row.active_below > 0
          || matches!(info.membership, Membership::Unit(_)) =>
      {
        (self.icons.planned, self.config.theme.planned, Vec::new())
      },
      BuildStatus::Unknown => {
        (self.icons.planned, self.config.theme.muted, Vec::new())
      },
      BuildStatus::Planned => {
        let suffix = info.blocked.map(|blocker| {
          let label = match blocker {
            Blocker::Lock => "  (locked by another build)",
            Blocker::Machine => "  (waiting for a machine)",
          };
          span(label, self.config.theme.muted)
        });
        (
          self.icons.planned,
          self.config.theme.planned,
          suffix.into_iter().collect(),
        )
      },
      BuildStatus::Building(build) => {
        let mut suffix = Vec::new();
        if let Some(phase) = &build.phase {
          suffix.push(span(format!("  ({phase})"), self.config.theme.muted));
        }
        if self.config.show_timers {
          suffix.push(span(
            format!(
              "  {}{}{}",
              self.icons.clock,
              self.icons.gap,
              format_duration(elapsed(build.start, self.now))
            ),
            self.config.theme.timer,
          ));
        }
        if let Host::Remote(machine) = &build.host {
          let label = format!(" @{machine}");
          suffix.push(span(label, self.config.theme.host));
        }
        if let Some(estimate) = build.estimate
          && !self.snapshot.shared_names.contains(info.name.name.as_str())
        {
          suffix.push(span(
            format!(
              "  ({}{}{})",
              self.icons.estimate,
              self.icons.gap,
              format_secs(estimate)
            ),
            self.config.theme.timer,
          ));
        }
        let mut seen = HashSet::new();
        let mut above = vec![id];
        let mut unseen = 0;
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
              unseen +=
                usize::from(!selection.rows.contains(&RowId::Build(parent)));
              above.push(parent);
            }
          }
        }
        if unseen > 0 {
          suffix.push(span(
            format!("  (+{unseen} waiting)"),
            self.config.theme.planned,
          ));
        }
        (self.icons.running, self.config.theme.running, suffix)
      },
      BuildStatus::Built { .. } | BuildStatus::Available => {
        (self.icons.done, self.config.theme.completed, Vec::new())
      },
      BuildStatus::Failed { .. } => {
        (self.icons.failed, self.config.theme.failed, Vec::new())
      },
      BuildStatus::DependencyFailed => {
        (self.icons.failed, self.config.theme.muted, Vec::new())
      },
    };
    if let Some(counts) = units
      && !matches!(
        info.build_status,
        BuildStatus::Built { .. } | BuildStatus::Available
      )
    {
      suffix.insert(0, self.unit_summary(counts));
    }
    if let Some(progress) = self.unit_progress(id) {
      suffix.insert(0, progress);
    }
    let stripped = info.name.name.strip_suffix(".drv");
    if stripped.is_some() && matches!(info.membership, Membership::Unit(_)) {
      suffix.insert(0, span("  (scan)", self.config.theme.muted));
    }
    spans.push(span(format!("{icon}{}", self.icons.gap), color));
    let transfer = (!row.inline.is_empty()).then(|| {
      aggregate_transfers(
        row
          .inline
          .iter()
          .map(|&index| &self.snapshot.placed_transfers[index].transfer),
      )
    });
    let has_transfer = transfer.is_some();
    let reserve = transfer.as_ref().map_or_else(
      || spans_width(&suffix),
      |item| if item.total.is_some() { 12 } else { 8 },
    );
    let name_width =
      usize::from(self.width).saturating_sub(spans_width(&spans) + reserve);
    let stem = stripped.unwrap_or(&info.name.name);
    let name = match info.membership {
      Membership::Unit(package) => {
        self.snapshot.derivation(package).map_or(stem, |package| {
          stem
            .strip_prefix(package.name.name.as_str())
            .and_then(|name| name.strip_prefix('-'))
            .unwrap_or(stem)
        })
      },
      Membership::Outside | Membership::Package => stem,
    };
    spans.push(span(truncate_text(name, name_width), color));
    if let Some(aggregate) = transfer {
      spans.extend(self.transfer_suffix(&aggregate, spans_width(&spans)));
    }
    if !has_transfer {
      spans.extend(suffix);
    }

    let mut children: Vec<_> = row
      .children
      .iter()
      .copied()
      .filter(|child| selection.rows.contains(child))
      .filter(|child| selection.parents[child] == Some(RowId::Build(id)))
      .filter(|child| {
        !matches!(child, RowId::Build(child) if self.seen.contains(child))
      })
      .collect();
    children.sort_by_key(|child| {
      (
        !matches!(child, RowId::Transfer(_)),
        Reverse(selection.plan.row(*child).priority),
      )
    });
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
    if hidden_sources > 0 {
      let plural = if hidden_sources == 1 { "" } else { "s" };
      spans.push(span(
        format!("  ({hidden_sources} source download{plural} hidden)"),
        self.config.theme.muted,
      ));
    }
    lines.push(fit_line(spans, self.width));

    let mut descendants = ancestors.to_vec();
    descendants.push(!last);
    let count = children.len();
    for (position, child) in children.into_iter().enumerate() {
      self.push_row(
        lines,
        child,
        &descendants,
        position + 1 == count,
        selection,
      );
      if lines.len() >= selection.maximum {
        break;
      }
    }
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

  fn unit_summary(&self, counts: &UnitCounts<'_>) -> Span<'static> {
    let mut parts = vec![format!("units {}/{}", counts.done, counts.total)];
    if counts.running > 0 {
      let mut remote = counts
        .remote
        .iter()
        .map(|(&machine, &count)| (machine, count))
        .collect::<Vec<(&str, usize)>>();
      remote.sort_by_key(|&(machine, count)| (Reverse(count), machine));
      let machines = remote
        .iter()
        .map(|(machine, count)| format!("{count} @{machine}"))
        .collect::<Vec<String>>();
      parts.push(if machines.is_empty() {
        format!("{} running", counts.running)
      } else {
        format!("{} running ({})", counts.running, machines.join(", "))
      });
    }
    if counts.failed > 0 {
      parts.push(format!("{} failed", counts.failed));
    }
    span(format!("  ({})", parts.join(", ")), self.config.theme.muted)
  }

  fn push_transfer_group(
    &mut self,
    lines: &mut Vec<Line<'static>>,
    ancestors: &[bool],
    last: bool,
    selection: &TreeSelection<'_>,
  ) {
    let row = selection.plan.row(RowId::TransferGroup);
    let shown: Vec<_> = row
      .children
      .iter()
      .copied()
      .filter(|child| selection.rows.contains(child))
      .collect();
    let hidden = row.children.len() - shown.len();
    let label = if hidden == 0 {
      "Transfers".to_owned()
    } else {
      format!("Transfers ({hidden} hidden)")
    };
    let mut spans = self.prefix(ancestors, last);
    spans.push(span(label, self.config.theme.text));
    lines.push(Line::from(spans));

    let mut descendants = ancestors.to_vec();
    descendants.push(!last);
    let count = shown.len();
    for (position, child) in shown.into_iter().enumerate() {
      self.push_row(
        lines,
        child,
        &descendants,
        position + 1 == count,
        selection,
      );
    }
  }

  fn prefix(&self, ancestors: &[bool], last: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for &continues in ancestors {
      spans.push(span(
        if continues { "\u{2502}  " } else { "   " }, // │
        self.config.theme.connector,
      ));
    }
    spans.push(span(
      if last {
        "\u{2570}\u{2500} " // ╰─
      } else {
        "\u{251c}\u{2500} " // ├─
      },
      self.config.theme.connector,
    ));
    spans
  }

  fn push_transfer_row(
    &self,
    lines: &mut Vec<Line<'static>>,
    ancestors: &[bool],
    last: bool,
    transfer: &Transfer,
    consumers: usize,
  ) {
    let mut spans = self.prefix(ancestors, last);
    let available =
      usize::from(self.width).saturating_sub(spans_width(&spans) + 12);
    let shared = if consumers > 1 {
      let label = format!(" (used by {consumers} builds)");
      if label.width() + 8 <= available {
        label
      } else {
        String::new()
      }
    } else {
      String::new()
    };
    let name =
      truncate_text(&transfer.name, available.saturating_sub(shared.width()));
    spans.push(span(
      format!("{name}{shared}"),
      self.transfer_color(transfer),
    ));
    spans.extend(self.transfer_suffix(transfer, spans_width(&spans)));
    lines.push(fit_line(spans, self.width));
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
