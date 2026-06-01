use std::borrow::Cow;

use ansi_to_tui::IntoText;
use nucleo_matcher::{
  Config,
  Matcher,
  Utf32Str,
  pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};
use ratatui::{
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, Paragraph, Wrap},
};

use super::{TuiView, hierarchy_style};

const MAX_RENDERED_LOG_LINE_CHARS: usize = 2_000;

pub(super) fn logs_pane(
  logs: &[String],
  view: &TuiView,
  height: u16,
) -> Paragraph<'static> {
  let visible_lines = height.saturating_sub(1) as usize;
  let (mut lines, match_count) = if view.search_query.is_empty() {
    visible_plain_log_lines(logs, view.log_scroll, visible_lines)
  } else {
    visible_matching_log_lines(logs, view, visible_lines)
  };
  if lines.is_empty() {
    let empty_message = if view.search_query.is_empty() {
      "No log lines yet."
    } else {
      "No matching log lines."
    };
    lines.push(Line::from(empty_message));
  }

  let paragraph = Paragraph::new(lines).block(
    Block::default()
      .borders(Borders::TOP)
      .border_style(hierarchy_style())
      .title(logs_title(view, match_count)),
  );
  if view.log_wrap {
    paragraph.wrap(Wrap { trim: false })
  } else {
    paragraph
  }
}

fn visible_plain_log_lines(
  logs: &[String],
  log_scroll: usize,
  visible_lines: usize,
) -> (Vec<Line<'static>>, usize) {
  let max_scroll = logs.len().saturating_sub(visible_lines);
  let scroll = log_scroll.min(max_scroll);
  let end = logs.len().saturating_sub(scroll);
  let start = end.saturating_sub(visible_lines);
  let lines = logs[start..end]
    .iter()
    .flat_map(|line| parse_ansi_line(line))
    .collect();
  (lines, logs.len())
}

fn visible_matching_log_lines(
  logs: &[String],
  view: &TuiView,
  visible_lines: usize,
) -> (Vec<Line<'static>>, usize) {
  let matching_logs = matching_logs(logs, &view.search_query);
  let match_count = matching_logs.len();
  let max_scroll = matching_logs.len().saturating_sub(visible_lines);
  let scroll = view.log_scroll.min(max_scroll);
  let end = matching_logs.len().saturating_sub(scroll);
  let start = end.saturating_sub(visible_lines);
  let lines = matching_logs[start..end]
    .iter()
    .flat_map(|line| render_log_line(line))
    .collect();
  (lines, match_count)
}

fn logs_title(view: &TuiView, match_count: usize) -> String {
  let mut title = String::from("Logs");
  if view.paused {
    title.push_str(" [paused]");
  }
  if view.search_active {
    title.push_str(" [search]");
  }
  if !view.log_wrap {
    title.push_str(" [nowrap]");
  }
  if !view.search_query.is_empty() {
    title.push_str(&format!(
      " [/{query}: {match_count}]",
      query = view.search_query
    ));
  }
  title
}

struct LogMatch<'a> {
  line:    &'a str,
  indices: Vec<usize>,
}

fn matching_logs<'a>(logs: &'a [String], query: &str) -> Vec<LogMatch<'a>> {
  if query.is_empty() {
    return logs
      .iter()
      .map(|line| {
        LogMatch {
          line:    line.as_str(),
          indices: Vec::new(),
        }
      })
      .collect();
  }

  let pattern = Pattern::new(
    query,
    CaseMatching::Ignore,
    Normalization::Smart,
    AtomKind::Fuzzy,
  );
  let mut matcher = Matcher::new(Config::DEFAULT);
  let mut utf32_buffer = Vec::new();

  logs
    .iter()
    .filter_map(|line| {
      match_log_line(line, &pattern, &mut matcher, &mut utf32_buffer)
    })
    .collect()
}

fn match_log_line<'a>(
  line: &'a str,
  pattern: &Pattern,
  matcher: &mut Matcher,
  utf32_buffer: &mut Vec<char>,
) -> Option<LogMatch<'a>> {
  let bounded = bounded_log_line(line);
  let text = strip_ansi_for_matching(bounded.as_ref());
  let mut indices = Vec::new();
  pattern.indices(Utf32Str::new(&text, utf32_buffer), matcher, &mut indices)?;
  indices.sort_unstable();
  indices.dedup();

  Some(LogMatch {
    line,
    indices: indices.into_iter().map(|index| index as usize).collect(),
  })
}

fn render_log_line(line: &LogMatch<'_>) -> Vec<Line<'static>> {
  let parsed = parse_ansi_line(line.line);
  if line.indices.is_empty() {
    return parsed;
  }
  highlight_matches(parsed, &line.indices)
}

fn strip_ansi_for_matching(line: &str) -> String {
  let mut stripped = String::with_capacity(line.len());
  let mut chars = line.chars().peekable();

  while let Some(ch) = chars.next() {
    if ch == '\x1b' && chars.peek() == Some(&'[') {
      chars.next();
      for code in chars.by_ref() {
        if ('@'..='~').contains(&code) {
          break;
        }
      }
      continue;
    }
    stripped.push(ch);
  }

  stripped
}

fn parse_ansi_line(line: &str) -> Vec<Line<'static>> {
  let line = bounded_log_line(line);
  match line.as_ref().into_text() {
    Ok(text) if !text.lines.is_empty() => text.lines,
    _ => vec![Line::from(line.into_owned())],
  }
}

fn bounded_log_line(line: &str) -> Cow<'_, str> {
  if line.chars().count() <= MAX_RENDERED_LOG_LINE_CHARS {
    return Cow::Borrowed(line);
  }

  let mut truncated = line
    .chars()
    .take(MAX_RENDERED_LOG_LINE_CHARS)
    .collect::<String>();
  truncated.push('…');
  Cow::Owned(truncated)
}

fn highlight_matches(
  lines: Vec<Line<'static>>,
  indices: &[usize],
) -> Vec<Line<'static>> {
  let line_count = lines.len();
  let mut cursor = 0;

  lines
    .into_iter()
    .enumerate()
    .map(|(line_index, mut line)| {
      line.spans = line
        .spans
        .into_iter()
        .flat_map(|span| highlight_span(span, indices, &mut cursor))
        .collect();

      if line_index + 1 < line_count {
        cursor += 1;
      }

      line
    })
    .collect()
}

fn highlight_span(
  span: Span<'static>,
  indices: &[usize],
  cursor: &mut usize,
) -> Vec<Span<'static>> {
  let base_style = span.style;
  let content = span.content.into_owned();
  let mut spans = Vec::new();
  let mut chunk = String::new();
  let mut chunk_highlighted = None;

  for character in content.chars() {
    let highlighted = indices.binary_search(&*cursor).is_ok();
    if let Some(current) = chunk_highlighted
      && current != highlighted
    {
      push_highlight_chunk(
        &mut spans,
        std::mem::take(&mut chunk),
        base_style,
        current,
      );
    }

    chunk_highlighted = Some(highlighted);
    chunk.push(character);
    *cursor += 1;
  }

  if let Some(highlighted) = chunk_highlighted {
    push_highlight_chunk(&mut spans, chunk, base_style, highlighted);
  }

  spans
}

fn push_highlight_chunk(
  spans: &mut Vec<Span<'static>>,
  content: String,
  base_style: Style,
  highlighted: bool,
) {
  if content.is_empty() {
    return;
  }

  let style = if highlighted {
    base_style.patch(search_highlight_style())
  } else {
    base_style
  };
  spans.push(Span::styled(content, style));
}

fn search_highlight_style() -> Style {
  Style::default()
    .bg(Color::Yellow)
    .add_modifier(Modifier::BOLD)
}
