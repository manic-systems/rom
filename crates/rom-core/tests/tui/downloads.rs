use super::support::*;

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
