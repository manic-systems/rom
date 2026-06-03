use std::collections::HashSet;

use ratatui::text::{Line, Span};

use super::{TransferLine, row::RenderedActivityLine};
use crate::{state::StorePathId, tui::secondary_style};

fn unrendered_transfer_lines(
  transfer_lines: &[TransferLine],
  rendered_transfer_paths: &HashSet<StorePathId>,
) -> Vec<Line<'static>> {
  transfer_lines
    .iter()
    .filter(|line| !rendered_transfer_paths.contains(&line.path_id))
    .map(|line| line.line.clone())
    .collect()
}

pub(super) fn combine_activity_lines(
  tree_lines: Vec<RenderedActivityLine>,
  transfer_lines: &[TransferLine],
  max_lines: usize,
) -> Vec<Line<'static>> {
  if max_lines == 0 {
    return Vec::new();
  }
  if tree_lines.is_empty() {
    return truncate_activity_lines(
      unrendered_transfer_lines(transfer_lines, &HashSet::new()),
      max_lines,
    );
  }
  if max_lines == 1 {
    let visible_paths =
      rendered_transfer_paths_for_budget(&tree_lines, max_lines);
    let transfer_lines =
      unrendered_transfer_lines(transfer_lines, &visible_paths);
    if transfer_lines.is_empty() {
      return activity_lines_for_budget(&tree_lines, max_lines);
    }
    return truncate_activity_lines(transfer_lines, max_lines);
  }

  let transfer_budget = standalone_transfer_budget(max_lines);
  let mut tree_budget = max_lines;
  for _ in 0..=max_lines {
    let visible_paths =
      rendered_transfer_paths_for_budget(&tree_lines, tree_budget);
    let transfer_lines =
      unrendered_transfer_lines(transfer_lines, &visible_paths);

    if transfer_lines.is_empty() {
      return activity_lines_for_budget(&tree_lines, tree_budget);
    }

    let mut transfer_lines =
      truncate_activity_lines(transfer_lines, transfer_budget);
    let next_tree_budget = max_lines.saturating_sub(transfer_lines.len());
    if next_tree_budget == tree_budget {
      transfer_lines
        .extend(activity_lines_for_budget(&tree_lines, tree_budget));
      return transfer_lines;
    }
    tree_budget = next_tree_budget;
  }

  activity_lines_for_budget(&tree_lines, max_lines)
}

fn standalone_transfer_budget(max_lines: usize) -> usize {
  if max_lines <= 3 {
    return 1;
  }

  (max_lines / 4).clamp(1, 6).min(max_lines - 3)
}

fn rendered_transfer_paths_for_budget(
  lines: &[RenderedActivityLine],
  max_lines: usize,
) -> HashSet<StorePathId> {
  visible_activity_line_slice(lines, max_lines)
    .iter()
    .filter_map(|line| line.transfer_path_id)
    .collect()
}

fn activity_lines_for_budget(
  lines: &[RenderedActivityLine],
  max_lines: usize,
) -> Vec<Line<'static>> {
  if lines.len() <= max_lines {
    let mut lines = lines.to_vec();
    clean_isolated_tree_rails(&mut lines);
    return lines.iter().map(RenderedActivityLine::to_line).collect();
  }
  if max_lines == 0 {
    return Vec::new();
  }
  if max_lines == 1 {
    return lines
      .last()
      .map(|line| line.to_line())
      .into_iter()
      .collect();
  }

  let tail_len = max_lines - 1;
  let hidden = lines.len().saturating_sub(tail_len);
  let mut visible = Vec::with_capacity(max_lines);
  visible.push(hidden_activity_line(hidden));
  let mut tree_lines = visible_activity_line_slice(lines, max_lines).to_vec();
  clean_isolated_tree_rails(&mut tree_lines);
  visible.extend(tree_lines.iter().map(RenderedActivityLine::to_line));
  visible
}

fn visible_activity_line_slice(
  lines: &[RenderedActivityLine],
  max_lines: usize,
) -> &[RenderedActivityLine] {
  if lines.len() <= max_lines {
    return lines;
  }
  if max_lines == 0 {
    return &[];
  }
  if max_lines == 1 {
    return lines.last().map(std::slice::from_ref).unwrap_or_default();
  }

  let tail_len = max_lines - 1;
  let tail_start = lines.len().saturating_sub(tail_len);
  &lines[tail_start..]
}

fn truncate_activity_lines(
  lines: Vec<Line<'static>>,
  max_lines: usize,
) -> Vec<Line<'static>> {
  if lines.len() <= max_lines {
    return lines;
  }
  if max_lines == 0 {
    return Vec::new();
  }
  if max_lines == 1 {
    return lines.into_iter().rev().take(1).collect();
  }

  let tail_len = max_lines - 1;
  let hidden = lines.len().saturating_sub(tail_len);
  let tail_start = lines.len().saturating_sub(tail_len);
  let mut visible = Vec::with_capacity(max_lines);
  visible.push(hidden_activity_line(hidden));
  visible.extend(lines.into_iter().skip(tail_start));
  visible
}

fn hidden_activity_line(hidden: usize) -> Line<'static> {
  let label = if hidden == 1 {
    "1 hidden row above".to_string()
  } else {
    format!("{hidden} hidden rows above")
  };
  Line::from(vec![
    Span::styled("⋮", secondary_style()),
    Span::raw(" "),
    Span::styled(label, secondary_style()),
  ])
}

fn clean_isolated_tree_rails(lines: &mut [RenderedActivityLine]) {
  let cells = lines
    .iter()
    .map(|line| line.graph_cells.clone())
    .collect::<Vec<_>>();
  let mut replacements = Vec::new();

  for (line_index, line_cells) in cells.iter().enumerate() {
    for (column, cell) in line_cells.iter().enumerate() {
      if !cell.is_vertical_rail() {
        continue;
      }

      let connected_above = line_index
        .checked_sub(1)
        .and_then(|index| cells.get(index))
        .and_then(|line| line.get(column))
        .is_some_and(|cell| cell.has_down_edge());
      let connected_below = cells
        .get(line_index + 1)
        .and_then(|line| line.get(column))
        .is_some_and(|cell| cell.has_up_edge());

      if !connected_above && !connected_below {
        replacements.push((line_index, column));
      }
    }
  }

  for (line_index, column) in replacements {
    if let Some(cell) = lines[line_index].graph_cells.get_mut(column) {
      cell.clear();
    }
  }
}
