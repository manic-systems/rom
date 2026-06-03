//! Full-screen ratatui renderer for ROM.
mod activity;
mod logs;

use ratatui::{
  Frame,
  layout::{Constraint, Layout},
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, Paragraph, Wrap},
};

pub use self::logs::{TuiLogSearch, TuiLogs, build_log_search};
use self::{activity::render_activity_graph_lines, logs::logs_pane};
use crate::{
  display::{DisplayConfig, format_duration},
  state::{RenderSnapshot, current_time},
};

const TEXT_PRIMARY: Color = Color::Rgb(214, 217, 207);
const TEXT_MUTED: Color = Color::Rgb(156, 162, 150);
const GRAPH_LINE_COLOR: Color = Color::Rgb(82, 89, 78);
const MOSS_GREEN: Color = Color::Rgb(158, 190, 112);
const BUILT_GREEN: Color = Color::Rgb(128, 158, 94);
const DOWNLOAD_BLUE: Color = Color::Rgb(116, 168, 196);
const MUTED_RED: Color = Color::Rgb(204, 102, 96);
const MUTED_YELLOW: Color = Color::Rgb(224, 190, 96);
const SPINNER_FRAMES: &[&str] = &["⢄", "⢂", "⢁", "⡁", "⡈", "⡐", "⡠"];

#[derive(Clone, Copy, Default)]
pub struct TuiConfig {
  pub display:        DisplayConfig,
  pub log_line_limit: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct TuiView {
  pub log_scroll:    usize,
  pub search_query:  String,
  pub search_active: bool,
  pub paused:        bool,
  pub log_wrap:      bool,
}

impl Default for TuiView {
  fn default() -> Self {
    Self {
      log_scroll:    0,
      search_query:  String::new(),
      search_active: false,
      paused:        false,
      log_wrap:      true,
    }
  }
}

pub fn draw(
  frame: &mut Frame<'_>,
  state: &RenderSnapshot,
  logs: &[String],
  config: &TuiConfig,
  view: &TuiView,
) {
  let logs = TuiLogs::for_view(logs.to_vec(), view);
  draw_prepared(frame, state, &logs, config, view);
}

pub fn draw_prepared(
  frame: &mut Frame<'_>,
  state: &RenderSnapshot,
  logs: &TuiLogs,
  config: &TuiConfig,
  view: &TuiView,
) {
  let area = frame.area();
  let log_height = area.height.saturating_sub(5).clamp(4, 14);
  let layout = Layout::vertical([
    Constraint::Length(1),
    Constraint::Min(3),
    Constraint::Length(log_height),
  ])
  .split(area);

  frame.render_widget(status_header(state, view), layout[0]);
  frame.render_widget(
    graph(state, config, view, layout[1].height, layout[1].width),
    layout[1],
  );
  frame.render_widget(logs_pane(logs, view, layout[2].height), layout[2]);
}

fn graph(
  state: &RenderSnapshot,
  config: &TuiConfig,
  view: &TuiView,
  height: u16,
  width: u16,
) -> Paragraph<'static> {
  let visible_lines = height.saturating_sub(1) as usize;
  let visible_width = width as usize;
  let mut lines = render_activity_graph_lines(
    state,
    config.display,
    visible_lines,
    visible_width,
  );
  if lines.is_empty() {
    lines.push(idle_graph_line(state, visible_width));
  }
  bottom_align(&mut lines, visible_lines);

  Paragraph::new(lines)
    .block(
      Block::default()
        .borders(Borders::TOP)
        .border_style(hierarchy_style())
        .title(graph_title(view)),
    )
    .wrap(Wrap { trim: false })
}

fn idle_graph_line(state: &RenderSnapshot, width: usize) -> Line<'static> {
  if let Some(line) = evaluation_graph_line(state, width) {
    return line;
  }

  Line::from(Span::styled(
    "Waiting for Nix activity...",
    secondary_style(),
  ))
}

fn evaluation_graph_line(
  state: &RenderSnapshot,
  width: usize,
) -> Option<Line<'static>> {
  let eval = &state.evaluation_state;
  if eval.count == 0 && eval.last_file_name.is_none() {
    return None;
  }

  let count_label = if eval.count == 1 {
    "1 file".to_string()
  } else {
    format!("{} files", eval.count)
  };
  let name_budget = width
    .saturating_sub(" Evaluating ".len() + count_label.len() + 4)
    .clamp(12, 72);
  let file = eval.last_file_name.as_deref().map_or_else(
    || "Nix expression".to_string(),
    |path| compact_eval_path(path, name_budget),
  );

  Some(Line::from(vec![
    Span::styled(
      spinner_frame(current_time()),
      Style::default().fg(Color::Yellow),
    ),
    Span::raw(" "),
    Span::styled("Evaluating", Style::default().fg(TEXT_PRIMARY)),
    Span::raw(" "),
    Span::styled(file, secondary_style()),
    Span::raw(" "),
    Span::styled(count_label, secondary_style()),
  ]))
}

fn spinner_frame(now: f64) -> &'static str {
  let frame = ((now * 1000.0) as usize / 80) % SPINNER_FRAMES.len();
  SPINNER_FRAMES[frame]
}

fn compact_eval_path(path: &str, max_chars: usize) -> String {
  let path = path.trim();
  let components: Vec<&str> = path
    .split('/')
    .filter(|component| !component.is_empty())
    .collect();

  if components.len() >= 3 {
    let tail = components[components.len() - 3..].join("/");
    if tail.chars().count() <= max_chars {
      return tail;
    }
  }

  truncate_start(path, max_chars)
}

fn truncate_start(value: &str, max_chars: usize) -> String {
  let len = value.chars().count();
  if len <= max_chars {
    return value.to_string();
  }
  if max_chars <= 3 {
    return ".".repeat(max_chars);
  }

  let tail: String = value.chars().skip(len - (max_chars - 3)).collect();
  format!("...{tail}")
}

fn secondary_style() -> Style {
  Style::default().fg(TEXT_MUTED)
}

pub(super) fn hierarchy_style() -> Style {
  Style::default().fg(GRAPH_LINE_COLOR)
}

fn status_header(state: &RenderSnapshot, view: &TuiView) -> Paragraph<'static> {
  let summary = &state.full_summary;
  let mut spans = vec![
    Span::styled(
      "Builds",
      Style::default()
        .fg(TEXT_PRIMARY)
        .add_modifier(Modifier::BOLD),
    ),
    Span::raw("  "),
  ];

  push_status_count(
    &mut spans,
    summary.running_builds.len(),
    Style::default().fg(MOSS_GREEN),
    "running",
  );
  push_status_count(
    &mut spans,
    summary.completed_builds.len(),
    Style::default().fg(BUILT_GREEN),
    "built",
  );
  push_status_count(
    &mut spans,
    summary.failed_builds.len(),
    Style::default().fg(MUTED_RED),
    "failed",
  );
  push_status_count(
    &mut spans,
    summary.planned_builds.len(),
    secondary_style(),
    "waiting",
  );

  if summary.running_downloads.len() + summary.completed_downloads.len() > 0 {
    push_header_gap(&mut spans);
    spans.push(Span::styled("Downloads", secondary_style()));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
      format!("{}", summary.running_downloads.len()),
      Style::default().fg(TEXT_PRIMARY),
    ));
    spans.push(Span::styled(" active", secondary_style()));
  }

  if summary.running_uploads.len() + summary.completed_uploads.len() > 0 {
    push_header_gap(&mut spans);
    spans.push(Span::styled("Uploads", secondary_style()));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
      format!("{}", summary.running_uploads.len()),
      Style::default().fg(TEXT_PRIMARY),
    ));
    spans.push(Span::styled(" active", secondary_style()));
  }

  push_header_gap(&mut spans);
  if view.paused {
    spans.push(Span::styled(
      "paused",
      Style::default()
        .fg(MUTED_YELLOW)
        .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw("  "));
  }
  spans.push(Span::styled("elapsed", secondary_style()));
  spans.push(Span::raw(" "));
  spans.push(Span::styled(
    format_duration(current_time() - state.start_time),
    secondary_style(),
  ));

  Paragraph::new(vec![Line::from(spans)]).wrap(Wrap { trim: false })
}

fn push_status_count(
  spans: &mut Vec<Span<'static>>,
  count: usize,
  active_style: Style,
  label: &'static str,
) {
  if spans.len() > 2 {
    spans.push(Span::styled(" / ", secondary_style()));
  }
  let count_style = if count == 0 {
    secondary_style()
  } else {
    active_style
  };
  spans.push(Span::styled(format!("{count}"), count_style));
  spans.push(Span::raw(" "));
  spans.push(Span::styled(label, secondary_style()));
}

fn push_header_gap(spans: &mut Vec<Span<'static>>) {
  spans.push(Span::styled("   ", secondary_style()));
}

fn graph_title(view: &TuiView) -> &'static str {
  if view.paused {
    "Build Graph [paused]"
  } else {
    "Build Graph"
  }
}

fn bottom_align(lines: &mut Vec<Line<'static>>, height: usize) {
  let padding = height.saturating_sub(lines.len());
  if padding == 0 {
    return;
  }

  let mut aligned = Vec::with_capacity(height);
  aligned.resize_with(padding, Line::default);
  aligned.append(lines);
  *lines = aligned;
}
