use std::{
  cmp::Reverse,
  collections::{BinaryHeap, HashMap, HashSet, VecDeque},
};

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
  model::TransferPlacement,
  progress_bar,
  spans_width,
  truncate_text,
};
use crate::state::{BuildStatus, DerivationId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum RowId {
  Build(DerivationId),
  Transfer(usize),
  TransferGroup,
}

struct PlannedRow {
  priority:     u8,
  active:       bool,
  active_below: usize,
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

  fn row(&self, id: RowId) -> &PlannedRow {
    &self.rows[&id]
  }
}

struct TreeSelection<'a> {
  plan:    &'a TreePlan,
  rows:    HashSet<RowId>,
  roots:   Vec<RowId>,
  parents: HashMap<RowId, Option<RowId>>,
  maximum: usize,
}

impl Renderer<'_> {
  /// Build one typed row graph. Selection and rendering operate on these same
  /// rows, so transfer placement cannot be lost in parallel index maps.
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
        BuildStatus::Planned => 2,
        BuildStatus::Available => 1,
        BuildStatus::Unknown
        | BuildStatus::Built { .. }
        | BuildStatus::DependencyFailed => 0,
      };
      relevance[id] = priority;
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

    // Reverse reachability computes the connected ancestor set in O(V + E).
    while let Some(id) = pending.pop_front() {
      let priority = relevance[id];
      let Some(info) = self.snapshot.derivation(id) else {
        continue;
      };
      for &parent in &info.derivation_parents {
        // Nix announces everything it will build, so an unannounced or
        // settled parent never waits on its inputs and only active work keeps
        // it on the path.
        let settled =
          self.snapshot.derivation(parent).is_some_and(|derivation| {
            matches!(
              derivation.build_status,
              BuildStatus::Unknown
                | BuildStatus::Available
                | BuildStatus::Built { .. }
                | BuildStatus::DependencyFailed
            )
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
        let Some(info) = self.snapshot.derivation(current) else {
          continue;
        };
        for &parent in &info.derivation_parents {
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
        .map(|&child| RowId::Build(child))
        .filter(|child| plan.rows.contains_key(child))
        .collect();
      plan.row_mut(RowId::Build(id)).children.extend(children);
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

    plan.roots = (0..slots)
      .filter(|&id| plan.rows.contains_key(&RowId::Build(id)))
      .filter(|&id| {
        self.snapshot.derivation(id).is_none_or(|info| {
          !info
            .derivation_parents
            .iter()
            .any(|&parent| plan.rows.contains_key(&RowId::Build(parent)))
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
    let mut lines = vec![Line::from(vec![
      self.span("┏━ ", self.config.theme.connector),
      self.span("Builds", self.config.theme.text),
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
        self.span("┣━ ", self.config.theme.connector),
        self.span(label, self.config.theme.muted),
      ]));
    }
    lines
  }

  /// Select active build/group rows first, then share remaining rows fairly
  /// among transfer children. Completed grace rows never displace active work.
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
    let focus_active = self.focus_active(plan);
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
          RowId::Transfer(_) | RowId::TransferGroup => {},
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
        .filter(|id| !focus_active || plan.row(*id).active)
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
            plan.row(id).children.iter().copied().filter(|child| {
              let RowId::Transfer(index) = child else {
                return false;
              };
              self.snapshot.placed_transfers[*index].transfer.completed
                == completed
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

  /// A waiting build whose inputs are all done or running starts next.
  fn ready(&self, id: RowId) -> bool {
    let RowId::Build(id) = id else {
      return false;
    };
    self.snapshot.derivation(id).is_some_and(|info| {
      info.input_derivations.iter().all(|&input| {
        self.snapshot.derivation(input).is_none_or(|input| {
          !matches!(
            input.build_status,
            BuildStatus::Planned | BuildStatus::Unknown
          )
        })
      })
    })
  }

  fn focus_active(&self, plan: &TreePlan) -> bool {
    plan.rows.values().any(|row| row.active)
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
      spans.push(self.span("… ", self.config.theme.muted));
    }
    let (icon, color, suffix) = match &info.build_status {
      BuildStatus::Unknown => {
        (self.icons.planned, self.config.theme.muted, None)
      },
      BuildStatus::Planned => {
        (self.icons.planned, self.config.theme.planned, None)
      },
      BuildStatus::Building(build) => {
        let mut suffix = build
          .phase
          .as_ref()
          .map(|phase| format!("  ({phase})"))
          .unwrap_or_default();
        if self.config.show_timers {
          suffix.push_str(&format!(
            "  {} {}",
            self.icons.clock,
            format_duration(self.now - build.start)
          ));
        }
        if let Some(estimate) = build.estimate
          && !self.snapshot.shared_names.contains(info.name.name.as_str())
        {
          suffix.push_str(&format!(
            "  ({} {})",
            self.icons.estimate,
            format_duration(estimate as f64)
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
          suffix.push_str(&format!("  (+{unseen} waiting)"));
        }
        (
          self.icons.running,
          self.config.theme.running,
          (!suffix.is_empty()).then_some(suffix),
        )
      },
      BuildStatus::Built { .. } | BuildStatus::Available => {
        (self.icons.done, self.config.theme.completed, None)
      },
      BuildStatus::Failed { .. } => {
        (self.icons.failed, self.config.theme.failed, None)
      },
      BuildStatus::DependencyFailed => {
        (self.icons.failed, self.config.theme.muted, None)
      },
    };
    spans.push(self.span(format!("{icon} "), color));
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
      || suffix.as_ref().map_or(0, |value| value.width()),
      |item| if item.total.is_some() { 12 } else { 8 },
    );
    let name_width =
      usize::from(self.width).saturating_sub(spans_width(&spans) + reserve);
    spans.push(self.span(truncate_text(&info.name.name, name_width), color));
    if let Some(aggregate) = transfer {
      spans.extend(self.transfer_suffix(&aggregate, spans_width(&spans)));
    }
    if !has_transfer && let Some(suffix) = suffix {
      spans.push(self.span(suffix, self.config.theme.muted));
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
      spans.push(self.span(
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
      "Transfers".to_string()
    } else {
      format!("Transfers ({hidden} hidden)")
    };
    let mut spans = self.prefix(ancestors, last);
    spans.push(self.span(label, self.config.theme.text));
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
      spans.push(self.span(
        if continues { "┃  " } else { "   " },
        self.config.theme.connector,
      ));
    }
    spans.push(self.span(
      if last { "┗━ " } else { "┣━ " },
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
    spans.push(
      self.span(format!("{name}{shared}"), self.transfer_color(transfer)),
    );
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
    let elapsed = format_duration(self.now - transfer.start);
    let essential = percent.map_or(8, |value| 7 + value.to_string().len());
    let available = usize::from(self.width).saturating_sub(occupied);
    let mut spans = vec![self.span(format!("  {arrow} "), color)];
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
      spans.push(self.span(format!("{value:>3}%"), color));
    } else {
      let spinner =
        ["◐", "◓", "◑", "◒"][((self.now * 4.0).max(0.0) as usize) % 4];
      spans.push(self.span(spinner, color));
    }
    if available > essential + bytes.len() + 2 {
      spans.push(self.span(format!("  {bytes}"), self.config.theme.muted));
    }
    if available > essential + bytes.len() + elapsed.len() + 5 {
      spans.push(self.span(format!("  {elapsed}"), self.config.theme.muted));
    }
    if transfer.host != "localhost"
      && available
        > essential + bytes.len() + elapsed.len() + transfer.host.len() + 9
    {
      spans.push(
        self.span(format!("  {}", transfer.host), self.config.theme.host),
      );
    }
    spans
  }
}

impl TreePlan {
  fn row_mut(&mut self, id: RowId) -> &mut PlannedRow {
    self.rows.get_mut(&id).expect("planned row exists")
  }
}
