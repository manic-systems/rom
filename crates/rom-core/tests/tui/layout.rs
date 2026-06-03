use super::support::*;

#[test]
fn tui_draws_graph_and_logs_without_header() {
  let backend = TestBackend::new(80, 20);
  let mut terminal = Terminal::new(backend).unwrap();
  let state = running_state();
  let logs = vec!["builder log line".to_string()];
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
    .draw(|frame| {
      draw(
        frame,
        &state.render_snapshot(),
        &[],
        &config,
        &TuiView::default(),
      )
    })
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
    .draw(|frame| {
      draw(
        frame,
        &state.render_snapshot(),
        &[],
        &config,
        &TuiView::default(),
      )
    })
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
    .draw(|frame| {
      draw(
        frame,
        &state.render_snapshot(),
        &[],
        &config,
        &TuiView::default(),
      )
    })
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
    .draw(|frame| {
      draw(
        frame,
        &state.render_snapshot(),
        &[],
        &config,
        &TuiView::default(),
      )
    })
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
    .draw(|frame| {
      draw(
        frame,
        &state.render_snapshot(),
        &[],
        &config,
        &TuiView::default(),
      )
    })
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
    .draw(|frame| {
      draw(
        frame,
        &state.render_snapshot(),
        &[],
        &config,
        &TuiView::default(),
      )
    })
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
    .draw(|frame| {
      draw(
        frame,
        &state.render_snapshot(),
        &[],
        &config,
        &TuiView::default(),
      )
    })
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
    .draw(|frame| {
      draw(
        frame,
        &state.render_snapshot(),
        &[],
        &config,
        &TuiView::default(),
      )
    })
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
