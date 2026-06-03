use super::support::*;

#[test]
fn tui_scrolls_logs() {
  let backend = TestBackend::new(80, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let state = running_state();
  let logs: Vec<String> = (0..30).map(|n| format!("line {n:02}")).collect();
  let config = tui_config();
  let view = TuiView {
    log_scroll: 5,
    ..TuiView::default()
  };

  terminal
    .draw(|frame| draw(frame, &state.render_snapshot(), &logs, &config, &view))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("line 13"),
    "scrolled log window missed expected line: {rendered}"
  );
  assert!(
    !rendered.contains("line 29"),
    "scrolled log window should not stay pinned to tail: {rendered}"
  );
}

#[test]
fn tui_filters_logs_by_search_query() {
  let backend = TestBackend::new(80, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let state = running_state();
  let logs = vec![
    "alpha".to_string(),
    "needle one".to_string(),
    "beta".to_string(),
    "needle two".to_string(),
  ];
  let config = tui_config();
  let view = TuiView {
    search_query: "ndl".to_string(),
    search_active: true,
    ..TuiView::default()
  };

  terminal
    .draw(|frame| draw(frame, &state.render_snapshot(), &logs, &config, &view))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(rendered.contains("needle one"), "missing match: {rendered}");
  assert!(rendered.contains("needle two"), "missing match: {rendered}");
  assert!(!rendered.contains("beta"), "non-match rendered: {rendered}");
  assert!(
    rendered.contains("/ndl: 2"),
    "search status missing match count: {rendered}"
  );
  assert!(
    terminal.backend().buffer().content().iter().any(|cell| {
      cell.bg == Color::Yellow && cell.modifier.contains(Modifier::BOLD)
    }),
    "fuzzy search matches were not highlighted: {rendered}"
  );
}

#[test]
fn tui_marks_paused_view() {
  let backend = TestBackend::new(80, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let state = running_state();
  let logs = Vec::new();
  let config = tui_config();
  let view = TuiView {
    paused: true,
    ..TuiView::default()
  };

  terminal
    .draw(|frame| draw(frame, &state.render_snapshot(), &logs, &config, &view))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("[paused]"),
    "missing paused status: {rendered}"
  );
}

#[test]
fn tui_can_disable_log_wrapping() {
  let backend = TestBackend::new(40, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let state = running_state();
  let logs =
    vec!["prefix-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-tail-marker".to_string()];
  let config = tui_config();
  let view = TuiView {
    log_wrap: false,
    ..TuiView::default()
  };

  terminal
    .draw(|frame| draw(frame, &state.render_snapshot(), &logs, &config, &view))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("[nowrap]"),
    "missing nowrap status: {rendered}"
  );
  assert!(
    !rendered.contains("tail-marker"),
    "unwrapped log line should be clipped, not wrapped: {rendered}"
  );
}

#[test]
fn tui_wraps_logs_by_default() {
  let backend = TestBackend::new(40, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let state = running_state();
  let logs =
    vec!["prefix-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-tail-marker".to_string()];
  let config = tui_config();

  terminal
    .draw(|frame| {
      draw(
        frame,
        &state.render_snapshot(),
        &logs,
        &config,
        &TuiView::default(),
      )
    })
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("tail-marker"),
    "default log wrapping should show wrapped tail: {rendered}"
  );
}

#[test]
fn tui_converts_ansi_graph_styles() {
  let backend = TestBackend::new(80, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let state = running_state();
  let logs = Vec::new();
  let config = tui_config();
  let raw_graph = render_state_lines(&state, config.display).join("\n");
  assert!(
    raw_graph.contains('\x1b'),
    "fixture graph did not contain ANSI escapes: {raw_graph:?}"
  );

  terminal
    .draw(|frame| {
      draw(
        frame,
        &state.render_snapshot(),
        &logs,
        &config,
        &TuiView::default(),
      )
    })
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    terminal
      .backend()
      .buffer()
      .content()
      .iter()
      .any(|cell| cell.fg != Color::Reset),
    "graph ANSI SGR was not converted into ratatui cell style: \
     raw={raw_graph:?} rendered={rendered}"
  );
}

#[test]
fn tui_converts_ansi_log_styles() {
  let backend = TestBackend::new(80, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let state = running_state();
  let logs = vec!["plain \x1b[35;1mcolored\x1b[0m".to_string()];
  let config = tui_config();

  terminal
    .draw(|frame| {
      draw(
        frame,
        &state.render_snapshot(),
        &logs,
        &config,
        &TuiView::default(),
      )
    })
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(rendered.contains("colored"), "missing log line: {rendered}");
  assert!(
    terminal.backend().buffer().content().iter().any(|cell| {
      cell.symbol() == "c"
        && cell.fg == Color::Magenta
        && cell.modifier.contains(Modifier::BOLD)
    }),
    "ANSI SGR was not converted into ratatui cell style"
  );
}
