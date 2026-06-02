use std::collections::HashSet;

use ratatui::{
  Terminal,
  backend::TestBackend,
  style::{Color, Modifier},
};
use rom_core::{
  display::{DisplayConfig, render_state_lines},
  icons,
  state::{
    BuildInfo,
    BuildStatus,
    Derivation,
    DerivationId,
    FailType,
    InputDerivation,
    State,
    StorePath,
    StorePathId,
    TransferInfo,
    current_time,
  },
  tui::{TuiConfig, TuiView, draw},
  types::{DisplayFormat, LegendStyle, SummaryStyle},
};

const GRAPH_LINE_COLOR: Color = Color::Rgb(82, 89, 78);
const MOSS_GREEN: Color = Color::Rgb(158, 190, 112);
const MUTED_RED: Color = Color::Rgb(204, 102, 96);
const MUTED_YELLOW: Color = Color::Rgb(224, 190, 96);

fn tui_config() -> TuiConfig {
  TuiConfig {
    display:        DisplayConfig {
      use_color: true,
      format: DisplayFormat::Tree,
      legend_style: LegendStyle::Table,
      summary_style: SummaryStyle::Concise,
      icons: &icons::UNICODE,
      ..DisplayConfig::default()
    },
    log_line_limit: Some(8),
  }
}

fn running_state() -> State {
  let mut state = State::new();
  let drv_id = add_derivation(&mut state, "hello-1.0");
  state.update_build_status(
    drv_id,
    BuildStatus::Building(BuildInfo {
      start:       current_time(),
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.forest_roots.push(drv_id);
  state
}

fn add_derivation(state: &mut State, name: &str) -> DerivationId {
  let drv = Derivation::parse(&format!(
    "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-{name}.drv"
  ))
  .unwrap();
  state.get_or_create_derivation_id(drv)
}

fn add_output_path(
  state: &mut State,
  drv_id: DerivationId,
  name: &str,
) -> StorePathId {
  let path_id = add_store_path(state, name);
  state.get_store_path_info_mut(path_id).unwrap().producer = Some(drv_id);
  state
    .get_derivation_info_mut(drv_id)
    .unwrap()
    .outputs
    .insert(cognos::OutputName::parse("out"), path_id);
  path_id
}

fn add_store_path(state: &mut State, name: &str) -> StorePathId {
  let path = StorePath::parse(&format!(
    "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-{name}"
  ))
  .unwrap();
  state.get_or_create_store_path_id(path)
}

fn row_text(terminal: &Terminal<TestBackend>, row: u16) -> String {
  let buffer = terminal.backend().buffer();
  (0..buffer.area.width)
    .map(|x| buffer[(x, row)].symbol())
    .collect()
}

fn row_containing(
  terminal: &Terminal<TestBackend>,
  needle: &str,
) -> Option<u16> {
  let buffer = terminal.backend().buffer();
  (0..buffer.area.height).find(|row| row_text(terminal, *row).contains(needle))
}

#[test]
fn tui_draws_graph_and_logs_without_header() {
  let backend = TestBackend::new(80, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let state = running_state();
  let logs = vec!["builder log line".to_string()];
  let config = tui_config();

  terminal
    .draw(|frame| draw(frame, &state, &logs, &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    !rendered.contains("ROM"),
    "top header still rendered: {rendered}"
  );
  assert!(
    rendered.contains("Build Graph"),
    "missing graph pane: {rendered}"
  );
  assert!(rendered.contains("hello-1.0"), "missing build: {rendered}");
  assert!(rendered.contains("Logs"), "missing logs pane: {rendered}");
  assert!(
    rendered.contains("builder log line"),
    "missing log line: {rendered}"
  );

  let graph_title_row = row_text(&terminal, 1);
  assert!(
    graph_title_row.contains("Build Graph"),
    "missing graph title row: {graph_title_row:?}"
  );
  assert!(
    !graph_title_row.contains("┌") && !graph_title_row.contains("┐"),
    "graph pane should use a top separator, not a boxed border: \
     {graph_title_row:?}"
  );
  let graph_content_row = row_text(&terminal, 2);
  assert!(
    !graph_content_row.starts_with("│"),
    "graph content should not have a left border wall: {graph_content_row:?}"
  );
}

#[test]
fn tui_renders_running_build_as_devenv_style_activity() {
  let backend = TestBackend::new(80, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let state = running_state();
  let logs = Vec::new();
  let config = tui_config();

  terminal
    .draw(|frame| draw(frame, &state, &logs, &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    !rendered.contains("Building"),
    "build state should be color-coded instead of text-labeled: {rendered}"
  );
  assert!(
    !rendered.contains("❧"),
    "old fleuron glyph should not remain: {rendered}"
  );
  assert!(
    !rendered.contains("◄") && !rendered.contains("►"),
    "old arrow glyphs should not remain: {rendered}"
  );
  assert!(
    !rendered.contains("╭")
      && !rendered.contains("╰")
      && !rendered.contains("┤"),
    "old leaf box glyphs should not remain: {rendered}"
  );
  assert!(
    rendered.contains("hello-1.0"),
    "missing build name: {rendered}"
  );
  assert!(
    !rendered.contains("Dependency Graph"),
    "old graph header should not remain in vine view: {rendered}"
  );
}

#[test]
fn tui_shows_evaluation_progress_before_build_graph_exists() {
  let backend = TestBackend::new(80, 16);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  state.evaluation_state.count = 42;
  state.evaluation_state.last_file_name = Some(
    "«nixpkgs»/pkgs/by-name/bc/bcachefs-tools/kernel-module.nix".to_string(),
  );
  let config = tui_config();

  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("Evaluating"),
    "evaluation status should replace idle graph text: {rendered}"
  );
  assert!(
    rendered.contains("bcachefs-tools/kernel-module.nix"),
    "evaluation status should show the current file tail: {rendered}"
  );
  assert!(
    rendered.contains("42 files"),
    "evaluation status should show the evaluation count: {rendered}"
  );
  assert!(
    !rendered.contains("Waiting for Nix activity"),
    "evaluation should not look idle: {rendered}"
  );
}

#[test]
fn tui_renders_multiple_running_builds_as_activity_rows() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let first_id = add_derivation(&mut state, "first-1.0");
  let second_id = add_derivation(&mut state, "second-1.0");

  for drv_id in [first_id, second_id] {
    state.update_build_status(
      drv_id,
      BuildStatus::Building(BuildInfo {
        start:       current_time(),
        host:        cognos::Host::Localhost,
        estimate:    None,
        activity_id: None,
      }),
    );
    state.forest_roots.push(drv_id);
  }
  let config = tui_config();

  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("first-1.0"),
    "first build should render as an activity row: {rendered}"
  );
  assert!(
    rendered.contains("second-1.0"),
    "second build should render as an activity row: {rendered}"
  );
  assert!(
    !rendered.contains("╭") && !rendered.contains("╰"),
    "devenv-style graph should not use decorative vine boxes: {rendered}"
  );
}

#[test]
fn tui_renders_dependency_branch_with_active_leaf() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let child_id = add_derivation(&mut state, "child-1.0");
  state
    .get_derivation_info_mut(root_id)
    .unwrap()
    .input_derivations
    .push(InputDerivation {
      derivation: child_id,
      outputs:    HashSet::new(),
    });
  state
    .get_derivation_info_mut(child_id)
    .unwrap()
    .derivation_parents
    .insert(root_id);
  state.update_build_status(root_id, BuildStatus::Planned);
  state.update_build_status(
    child_id,
    BuildStatus::Building(BuildInfo {
      start:       current_time(),
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.forest_roots.push(root_id);
  let config = tui_config();

  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("┌─"),
    "missing hierarchy connector: {rendered}"
  );
  assert!(
    !rendered.contains("╭") && !rendered.contains("╰"),
    "graph should use connected square guide rails instead of decorative \
     rounded vines: {rendered}"
  );
  assert!(
    rendered.contains("child-1.0"),
    "missing active dependency leaf: {rendered}"
  );
  assert!(
    rendered.contains("root-1.0"),
    "missing planned root bud: {rendered}"
  );
  let child_row =
    row_containing(&terminal, "child-1.0").expect("child row should render");
  let root_row =
    row_containing(&terminal, "root-1.0").expect("root row should render");
  assert!(
    child_row < root_row,
    "dependency should render above root: child={child_row} root={root_row}"
  );
}

#[test]
fn tui_keeps_root_visible_when_activity_graph_overflows() {
  let backend = TestBackend::new(80, 12);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "nixos-system-fool");
  state.update_build_status(root_id, BuildStatus::Planned);

  for index in 0..8 {
    let child_id = add_derivation(&mut state, &format!("dep-{index:02}"));
    state
      .get_derivation_info_mut(root_id)
      .unwrap()
      .input_derivations
      .push(InputDerivation {
        derivation: child_id,
        outputs:    HashSet::new(),
      });
    state
      .get_derivation_info_mut(child_id)
      .unwrap()
      .derivation_parents
      .insert(root_id);
    state.update_build_status(
      child_id,
      BuildStatus::Building(BuildInfo {
        start:       current_time(),
        host:        cognos::Host::Localhost,
        estimate:    None,
        activity_id: None,
      }),
    );
  }

  state.forest_roots.push(root_id);
  let config = tui_config();

  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("nixos-system-fool"),
    "overflowing graph should keep the requested root visible: {rendered}"
  );
  assert!(
    rendered.contains("dep-07"),
    "overflowing graph should still bottom-align active leaves: {rendered}"
  );
  assert!(
    rendered.contains("hidden rows above"),
    "overflowing graph should advertise clipped rows: {rendered}"
  );
}

#[test]
fn tui_uses_thin_connectors_for_dependency_siblings() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let first_id = add_derivation(&mut state, "first-dep-1.0");
  let middle_id = add_derivation(&mut state, "middle-dep-1.0");
  let last_id = add_derivation(&mut state, "last-dep-1.0");

  state.update_build_status(root_id, BuildStatus::Planned);
  for child_id in [first_id, middle_id, last_id] {
    state
      .get_derivation_info_mut(root_id)
      .unwrap()
      .input_derivations
      .push(InputDerivation {
        derivation: child_id,
        outputs:    HashSet::new(),
      });
    state
      .get_derivation_info_mut(child_id)
      .unwrap()
      .derivation_parents
      .insert(root_id);
    state.update_build_status(
      child_id,
      BuildStatus::Building(BuildInfo {
        start:       current_time(),
        host:        cognos::Host::Localhost,
        estimate:    None,
        activity_id: None,
      }),
    );
  }
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let first_row = row_text(
    &terminal,
    row_containing(&terminal, "first-dep-1.0").unwrap(),
  );
  let middle_row = row_text(
    &terminal,
    row_containing(&terminal, "middle-dep-1.0").unwrap(),
  );
  let last_row = row_text(
    &terminal,
    row_containing(&terminal, "last-dep-1.0").unwrap(),
  );
  let root_row =
    row_text(&terminal, row_containing(&terminal, "root-1.0").unwrap());

  assert!(
    first_row.contains("┌─"),
    "first dependency should start the branch: {first_row:?}"
  );
  assert!(
    middle_row.contains("├─"),
    "middle dependency should use a true intersection: {middle_row:?}"
  );
  assert!(
    last_row.contains("├─"),
    "last dependency should keep the branch open for the root: {last_row:?}"
  );
  assert!(
    root_row.contains("└─"),
    "root should close the dependency branch at the bottom: {root_row:?}"
  );
}

#[test]
fn tui_joins_visible_dependency_branch_into_parent() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let parent_id = add_derivation(&mut state, "parent-1.0");
  let child_id = add_derivation(&mut state, "child-1.0");

  for (parent, child) in [(root_id, parent_id), (parent_id, child_id)] {
    state
      .get_derivation_info_mut(parent)
      .unwrap()
      .input_derivations
      .push(InputDerivation {
        derivation: child,
        outputs:    HashSet::new(),
      });
    state
      .get_derivation_info_mut(child)
      .unwrap()
      .derivation_parents
      .insert(parent);
  }

  for drv_id in [root_id, parent_id] {
    state.update_build_status(drv_id, BuildStatus::Planned);
  }
  state.update_build_status(
    child_id,
    BuildStatus::Building(BuildInfo {
      start:       current_time(),
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let parent_row =
    row_text(&terminal, row_containing(&terminal, "parent-1.0").unwrap());
  let child_row =
    row_text(&terminal, row_containing(&terminal, "child-1.0").unwrap());
  let root_row =
    row_text(&terminal, row_containing(&terminal, "root-1.0").unwrap());
  assert!(
    child_row.starts_with("  ┌─"),
    "dependency rows should not inherit a left-edge ancestor rail: \
     {child_row:?}"
  );
  assert!(
    parent_row.contains("┌─┴─"),
    "parent row should join its visible dependency rail: {parent_row:?}"
  );
  assert!(
    root_row.contains("└─"),
    "root row should close the visible branch: {root_row:?}"
  );
}

#[test]
fn tui_keeps_sibling_rail_through_nested_active_subtree() {
  let backend = TestBackend::new(96, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let direct_id = add_derivation(&mut state, "direct-build-1.0");
  let parent_id = add_derivation(&mut state, "parent-1.0");
  let nested_id = add_derivation(&mut state, "nested-build-1.0");

  for child_id in [direct_id, parent_id] {
    state
      .get_derivation_info_mut(root_id)
      .unwrap()
      .input_derivations
      .push(InputDerivation {
        derivation: child_id,
        outputs:    HashSet::new(),
      });
    state
      .get_derivation_info_mut(child_id)
      .unwrap()
      .derivation_parents
      .insert(root_id);
  }
  state
    .get_derivation_info_mut(parent_id)
    .unwrap()
    .input_derivations
    .push(InputDerivation {
      derivation: nested_id,
      outputs:    HashSet::new(),
    });
  state
    .get_derivation_info_mut(nested_id)
    .unwrap()
    .derivation_parents
    .insert(parent_id);

  for drv_id in [root_id, parent_id] {
    state.update_build_status(drv_id, BuildStatus::Planned);
  }
  for drv_id in [direct_id, nested_id] {
    state.update_build_status(
      drv_id,
      BuildStatus::Building(BuildInfo {
        start:       current_time(),
        host:        cognos::Host::Localhost,
        estimate:    None,
        activity_id: None,
      }),
    );
  }
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let nested_row = row_text(
    &terminal,
    row_containing(&terminal, "nested-build-1.0").unwrap(),
  );
  assert!(
    nested_row.starts_with("│ ┌─"),
    "nested active subtrees should keep the root sibling rail connected: \
     {nested_row:?}"
  );
}

#[test]
fn tui_removes_dangling_left_rail_from_nested_sibling_subtrees() {
  let backend = TestBackend::new(96, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "nixos-system-fool");
  let system_path_id = add_derivation(&mut state, "system-path");
  let portal_id = add_derivation(&mut state, "xdg-desktop-portal-kde-6.6.5");
  let plasma_id = add_derivation(&mut state, "plasma-workspace-6.6.5");
  let bitwarden_wrapped_id =
    add_derivation(&mut state, "bitwarden-desktop-wrapped");
  let bitwarden_id = add_derivation(&mut state, "bitwarden-desktop-2026.5.0");

  for (parent, child) in [
    (root_id, system_path_id),
    (system_path_id, portal_id),
    (portal_id, plasma_id),
    (system_path_id, bitwarden_wrapped_id),
    (bitwarden_wrapped_id, bitwarden_id),
  ] {
    state
      .get_derivation_info_mut(parent)
      .unwrap()
      .input_derivations
      .push(InputDerivation {
        derivation: child,
        outputs:    HashSet::new(),
      });
    state
      .get_derivation_info_mut(child)
      .unwrap()
      .derivation_parents
      .insert(parent);
  }

  for drv_id in [root_id, system_path_id, portal_id, bitwarden_wrapped_id] {
    state.update_build_status(drv_id, BuildStatus::Planned);
  }
  for drv_id in [plasma_id, bitwarden_id] {
    state.update_build_status(
      drv_id,
      BuildStatus::Building(BuildInfo {
        start:       current_time() - 2.0,
        host:        cognos::Host::Localhost,
        estimate:    None,
        activity_id: None,
      }),
    );
  }
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let plasma_row = row_text(
    &terminal,
    row_containing(&terminal, "plasma-workspace-6.6.5").unwrap(),
  );
  assert!(
    !plasma_row.starts_with("│ "),
    "nested sibling subtree should not show a dangling left rail: \
     {plasma_row:?}"
  );
}

#[test]
fn tui_prefers_structural_parent_for_shared_active_dependency() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let active_id = add_derivation(&mut state, "active-leaf-1.0");
  let aggregator_id = add_derivation(&mut state, "aggregator-1.0");

  for child_id in [active_id, aggregator_id] {
    state
      .get_derivation_info_mut(root_id)
      .unwrap()
      .input_derivations
      .push(InputDerivation {
        derivation: child_id,
        outputs:    HashSet::new(),
      });
    state
      .get_derivation_info_mut(child_id)
      .unwrap()
      .derivation_parents
      .insert(root_id);
  }
  state
    .get_derivation_info_mut(aggregator_id)
    .unwrap()
    .input_derivations
    .push(InputDerivation {
      derivation: active_id,
      outputs:    HashSet::new(),
    });
  state
    .get_derivation_info_mut(active_id)
    .unwrap()
    .derivation_parents
    .insert(aggregator_id);

  state.update_build_status(root_id, BuildStatus::Planned);
  state.update_build_status(aggregator_id, BuildStatus::Planned);
  state.update_build_status(
    active_id,
    BuildStatus::Building(BuildInfo {
      start:       current_time(),
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  let active_row = row_text(
    &terminal,
    row_containing(&terminal, "active-leaf-1.0").unwrap(),
  );
  let aggregator_row = row_text(
    &terminal,
    row_containing(&terminal, "aggregator-1.0").unwrap(),
  );
  assert!(
    active_row.starts_with("  ┌─"),
    "shared active dependency should render under its structural parent: \
     {active_row:?}"
  );
  assert!(
    aggregator_row.contains("┌─┴─"),
    "structural parent should own the active dependency branch: \
     {aggregator_row:?}"
  );
  assert!(
    rendered.contains("root-1.0 1 shared"),
    "direct duplicate should collapse into the root summary: {rendered}"
  );
  assert_eq!(
    rendered.matches("active-leaf-1.0").count(),
    1,
    "active dependency should render only once: {rendered}"
  );
}

#[test]
fn tui_renders_running_downloads_inline_in_dependency_graph() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let downloaded_id = add_derivation(&mut state, "electron-41.7.1");
  let path_id = add_output_path(&mut state, downloaded_id, "electron-41.7.1");

  state
    .get_derivation_info_mut(root_id)
    .unwrap()
    .input_derivations
    .push(InputDerivation {
      derivation: downloaded_id,
      outputs:    HashSet::new(),
    });
  state
    .get_derivation_info_mut(downloaded_id)
    .unwrap()
    .derivation_parents
    .insert(root_id);

  state.update_build_status(root_id, BuildStatus::Planned);
  state.update_build_status(downloaded_id, BuildStatus::Planned);
  state
    .full_summary
    .running_downloads
    .insert(path_id, TransferInfo {
      start:             current_time() - 2.0,
      host:              cognos::Host::Localhost,
      activity_id:       42,
      bytes_transferred: 512,
      total_bytes:       Some(1024),
    });
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  let download_row = row_text(
    &terminal,
    row_containing(&terminal, "electron-41.7.1").unwrap(),
  );
  assert!(
    download_row.contains("↓"),
    "running substitute should render as an inline download row: \
     {download_row:?}"
  );
  assert!(
    download_row.contains("512 B / 1.0 KiB"),
    "running substitute should show transfer progress: {download_row:?}"
  );
  assert!(
    rendered.contains("Downloads"),
    "download should still be counted in the header: {rendered}"
  );
}

#[test]
fn tui_renders_downloads_inline_by_store_path_name_without_outputs() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let downloaded_id = add_derivation(&mut state, "electron-39.8.10");
  let path_id = add_store_path(&mut state, "electron-39.8.10");

  state
    .get_derivation_info_mut(root_id)
    .unwrap()
    .input_derivations
    .push(InputDerivation {
      derivation: downloaded_id,
      outputs:    HashSet::new(),
    });
  state
    .get_derivation_info_mut(downloaded_id)
    .unwrap()
    .derivation_parents
    .insert(root_id);

  state.update_build_status(root_id, BuildStatus::Planned);
  state.update_build_status(downloaded_id, BuildStatus::Planned);
  state
    .full_summary
    .running_downloads
    .insert(path_id, TransferInfo {
      start:             current_time() - 2.0,
      host:              cognos::Host::Localhost,
      activity_id:       42,
      bytes_transferred: 512,
      total_bytes:       Some(1024),
    });
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let download_row = row_text(
    &terminal,
    row_containing(&terminal, "electron-39.8.10").unwrap(),
  );
  assert!(
    download_row.contains("↓"),
    "download should attach to the planned derivation by store path name when \
     output metadata has not been parsed: {download_row:?}"
  );
}

#[test]
fn tui_renders_unmatched_downloads_as_standalone_activity_rows() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let path_id = add_store_path(&mut state, "source-tarball-1.0");

  state
    .full_summary
    .running_downloads
    .insert(path_id, TransferInfo {
      start:             current_time() - 2.0,
      host:              cognos::Host::Localhost,
      activity_id:       42,
      bytes_transferred: 512,
      total_bytes:       Some(1024),
    });

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let download_row = row_text(
    &terminal,
    row_containing(&terminal, "source-tarball-1.0").unwrap(),
  );
  assert!(
    download_row.contains("↓"),
    "substitute-only downloads should not disappear when no derivation node \
     is known yet: {download_row:?}"
  );
}

#[test]
fn tui_renders_unmatched_downloads_from_render_snapshot() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let path_id = add_store_path(&mut state, "source-tarball-1.0");

  state
    .full_summary
    .running_downloads
    .insert(path_id, TransferInfo {
      start:             current_time() - 2.0,
      host:              cognos::Host::Localhost,
      activity_id:       42,
      bytes_transferred: 512,
      total_bytes:       Some(1024),
    });

  let snapshot = state.render_snapshot();
  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &snapshot, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("source-tarball-1.0"),
    "render snapshots should retain active download path names: {rendered}"
  );
}

#[test]
fn tui_keeps_unrendered_downloads_visible_when_tree_is_truncated() {
  let backend = TestBackend::new(80, 18);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let middle_id = add_derivation(&mut state, "middle-1.0");
  let active_id = add_derivation(&mut state, "active-leaf-1.0");
  let path_id = add_store_path(&mut state, "electron-unwrapped-41.7.1");

  state
    .get_derivation_info_mut(root_id)
    .unwrap()
    .input_derivations
    .push(InputDerivation {
      derivation: middle_id,
      outputs:    HashSet::new(),
    });
  state
    .get_derivation_info_mut(middle_id)
    .unwrap()
    .input_derivations
    .push(InputDerivation {
      derivation: active_id,
      outputs:    HashSet::new(),
    });
  state
    .get_derivation_info_mut(middle_id)
    .unwrap()
    .derivation_parents
    .insert(root_id);
  state
    .get_derivation_info_mut(active_id)
    .unwrap()
    .derivation_parents
    .insert(middle_id);

  state.update_build_status(root_id, BuildStatus::Planned);
  state.update_build_status(middle_id, BuildStatus::Planned);
  state.update_build_status(
    active_id,
    BuildStatus::Building(BuildInfo {
      start:       current_time() - 2.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: Some(7),
    }),
  );
  state
    .full_summary
    .running_downloads
    .insert(path_id, TransferInfo {
      start:             current_time() - 2.0,
      host:              cognos::Host::Localhost,
      activity_id:       42,
      bytes_transferred: 512,
      total_bytes:       Some(1024),
    });
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("electron-unwrapped-41.7.1"),
    "active downloads should keep a visible row even when the dependency tree \
     is taller than the graph pane: {rendered}"
  );
}

#[test]
fn tui_falls_back_when_inline_download_row_is_truncated() {
  let backend = TestBackend::new(80, 18);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let parent_id = add_derivation(&mut state, "parent-1.0");
  let middle_id = add_derivation(&mut state, "middle-1.0");
  let downloaded_id = add_derivation(&mut state, "electron-unwrapped-41.7.1");
  let path_id =
    add_output_path(&mut state, downloaded_id, "electron-unwrapped-41.7.1");

  for (parent, child) in [
    (root_id, parent_id),
    (parent_id, middle_id),
    (middle_id, downloaded_id),
  ] {
    state
      .get_derivation_info_mut(parent)
      .unwrap()
      .input_derivations
      .push(InputDerivation {
        derivation: child,
        outputs:    HashSet::new(),
      });
    state
      .get_derivation_info_mut(child)
      .unwrap()
      .derivation_parents
      .insert(parent);
  }

  for drv_id in [root_id, parent_id, middle_id, downloaded_id] {
    state.update_build_status(drv_id, BuildStatus::Planned);
  }
  state
    .full_summary
    .running_downloads
    .insert(path_id, TransferInfo {
      start:             current_time() - 2.0,
      host:              cognos::Host::Localhost,
      activity_id:       42,
      bytes_transferred: 512,
      total_bytes:       Some(1024),
    });
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let download_row = row_text(
    &terminal,
    row_containing(&terminal, "electron-unwrapped-41.7.1").unwrap(),
  );
  assert!(
    download_row.contains("↓"),
    "downloads whose inline tree row was truncated should get a fallback row: \
     {download_row:?}"
  );
}

#[test]
fn tui_renders_name_matched_downloads_when_derivation_is_not_in_tree() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let _orphan_id = add_derivation(&mut state, "electron-unwrapped-41.7.1");
  let path_id = add_store_path(&mut state, "electron-unwrapped-41.7.1");

  state.update_build_status(root_id, BuildStatus::Planned);
  state
    .full_summary
    .running_downloads
    .insert(path_id, TransferInfo {
      start:             current_time() - 2.0,
      host:              cognos::Host::Localhost,
      activity_id:       42,
      bytes_transferred: 512,
      total_bytes:       Some(1024),
    });
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let download_row = row_text(
    &terminal,
    row_containing(&terminal, "electron-unwrapped-41.7.1").unwrap(),
  );
  assert!(
    download_row.contains("↓"),
    "name-matched downloads should fall back to a standalone row when their \
     derivation node was not rendered: {download_row:?}"
  );
}

#[test]
fn tui_caps_standalone_downloads_so_dependency_tree_stays_visible() {
  let backend = TestBackend::new(100, 32);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let middle_id = add_derivation(&mut state, "middle-1.0");
  let active_id = add_derivation(&mut state, "active-leaf-1.0");

  for (parent, child) in [(root_id, middle_id), (middle_id, active_id)] {
    state
      .get_derivation_info_mut(parent)
      .unwrap()
      .input_derivations
      .push(InputDerivation {
        derivation: child,
        outputs:    HashSet::new(),
      });
    state
      .get_derivation_info_mut(child)
      .unwrap()
      .derivation_parents
      .insert(parent);
  }

  state.update_build_status(root_id, BuildStatus::Planned);
  state.update_build_status(middle_id, BuildStatus::Planned);
  state.update_build_status(
    active_id,
    BuildStatus::Building(BuildInfo {
      start:       current_time() - 2.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: Some(7),
    }),
  );
  state.forest_roots.push(root_id);

  for index in 0..24 {
    let path_id = add_store_path(&mut state, &format!("download-only-{index}"));
    state
      .full_summary
      .running_downloads
      .insert(path_id, TransferInfo {
        start:             current_time() - 2.0,
        host:              cognos::Host::Localhost,
        activity_id:       100 + index,
        bytes_transferred: 512,
        total_bytes:       Some(1024),
      });
  }

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("active-leaf-1.0")
      && rendered.contains("middle-1.0")
      && rendered.contains("root-1.0"),
    "standalone downloads should not crowd out the dependency tree: {rendered}"
  );

  let download_rows = (0..terminal.backend().buffer().area.height)
    .filter(|row| row_text(&terminal, *row).contains("download-only-"))
    .count();
  assert!(
    download_rows <= 6,
    "standalone downloads should be capped, not fill the graph pane: \
     {rendered}"
  );
}

#[test]
fn tui_renders_planned_downloads_inline_in_dependency_graph() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let downloaded_id = add_derivation(&mut state, "signal-desktop-8.9.1");
  let path_id =
    add_output_path(&mut state, downloaded_id, "signal-desktop-8.9.1");

  state
    .get_derivation_info_mut(root_id)
    .unwrap()
    .input_derivations
    .push(InputDerivation {
      derivation: downloaded_id,
      outputs:    HashSet::new(),
    });
  state
    .get_derivation_info_mut(downloaded_id)
    .unwrap()
    .derivation_parents
    .insert(root_id);

  state.update_build_status(root_id, BuildStatus::Planned);
  state.update_build_status(downloaded_id, BuildStatus::Planned);
  state.full_summary.planned_downloads.insert(path_id);
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let download_row = row_text(
    &terminal,
    row_containing(&terminal, "signal-desktop-8.9.1").unwrap(),
  );
  assert!(
    download_row.contains("↓"),
    "planned substitute should render as an inline download row: \
     {download_row:?}"
  );
}

#[test]
fn tui_prefers_active_subtrees_near_graph_root() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let waiting_id = add_derivation(&mut state, "waiting-only-1.0");
  let active_parent_id = add_derivation(&mut state, "active-parent-1.0");
  let active_leaf_id = add_derivation(&mut state, "active-leaf-1.0");

  for child_id in [waiting_id, active_parent_id] {
    state
      .get_derivation_info_mut(root_id)
      .unwrap()
      .input_derivations
      .push(InputDerivation {
        derivation: child_id,
        outputs:    HashSet::new(),
      });
    state
      .get_derivation_info_mut(child_id)
      .unwrap()
      .derivation_parents
      .insert(root_id);
  }
  state
    .get_derivation_info_mut(active_parent_id)
    .unwrap()
    .input_derivations
    .push(InputDerivation {
      derivation: active_leaf_id,
      outputs:    HashSet::new(),
    });
  state
    .get_derivation_info_mut(active_leaf_id)
    .unwrap()
    .derivation_parents
    .insert(active_parent_id);

  for drv_id in [root_id, waiting_id, active_parent_id] {
    state.update_build_status(drv_id, BuildStatus::Planned);
  }
  state.update_build_status(
    active_leaf_id,
    BuildStatus::Building(BuildInfo {
      start:       current_time(),
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let active_row = row_containing(&terminal, "active-leaf-1.0")
    .expect("active subtree should render");
  let root_row =
    row_containing(&terminal, "root-1.0").expect("root should render");
  let rendered = format!("{}", terminal.backend());
  assert!(
    !rendered.contains("waiting-only-1.0"),
    "inactive waiting siblings should collapse into the root summary: \
     {rendered}"
  );
  assert!(
    rendered.contains("root-1.0 1 waiting"),
    "root should summarize the collapsed waiting blocker: {rendered}"
  );
  assert!(
    active_row < root_row,
    "active subtree should remain connected above the root: \
     active={active_row} root={root_row}"
  );
}

#[test]
fn tui_collapses_waiting_only_subtrees() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let parent_id = add_derivation(&mut state, "waiting-parent-1.0");
  let child_id = add_derivation(&mut state, "waiting-child-1.0");
  let leaf_id = add_derivation(&mut state, "waiting-leaf-1.0");

  for (parent, child) in [
    (root_id, parent_id),
    (parent_id, child_id),
    (child_id, leaf_id),
  ] {
    state
      .get_derivation_info_mut(parent)
      .unwrap()
      .input_derivations
      .push(InputDerivation {
        derivation: child,
        outputs:    HashSet::new(),
      });
    state
      .get_derivation_info_mut(child)
      .unwrap()
      .derivation_parents
      .insert(parent);
  }

  for drv_id in [root_id, parent_id, child_id, leaf_id] {
    state.update_build_status(drv_id, BuildStatus::Planned);
  }
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("root-1.0 3 waiting"),
    "waiting-only subtree should collapse into the nearest visible root: \
     {rendered}"
  );
  assert!(
    !rendered.contains("waiting-parent-1.0")
      && !rendered.contains("waiting-child-1.0")
      && !rendered.contains("waiting-leaf-1.0"),
    "collapsed waiting-only descendants should not render as separate rows: \
     {rendered}"
  );
}

#[test]
fn tui_limits_large_planned_root_sets() {
  let backend = TestBackend::new(80, 12);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();

  for index in 0..100 {
    let drv_id = add_derivation(&mut state, &format!("planned-{index:03}"));
    state.update_build_status(drv_id, BuildStatus::Planned);
    state.forest_roots.push(drv_id);
  }
  let config = tui_config();

  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("planned-000"),
    "large root sets should retain the first/highest-priority root: {rendered}"
  );
  assert!(
    rendered.contains("planned-099"),
    "large root sets should retain the tail roots: {rendered}"
  );
  assert!(
    !rendered.contains("planned-050"),
    "large root sets should not render every planned root: {rendered}"
  );
}

#[test]
fn tui_collapses_old_completed_dependencies_into_parent_summary() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let completed_id = add_derivation(&mut state, "completed-dep-1.0");
  state
    .get_derivation_info_mut(root_id)
    .unwrap()
    .input_derivations
    .push(InputDerivation {
      derivation: completed_id,
      outputs:    HashSet::new(),
    });
  state
    .get_derivation_info_mut(completed_id)
    .unwrap()
    .derivation_parents
    .insert(root_id);

  let now = current_time();
  state.update_build_status(root_id, BuildStatus::Planned);
  state.update_build_status(completed_id, BuildStatus::Built {
    info: BuildInfo {
      start:       now - 10.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    },
    end:  now - 6.0,
  });
  state.forest_roots.push(root_id);
  let config = tui_config();

  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("root-1.0 1 dep built"),
    "old completed dependency should collapse into parent suffix: {rendered}"
  );
  assert!(
    !rendered.contains("○"),
    "waiting rows should be color-coded instead of using a marker: {rendered}"
  );
  assert!(
    !rendered.contains("completed-dep-1.0"),
    "old completed dependency should not render as its own row: {rendered}"
  );
}

#[test]
fn tui_collapses_shared_dependency_references_into_parent_summary() {
  let backend = TestBackend::new(80, 28);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let first_root_id = add_derivation(&mut state, "first-root-1.0");
  let second_root_id = add_derivation(&mut state, "second-root-1.0");
  let shared_id = add_derivation(&mut state, "shared-dep-1.0");

  for root_id in [first_root_id, second_root_id] {
    state
      .get_derivation_info_mut(root_id)
      .unwrap()
      .input_derivations
      .push(InputDerivation {
        derivation: shared_id,
        outputs:    HashSet::new(),
      });
    state
      .get_derivation_info_mut(shared_id)
      .unwrap()
      .derivation_parents
      .insert(root_id);
    state.update_build_status(root_id, BuildStatus::Planned);
    state.forest_roots.push(root_id);
  }

  state.update_build_status(
    shared_id,
    BuildStatus::Building(BuildInfo {
      start:       current_time(),
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  let config = tui_config();

  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("shared-dep-1.0"),
    "shared dependency should render fully once: {rendered}"
  );
  assert!(
    rendered.contains("second-root-1.0 1 shared"),
    "second occurrence should collapse into a parent summary: {rendered}"
  );
  assert!(
    !rendered.contains("⧉"),
    "shared dependency references should not use a marker glyph: {rendered}"
  );
  assert!(
    !rendered.contains("↳"),
    "shared dependency reference should not use a directional arrow: \
     {rendered}"
  );
  assert_eq!(
    rendered.matches("shared-dep-1.0").count(),
    1,
    "shared dependency should not duplicate its active subtree or reference \
     rows: {rendered}"
  );
}

#[test]
fn tui_completed_builds_linger_then_disappear_from_graph() {
  let backend = TestBackend::new(80, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let drv_id = add_derivation(&mut state, "done-1.0");
  let now = current_time();
  state.update_build_status(drv_id, BuildStatus::Built {
    info: BuildInfo {
      start:       now - 3.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    },
    end:  now - 1.0,
  });
  state.forest_roots.push(drv_id);
  let config = tui_config();

  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    rendered.contains("✓"),
    "completed build should show success status: {rendered}"
  );
  assert!(
    rendered.contains("✓ done-1.0"),
    "recent completed build should linger briefly: {rendered}"
  );
  assert!(
    !rendered.contains("~")
      && !rendered.contains("╭")
      && !rendered.contains("╰"),
    "completed activity should not use the old wind/leaf treatment: {rendered}"
  );

  let backend = TestBackend::new(80, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let drv_id = add_derivation(&mut state, "old-done-1.0");
  state.update_build_status(drv_id, BuildStatus::Built {
    info: BuildInfo {
      start:       now - 9.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    },
    end:  now - 6.0,
  });
  state.forest_roots.push(drv_id);

  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let rendered = format!("{}", terminal.backend());
  assert!(
    !rendered.contains("old-done-1.0"),
    "old completed builds should disappear from graph: {rendered}"
  );
}

#[test]
fn tui_bottom_aligns_build_graph() {
  let backend = TestBackend::new(80, 30);
  let mut terminal = Terminal::new(backend).unwrap();
  let state = running_state();
  let logs = Vec::new();
  let config = tui_config();

  terminal
    .draw(|frame| draw(frame, &state, &logs, &config, &TuiView::default()))
    .unwrap();

  let header_row = row_text(&terminal, 0);
  assert!(
    header_row.contains("Builds"),
    "status summary should render in the top header: {header_row:?}"
  );
  assert!(
    header_row.contains("1 running"),
    "header should include running build count: {header_row:?}"
  );
  assert!(
    header_row.contains("elapsed"),
    "header should include elapsed time: {header_row:?}"
  );
  assert!(
    !header_row.contains("∑")
      && !header_row.contains("⏵")
      && !header_row.contains("⏱"),
    "header should not use the old icon summary style: {header_row:?}"
  );

  let top_inner_row = row_text(&terminal, 2);
  assert!(
    !top_inner_row.contains("Dependency Graph"),
    "graph should not start at the top: {top_inner_row:?}"
  );

  let bottom_graph_row = row_text(&terminal, 15);
  assert!(
    bottom_graph_row.contains("hello-1.0"),
    "activity row should sit at the bottom of the graph pane: \
     {bottom_graph_row:?}"
  );
  assert!(
    !bottom_graph_row.contains("Builds")
      && !bottom_graph_row.contains("elapsed"),
    "status summary should not remain in the graph pane: {bottom_graph_row:?}"
  );
}

#[test]
fn tui_color_codes_activity_statuses() {
  let backend = TestBackend::new(80, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let building_id = add_derivation(&mut state, "building-1.0");
  let waiting_id = add_derivation(&mut state, "waiting-1.0");
  let failed_id = add_derivation(&mut state, "failed-1.0");
  let now = current_time();

  state.update_build_status(
    building_id,
    BuildStatus::Building(BuildInfo {
      start:       now,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.update_build_status(waiting_id, BuildStatus::Planned);
  state.update_build_status(failed_id, BuildStatus::Failed {
    info: BuildInfo {
      start:       now - 3.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    },
    fail: rom_core::state::BuildFail {
      at:        now,
      fail_type: FailType::Unknown,
    },
  });
  state
    .forest_roots
    .extend([building_id, waiting_id, failed_id]);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let buffer = terminal.backend().buffer();
  let building_row =
    row_containing(&terminal, "building-1.0").expect("building row");
  let waiting_row =
    row_containing(&terminal, "waiting-1.0").expect("waiting row");
  let failed_row = row_containing(&terminal, "failed-1.0").expect("failed row");

  assert_eq!(buffer[(0, building_row)].fg, MOSS_GREEN);
  assert_eq!(buffer[(0, waiting_row)].fg, MUTED_YELLOW);
  assert_eq!(buffer[(0, failed_row)].fg, MUTED_RED);
}

#[test]
fn tui_uses_muted_graph_connector_lines() {
  let backend = TestBackend::new(80, 24);
  let mut terminal = Terminal::new(backend).unwrap();
  let mut state = State::new();
  let root_id = add_derivation(&mut state, "root-1.0");
  let child_id = add_derivation(&mut state, "child-1.0");

  state
    .get_derivation_info_mut(root_id)
    .unwrap()
    .input_derivations
    .push(InputDerivation {
      derivation: child_id,
      outputs:    HashSet::new(),
    });
  state
    .get_derivation_info_mut(child_id)
    .unwrap()
    .derivation_parents
    .insert(root_id);
  state.update_build_status(root_id, BuildStatus::Planned);
  state.update_build_status(
    child_id,
    BuildStatus::Building(BuildInfo {
      start:       current_time(),
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.forest_roots.push(root_id);

  let config = tui_config();
  terminal
    .draw(|frame| draw(frame, &state, &[], &config, &TuiView::default()))
    .unwrap();

  let buffer = terminal.backend().buffer();
  let child_row =
    row_containing(&terminal, "child-1.0").expect("child row should render");
  assert_eq!(buffer[(0, child_row)].fg, GRAPH_LINE_COLOR);
  assert!(!buffer[(0, child_row)].modifier.contains(Modifier::DIM));
}

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
    .draw(|frame| draw(frame, &state, &logs, &config, &view))
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
    .draw(|frame| draw(frame, &state, &logs, &config, &view))
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
    .draw(|frame| draw(frame, &state, &logs, &config, &view))
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
    .draw(|frame| draw(frame, &state, &logs, &config, &view))
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
    .draw(|frame| draw(frame, &state, &logs, &config, &TuiView::default()))
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
    .draw(|frame| draw(frame, &state, &logs, &config, &TuiView::default()))
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
    .draw(|frame| draw(frame, &state, &logs, &config, &TuiView::default()))
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
