use std::{
  io::{self, Write},
  process::{Child, ExitStatus},
  sync::atomic::Ordering,
  thread,
  time::Duration,
};

use crossterm::{
  cursor,
  event::{self, KeyCode, KeyEvent, KeyModifiers},
  execute,
  terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use super::{
  DEPENDENCY_POPULATE_BUDGET_PER_FRAME,
  MonitorShared,
  WrapperConfig,
  display_config,
  render_final_after_tui,
  run_streaming_render_loop,
  snapshot_logs,
};

struct TerminalSession {
  terminal: Terminal<CrosstermBackend<io::Stderr>>,
}

impl TerminalSession {
  fn enter() -> io::Result<Self> {
    let mut stderr = io::stderr();
    crossterm::terminal::enable_raw_mode()?;
    execute!(stderr, EnterAlternateScreen, cursor::Hide)?;

    let backend = CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(Self { terminal })
  }

  fn draw(
    &mut self,
    state: &rom_core::state::State,
    logs: &[String],
    config: &rom_core::tui::TuiConfig,
    view: &rom_core::tui::TuiView,
  ) -> io::Result<()> {
    self.terminal.draw(|frame| {
      rom_core::tui::draw(frame, state, logs, config, view);
    })?;
    Ok(())
  }
}

impl Drop for TerminalSession {
  fn drop(&mut self) {
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = execute!(
      self.terminal.backend_mut(),
      LeaveAlternateScreen,
      cursor::Show
    );
  }
}

#[derive(Default)]
struct TuiRuntime {
  view:         rom_core::tui::TuiView,
  frozen_state: Option<rom_core::state::State>,
  frozen_logs:  Vec<String>,
}

impl TuiRuntime {
  fn draw(
    &self,
    terminal: &mut TerminalSession,
    shared: &MonitorShared,
    silent: bool,
    config: &rom_core::tui::TuiConfig,
  ) -> io::Result<()> {
    if self.view.paused
      && let Some(state) = &self.frozen_state
    {
      return terminal.draw(state, &self.frozen_logs, config, &self.view);
    }

    let state = {
      let state = shared.state.lock().unwrap();
      state.render_snapshot()
    };
    let logs = snapshot_logs(shared, silent, Some(&self.view));
    terminal.draw(&state, &logs, config, &self.view)
  }

  fn toggle_pause(&mut self, shared: &MonitorShared, silent: bool) {
    if self.view.paused {
      self.view.paused = false;
      self.frozen_state = None;
      self.frozen_logs.clear();
      self.view.log_scroll = 0;
      return;
    }

    let state = {
      let state = shared.state.lock().unwrap();
      state.render_snapshot()
    };
    let logs = snapshot_logs(shared, silent, None);
    self.frozen_state = Some(state);
    self.frozen_logs = logs;
    self.view.paused = true;
  }
}

pub(super) fn run_tui_render_loop(
  child: &mut Child,
  shared: &MonitorShared,
  cfg: &WrapperConfig,
) -> eyre::Result<i32> {
  let Ok(mut terminal) = TerminalSession::enter() else {
    return run_streaming_render_loop(child, shared, cfg);
  };

  let tui_config = rom_core::tui::TuiConfig {
    display:        display_config(cfg, true),
    log_line_limit: cfg.log_lines,
  };
  let mut runtime = TuiRuntime::default();
  let mut status: Option<ExitStatus> = None;
  let mut cancelled_exit_code = None;

  loop {
    if let Some(exit_code) =
      handle_tui_events(child, shared, cfg, &mut runtime, &mut status)?
    {
      cancelled_exit_code = Some(exit_code);
      break;
    }

    if status.is_none() {
      status = child.try_wait().map_err(rom_core::error::RomError::Io)?;
    }

    populate_pending_dependencies(shared);

    runtime
      .draw(&mut terminal, shared, cfg.silent, &tui_config)
      .map_err(rom_core::error::RomError::Io)?;

    if status.is_some()
      && shared.stderr_done.load(Ordering::Acquire)
      && !runtime.view.paused
    {
      break;
    }

    thread::sleep(Duration::from_millis(100));
  }

  if let Some(exit_code) = cancelled_exit_code {
    drop(terminal);
    if exit_code == 130 {
      let _ = writeln!(io::stderr(), "rom: build cancelled");
    }
    return Ok(exit_code);
  }

  {
    let mut state = shared.state.lock().unwrap();
    let mut graph = shared.graph.lock().unwrap();
    if graph.populate_pending(&mut state, DEPENDENCY_POPULATE_BUDGET_PER_FRAME)
    {
      let now = rom_core::state::current_time();
      rom_core::update::maintain_state(&mut state, now);
    }
    rom_core::update::finish_state(&mut state);
  }
  {
    let state = shared.state.lock().unwrap();
    let logs = snapshot_logs(shared, cfg.silent, None);
    terminal
      .draw(&state, &logs, &tui_config, &runtime.view)
      .map_err(rom_core::error::RomError::Io)?;
  }
  drop(terminal);

  let exit_code = status.and_then(|status| status.code()).unwrap_or(1);
  render_final_after_tui(shared, cfg, exit_code)?;

  Ok(exit_code)
}

fn handle_tui_events(
  child: &mut Child,
  shared: &MonitorShared,
  cfg: &WrapperConfig,
  runtime: &mut TuiRuntime,
  status: &mut Option<ExitStatus>,
) -> eyre::Result<Option<i32>> {
  while event::poll(Duration::from_millis(0))
    .map_err(rom_core::error::RomError::Io)?
  {
    let event = event::read().map_err(rom_core::error::RomError::Io)?;
    if let Some(key) = event.as_key_press_event()
      && let Some(exit_code) =
        handle_tui_key(key, child, shared, cfg, runtime, status)?
    {
      return Ok(Some(exit_code));
    }
  }

  Ok(None)
}

fn handle_tui_key(
  key: KeyEvent,
  child: &mut Child,
  shared: &MonitorShared,
  cfg: &WrapperConfig,
  runtime: &mut TuiRuntime,
  status: &mut Option<ExitStatus>,
) -> eyre::Result<Option<i32>> {
  if key.modifiers.contains(KeyModifiers::CONTROL)
    && matches!(key.code, KeyCode::Char('c' | 'C'))
  {
    return cancel_child(child, status).map(Some);
  }

  if matches!(key.code, KeyCode::Char('q' | 'Q')) {
    return cancel_child(child, status).map(Some);
  }

  if matches!(key.code, KeyCode::Char(' '))
    && !key
      .modifiers
      .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
  {
    runtime.toggle_pause(shared, cfg.silent);
    return Ok(None);
  }

  if runtime.view.search_active {
    return handle_tui_search_key(key, runtime, child, status);
  }

  match key.code {
    KeyCode::Char('/') => {
      runtime.view.search_active = true;
      runtime.view.log_scroll = 0;
      Ok(None)
    },
    KeyCode::Esc => {
      runtime.view.search_active = false;
      runtime.view.search_query.clear();
      runtime.view.log_scroll = 0;
      Ok(None)
    },
    KeyCode::Char('w' | 'W') => {
      runtime.view.log_wrap = !runtime.view.log_wrap;
      Ok(None)
    },
    KeyCode::Up | KeyCode::Char('k' | 'K') => {
      scroll_logs_up(runtime, 1);
      Ok(None)
    },
    KeyCode::Down | KeyCode::Char('j' | 'J') => {
      scroll_logs_down(runtime, 1);
      Ok(None)
    },
    KeyCode::PageUp => {
      scroll_logs_up(runtime, 10);
      Ok(None)
    },
    KeyCode::PageDown => {
      scroll_logs_down(runtime, 10);
      Ok(None)
    },
    KeyCode::Home => {
      runtime.view.log_scroll = usize::MAX;
      Ok(None)
    },
    KeyCode::End => {
      runtime.view.log_scroll = 0;
      Ok(None)
    },
    _ => Ok(None),
  }
}

fn handle_tui_search_key(
  key: KeyEvent,
  runtime: &mut TuiRuntime,
  child: &mut Child,
  status: &mut Option<ExitStatus>,
) -> eyre::Result<Option<i32>> {
  match key.code {
    KeyCode::Esc | KeyCode::Enter => {
      runtime.view.search_active = false;
      Ok(None)
    },
    KeyCode::Backspace => {
      runtime.view.search_query.pop();
      runtime.view.log_scroll = 0;
      Ok(None)
    },
    KeyCode::Char('u' | 'U')
      if key.modifiers.contains(KeyModifiers::CONTROL) =>
    {
      runtime.view.search_query.clear();
      runtime.view.log_scroll = 0;
      Ok(None)
    },
    KeyCode::Up => {
      scroll_logs_up(runtime, 1);
      Ok(None)
    },
    KeyCode::Down => {
      scroll_logs_down(runtime, 1);
      Ok(None)
    },
    KeyCode::PageUp => {
      scroll_logs_up(runtime, 10);
      Ok(None)
    },
    KeyCode::PageDown => {
      scroll_logs_down(runtime, 10);
      Ok(None)
    },
    KeyCode::Home => {
      runtime.view.log_scroll = usize::MAX;
      Ok(None)
    },
    KeyCode::End => {
      runtime.view.log_scroll = 0;
      Ok(None)
    },
    KeyCode::Char(ch)
      if !key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
    {
      runtime.view.search_query.push(ch);
      runtime.view.log_scroll = 0;
      Ok(None)
    },
    KeyCode::Char('c' | 'C')
      if key.modifiers.contains(KeyModifiers::CONTROL) =>
    {
      cancel_child(child, status).map(Some)
    },
    _ => Ok(None),
  }
}

fn scroll_logs_up(runtime: &mut TuiRuntime, amount: usize) {
  runtime.view.log_scroll = runtime.view.log_scroll.saturating_add(amount);
}

fn scroll_logs_down(runtime: &mut TuiRuntime, amount: usize) {
  runtime.view.log_scroll = runtime.view.log_scroll.saturating_sub(amount);
}

fn cancel_child(
  child: &mut Child,
  status: &mut Option<ExitStatus>,
) -> eyre::Result<i32> {
  if let Some(status) = status.as_ref() {
    return Ok(status.code().unwrap_or(1));
  }

  if let Some(done) = child.try_wait().map_err(rom_core::error::RomError::Io)? {
    let exit_code = done.code().unwrap_or(1);
    *status = Some(done);
    return Ok(exit_code);
  }

  child.kill().map_err(rom_core::error::RomError::Io)?;
  let killed = child.wait().map_err(rom_core::error::RomError::Io)?;
  *status = Some(killed);
  Ok(130)
}

fn populate_pending_dependencies(shared: &MonitorShared) {
  let mut state = shared.state.lock().unwrap();
  let mut graph = shared.graph.lock().unwrap();
  if graph.populate_pending(&mut state, DEPENDENCY_POPULATE_BUDGET_PER_FRAME) {
    let now = rom_core::state::current_time();
    rom_core::update::maintain_state(&mut state, now);
  }
}
