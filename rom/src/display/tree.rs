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
  priority:  u8,
  children:  Vec<RowId>,
  inline:    Vec<usize>,
  consumers: usize,
}

impl PlannedRow {
  const fn new(priority: u8) -> Self {
    Self {
      priority,
      children: Vec::new(),
      inline: Vec::new(),
      consumers: 0,
    }
  }
}

#[derive(Default)]
pub(super) struct TreePlan {
  rows:  HashMap<RowId, PlannedRow>,
  roots: Vec<RowId>,
}

impl TreePlan {
  pub(super) fn line_count(&self) -> usize {
    // The Builds header is not represented by a row.
    self.rows.len().saturating_add(1)
  }

  fn row(&self, id: RowId) -> &PlannedRow {
    &self.rows[&id]
  }
}

struct TreeSelection<'a> {
  plan:    &'a TreePlan,
  rows:    HashSet<RowId>,
  maximum: usize,
}

impl Renderer<'_> {
  /// Build one typed row graph. Selection and rendering operate on these same
  /// rows, so transfer placement cannot be lost in parallel index maps.
  pub(super) fn tree_plan(&self) -> TreePlan {
    let mut relevance = HashMap::new();
    let mut pending = VecDeque::new();

    for (&id, info) in self.snapshot.derivations {
      let priority = match info.build_status {
        BuildStatus::Failed { .. } => 4,
        BuildStatus::Building(_) => 3,
        BuildStatus::Planned => 2,
        BuildStatus::Unknown | BuildStatus::Built { .. } => 0,
      };
      if priority > 0 {
        relevance.insert(id, priority);
      }
    }
    for item in &self.snapshot.placed_transfers {
      if let Some(id) = item.placement.derivation() {
        let priority = if item.transfer.completed { 1 } else { 3 };
        relevance
          .entry(id)
          .and_modify(|current| *current = (*current).max(priority))
          .or_insert(priority);
      }
    }
    pending.extend(relevance.keys().copied());

    // Reverse reachability computes the connected ancestor set in O(V + E).
    while let Some(id) = pending.pop_front() {
      let priority = relevance[&id];
      let Some(info) = self.snapshot.derivation(id) else {
        continue;
      };
      for &parent in &info.derivation_parents {
        let previous = relevance.get(&parent).copied().unwrap_or(0);
        if priority > previous {
          relevance.insert(parent, priority);
          pending.push_back(parent);
        }
      }
    }

    let mut plan = TreePlan::default();
    for (&id, &priority) in &relevance {
      plan
        .rows
        .insert(RowId::Build(id), PlannedRow::new(priority));
    }
    for &id in relevance.keys() {
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
          group.children.push(id);
        },
      }
    }

    plan.roots = self
      .snapshot
      .roots
      .iter()
      .copied()
      .map(RowId::Build)
      .filter(|root| plan.rows.contains_key(root))
      .collect();
    let rows = &plan.rows;
    plan.roots.sort_by_key(|root| Reverse(rows[root].priority));
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
    let focus_active = self.focus_active(plan);
    let truncated = plan.line_count() > maximum
      || (focus_active && plan.rows.values().any(|row| row.priority < 3));
    let content_limit = if truncated && maximum > 1 {
      maximum - 1
    } else {
      maximum
    };
    let selection = self.select_rows(plan, content_limit);
    let mut lines = vec![Line::from(vec![
      self.span("┏━ ", self.config.theme.connector),
      self.span("Builds", self.config.theme.text),
    ])];
    for &root in &plan.roots {
      self.push_row(&mut lines, root, &[], false, &selection);
      if lines.len() >= content_limit {
        break;
      }
    }
    if truncated && maximum > 1 {
      let hidden = plan.line_count().saturating_sub(lines.len());
      lines.push(Line::from(vec![
        self.span("┣━ ", self.config.theme.connector),
        self.span(format!("… {hidden} hidden"), self.config.theme.muted),
      ]));
    }
    lines
  }

  /// Select connected build/group rows first, then share remaining rows fairly
  /// among transfer children. Completed grace rows never displace active work.
  fn select_rows<'a>(
    &self,
    plan: &'a TreePlan,
    maximum: usize,
  ) -> TreeSelection<'a> {
    let mut selection = TreeSelection {
      plan,
      rows: HashSet::new(),
      maximum,
    };
    let focus_active = self.focus_active(plan);
    let mut frontier = BinaryHeap::new();
    let mut sequence = 0_usize;
    for &root in &plan.roots {
      frontier.push((plan.row(root).priority, Reverse(sequence), root));
      sequence += 1;
    }

    let mut remaining = maximum.saturating_sub(1); // Builds header
    let mut detail_groups = Vec::new();
    while remaining > 0 {
      let Some((priority, _, id)) = frontier.pop() else {
        break;
      };
      if focus_active && priority < 3 {
        break;
      }
      if !selection.rows.insert(id) {
        continue;
      }
      let row = plan.row(id);
      let mut details = Vec::new();
      for &child in &row.children {
        match child {
          RowId::Build(_) => {
            frontier.push((plan.row(child).priority, Reverse(sequence), child));
            sequence += 1;
          },
          RowId::Transfer(_) => details.push(child),
          RowId::TransferGroup => {},
        }
      }
      if !details.is_empty() {
        detail_groups.push(details);
      }
      remaining -= 1;
    }

    for completed in [false, true] {
      let mut groups: VecDeque<_> = detail_groups
        .iter()
        .map(|rows| {
          rows.iter().copied().filter(|id| {
            let RowId::Transfer(index) = id else {
              return false;
            };
            self.snapshot.placed_transfers[*index].transfer.completed
              == completed
          })
        })
        .collect();
      while remaining > 0 {
        let Some(mut group) = groups.pop_front() else {
          break;
        };
        if let Some(id) = group.next() {
          selection.rows.insert(id);
          remaining -= 1;
          groups.push_back(group);
        }
      }
    }
    selection
  }

  fn focus_active(&self, plan: &TreePlan) -> bool {
    plan.rows.values().any(|row| row.priority >= 3)
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
        (
          self.icons.running,
          self.config.theme.running,
          (!suffix.is_empty()).then_some(suffix),
        )
      },
      BuildStatus::Built { .. } => {
        (self.icons.done, self.config.theme.completed, None)
      },
      BuildStatus::Failed { .. } => {
        (self.icons.failed, self.config.theme.failed, None)
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
