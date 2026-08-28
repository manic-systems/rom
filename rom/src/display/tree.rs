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
  progress_bar,
  spans_width,
  truncate_text,
};
use crate::state::{BuildStatus, DerivationId};

pub(super) struct TreePlan {
  relevance: HashMap<DerivationId, u8>,
  roots:     Vec<DerivationId>,
  lines:     usize,
}

impl TreePlan {
  pub(super) const fn line_count(&self) -> usize {
    self.lines
  }
}

struct TreeSelection<'a> {
  plan:    &'a TreePlan,
  nodes:   &'a HashSet<DerivationId>,
  maximum: usize,
}

impl Renderer<'_> {
  pub(super) fn tree(&mut self) -> Vec<Line<'static>> {
    let plan = self.tree_plan();
    let maximum = plan.line_count();
    self.render_tree(&plan, maximum)
  }

  /// Select visible nodes before allocating lines or connector prefixes.
  /// Relevance is propagated towards the roots so active and failed subtrees
  /// win stable ties when the live graph is height-bounded.
  pub(super) fn tree_plan(&self) -> TreePlan {
    let mut relevance = HashMap::new();
    let mut pending = VecDeque::new();

    for (&id, info) in self.snapshot.derivations {
      let build_relevance = match info.build_status {
        BuildStatus::Failed { .. } => 4,
        BuildStatus::Building(_) => 3,
        BuildStatus::Planned => 2,
        BuildStatus::Unknown | BuildStatus::Built { .. } => 0,
      };
      let transfer_relevance =
        self
          .snapshot
          .transfers_by_drv
          .get(&id)
          .map_or(0, |transfers| {
            if transfers.iter().any(|transfer| !transfer.completed) {
              3
            } else {
              1
            }
          });
      let direct_relevance = build_relevance.max(transfer_relevance);
      if direct_relevance > 0 {
        relevance.insert(id, direct_relevance);
        pending.push_back(id);
      }
    }

    // Reverse reachability computes the connected ancestor set in
    // O(nodes + edges). The former recursive visibility predicate started a
    // fresh subtree walk for every node and became quadratic on deep graphs.
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

    let mut roots: Vec<_> = self
      .snapshot
      .roots
      .iter()
      .copied()
      .filter(|id| relevance.contains_key(id))
      .collect();
    roots.sort_by_key(|id| Reverse(relevance[id]));

    let transfer_lines = if self.snapshot.unmatched_transfers.is_empty() {
      0
    } else {
      1 + self.snapshot.unmatched_transfers.len()
    };
    let lines = 1_usize
      .saturating_add(relevance.len())
      .saturating_add(transfer_lines);
    TreePlan {
      relevance,
      roots,
      lines,
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
    let focus_active = self.focus_active(plan);
    let truncated = plan.line_count() > maximum
      || (focus_active
        && plan.relevance.values().any(|priority| *priority < 3));
    let content_limit = if truncated && maximum > 1 {
      maximum - 1
    } else {
      maximum
    };
    let selected = self.select_nodes(plan, content_limit.saturating_sub(1));
    let selection = TreeSelection {
      plan,
      nodes: &selected,
      maximum: content_limit,
    };
    let mut lines = vec![Line::from(vec![
      self.span("┏━ ", self.config.theme.connector),
      self.span("Builds", self.config.theme.text),
    ])];
    for &root in &plan.roots {
      self.push_node(&mut lines, root, &[], false, &selection);
      if lines.len() >= content_limit {
        break;
      }
    }
    if lines.len() < content_limit
      && !self.snapshot.unmatched_transfers.is_empty()
    {
      self.push_transfer_branch(&mut lines, false, content_limit);
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

  /// Choose a connected forest globally instead of exhausting one relevant
  /// branch depth-first. A node enters the frontier only after one of its
  /// parents was selected, so every chosen descendant retains a path to a
  /// displayed root. Stable sequence numbers preserve state insertion order
  /// between equally relevant nodes.
  fn select_nodes(
    &self,
    plan: &TreePlan,
    maximum: usize,
  ) -> HashSet<DerivationId> {
    let focus_active = self.focus_active(plan);
    if !focus_active && plan.relevance.len() <= maximum {
      return plan.relevance.keys().copied().collect();
    }

    let mut selected = HashSet::with_capacity(maximum);
    let mut frontier = BinaryHeap::new();
    let mut sequence = 0_usize;
    for &root in &plan.roots {
      frontier.push((plan.relevance[&root], Reverse(sequence), root));
      sequence = sequence.saturating_add(1);
    }
    while selected.len() < maximum {
      let Some((relevance, _, id)) = frontier.pop() else {
        break;
      };
      // While work is active, waiting derivations are already represented by
      // the legend. Spending spare rows on them produces walls of low-value
      // leaves and can push another active branch below the fold.
      if focus_active && relevance < 3 {
        break;
      }
      if !selected.insert(id) {
        continue;
      }
      let Some(info) = self.snapshot.derivation(id) else {
        continue;
      };
      for child in &info.input_derivations {
        if let Some(&relevance) = plan.relevance.get(&child.derivation) {
          frontier.push((relevance, Reverse(sequence), child.derivation));
          sequence = sequence.saturating_add(1);
        }
      }
    }
    selected
  }

  fn focus_active(&self, plan: &TreePlan) -> bool {
    plan.relevance.values().any(|priority| *priority >= 3)
      || self
        .snapshot
        .unmatched_transfers
        .iter()
        .any(|transfer| !transfer.completed)
  }

  fn push_node(
    &mut self,
    lines: &mut Vec<Line<'static>>,
    id: DerivationId,
    ancestor_continues: &[bool],
    last: bool,
    selection: &TreeSelection<'_>,
  ) {
    if lines.len() >= selection.maximum || !selection.nodes.contains(&id) {
      return;
    }
    if !self.seen.insert(id) {
      return;
    }
    let Some(info) = self.snapshot.derivation(id) else {
      return;
    };

    let mut spans = self.prefix(ancestor_continues, last);
    let (icon, color, suffix) = match &info.build_status {
      BuildStatus::Unknown => {
        (self.icons.planned, self.config.theme.muted, None)
      },
      BuildStatus::Planned => {
        (self.icons.planned, self.config.theme.planned, None)
      },
      BuildStatus::Building(build) => {
        let suffix = self.config.show_timers.then(|| {
          format!(
            "  {} {}",
            self.icons.clock,
            format_duration(self.now - build.start)
          )
        });
        (self.icons.running, self.config.theme.running, suffix)
      },
      BuildStatus::Built { .. } => {
        (self.icons.done, self.config.theme.completed, None)
      },
      BuildStatus::Failed { .. } => {
        (self.icons.failed, self.config.theme.failed, None)
      },
    };
    spans.push(self.span(format!("{icon} "), color));
    let transfer = self
      .snapshot
      .transfers_by_drv
      .get(&id)
      .map(|transfers| aggregate_transfers(transfers));
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
    lines.push(fit_line(spans, self.width));

    let mut children: Vec<_> = info
      .input_derivations
      .iter()
      .map(|child| child.derivation)
      .filter(|child| {
        !self.seen.contains(child) && selection.nodes.contains(child)
      })
      .collect();
    children.sort_by_key(|child| Reverse(selection.plan.relevance[child]));
    let mut ancestors = ancestor_continues.to_vec();
    ancestors.push(!last);
    let count = children.len();
    for (index, child) in children.into_iter().enumerate() {
      self.push_node(lines, child, &ancestors, index + 1 == count, selection);
      if lines.len() >= selection.maximum {
        break;
      }
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

  fn push_transfer_branch(
    &self,
    lines: &mut Vec<Line<'static>>,
    last: bool,
    maximum: usize,
  ) {
    if lines.len() >= maximum {
      return;
    }
    lines.push(Line::from(vec![
      self.span(
        if last { "┗━ " } else { "┣━ " },
        self.config.theme.connector,
      ),
      self.span("Transfers", self.config.theme.text),
    ]));
    for (index, transfer) in
      self.snapshot.unmatched_transfers.iter().enumerate()
    {
      if lines.len() >= maximum {
        break;
      }
      let last_transfer = index + 1 == self.snapshot.unmatched_transfers.len();
      let mut spans = vec![
        self.span(
          if last { "   " } else { "┃  " },
          self.config.theme.connector,
        ),
        self.span(
          if last_transfer { "┗━ " } else { "┣━ " },
          self.config.theme.connector,
        ),
        self.span(transfer.name.clone(), self.transfer_color(transfer)),
      ];
      spans.extend(self.transfer_suffix(transfer, spans_width(&spans)));
      lines.push(fit_line(spans, self.width));
    }
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
      // Priority is percent, bar, bytes, elapsed, then host. Optional fields
      // are appended later and `fit_line` removes them from the right.
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
