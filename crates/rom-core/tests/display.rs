use std::collections::{HashMap, HashSet};

use rom_core::{
  display::{Display, DisplayConfig},
  icons,
  state::{
    BuildInfo,
    BuildStatus,
    CompletedBuildInfo,
    DerivationId,
    FailType,
    FailedBuildInfo,
    State,
    current_time,
  },
  types::{DisplayFormat, LegendStyle, SummaryStyle},
};

fn make_drv_info(
  name: &str,
  status: BuildStatus,
) -> rom_core::state::DerivationInfo {
  use std::path::PathBuf;

  use rom_core::state::{DependencySummary, Derivation, DerivationInfo};
  DerivationInfo {
    name:                   Derivation {
      path: PathBuf::from(format!(
        "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-{name}.drv"
      )),
      name: name.to_string(),
    },
    outputs:                HashMap::new(),
    input_derivations:      Vec::new(),
    input_sources:          HashSet::new(),
    build_status:           status,
    dependency_summary:     DependencySummary::default(),
    dependencies_populated: true,
    cached:                 false,
    derivation_parents:     HashSet::new(),
    pname:                  None,
    platform:               None,
  }
}

fn render_tree(state: &State) -> String {
  render_to_string(
    DisplayFormat::Tree,
    false,
    LegendStyle::Table,
    SummaryStyle::Concise,
    state,
    false,
  )
}

fn render_tree_colored(state: &State) -> String {
  render_to_string(
    DisplayFormat::Tree,
    true,
    LegendStyle::Table,
    SummaryStyle::Concise,
    state,
    false,
  )
}

fn render_tree_timed(state: &State) -> String {
  render_to_string_with_timers(
    DisplayFormat::Tree,
    false,
    LegendStyle::Table,
    SummaryStyle::Concise,
    state,
    false,
    true,
  )
}

fn render_to_string(
  format: DisplayFormat,
  use_color: bool,
  legend: LegendStyle,
  summary: SummaryStyle,
  state: &State,
  final_render: bool,
) -> String {
  render_to_string_with_timers(
    format,
    use_color,
    legend,
    summary,
    state,
    final_render,
    false,
  )
}

fn render_to_string_with_timers(
  format: DisplayFormat,
  use_color: bool,
  legend: LegendStyle,
  summary: SummaryStyle,
  state: &State,
  final_render: bool,
  show_timers: bool,
) -> String {
  let mut buf = Vec::new();
  {
    let mut d = Display::new(&mut buf, DisplayConfig {
      show_timers,
      max_tree_depth: 10,
      max_visible_lines: 100,
      use_color,
      format,
      legend_style: legend,
      summary_style: summary,
      icons: &icons::UNICODE,
    })
    .unwrap();
    if final_render {
      d.render_final(state).unwrap();
    } else {
      d.render(state, &[]).unwrap();
    }
  }
  String::from_utf8_lossy(&buf).into_owned()
}

fn state_running() -> State {
  let mut s = State::new();
  s.full_summary.running_builds.insert(0, BuildInfo {
    start:       0.0,
    host:        cognos::Host::Localhost,
    estimate:    None,
    activity_id: None,
  });
  s
}

fn state_completed() -> State {
  let mut s = State::new();
  s.full_summary
    .completed_builds
    .insert(0, CompletedBuildInfo {
      start: 0.0,
      end:   1.0,
      host:  cognos::Host::Localhost,
    });
  s
}

fn state_failed() -> State {
  let mut s = State::new();
  s.full_summary.failed_builds.insert(0, FailedBuildInfo {
    start:     0.0,
    end:       1.0,
    host:      cognos::Host::Localhost,
    fail_type: FailType::BuildFailed(-1),
  });
  s
}

#[test]
fn dashboard_color_on_emits_ansi() {
  let out = render_to_string(
    DisplayFormat::Dashboard,
    true,
    LegendStyle::Table,
    SummaryStyle::Concise,
    &state_running(),
    false,
  );
  assert!(
    out.contains('\x1b'),
    "expected ANSI escapes in colored dashboard output"
  );
}

#[test]
fn dashboard_color_off_no_ansi() {
  let out = render_to_string(
    DisplayFormat::Dashboard,
    false,
    LegendStyle::Table,
    SummaryStyle::Concise,
    &state_running(),
    false,
  );
  assert!(
    !out.contains('\x1b'),
    "expected no ANSI escapes in plain dashboard output"
  );
}

#[test]
fn dashboard_running_shows_building() {
  let out = render_to_string(
    DisplayFormat::Dashboard,
    false,
    LegendStyle::Table,
    SummaryStyle::Concise,
    &state_running(),
    false,
  );
  assert!(
    out.contains("building"),
    "expected 'building' label for running state"
  );
}

#[test]
fn dashboard_completed_shows_done() {
  let out = render_to_string(
    DisplayFormat::Dashboard,
    false,
    LegendStyle::Table,
    SummaryStyle::Concise,
    &state_completed(),
    false,
  );
  assert!(
    out.contains("done"),
    "expected 'done' label when all builds completed"
  );
}

#[test]
fn dashboard_failed_final_shows_failed() {
  let out = render_to_string(
    DisplayFormat::Dashboard,
    false,
    LegendStyle::Table,
    SummaryStyle::Concise,
    &state_failed(),
    true,
  );
  assert!(
    out.contains("failed"),
    "expected 'failed' label in final dashboard with failures"
  );
}

#[test]
fn dashboard_empty_state_no_graph_header() {
  let out = render_to_string(
    DisplayFormat::Dashboard,
    true,
    LegendStyle::Table,
    SummaryStyle::Concise,
    &State::new(),
    false,
  );
  assert!(
    !out.contains("BUILD GRAPH"),
    "expected no BUILD GRAPH header for empty state"
  );
}

#[test]
fn dashboard_nonempty_state_has_graph_header() {
  let out = render_to_string(
    DisplayFormat::Dashboard,
    false,
    LegendStyle::Table,
    SummaryStyle::Concise,
    &state_running(),
    false,
  );
  assert!(
    out.contains("BUILD GRAPH"),
    "expected BUILD GRAPH header in dashboard output"
  );
}

#[test]
fn all_formats_color_on_render_and_final_without_panic() {
  let state = state_running();
  for format in [
    DisplayFormat::Tree,
    DisplayFormat::Plain,
    DisplayFormat::Dashboard,
  ] {
    render_to_string(
      format,
      true,
      LegendStyle::Table,
      SummaryStyle::Concise,
      &state,
      false,
    );
    render_to_string(
      format,
      true,
      LegendStyle::Table,
      SummaryStyle::Concise,
      &state,
      true,
    );
  }
}

#[test]
fn all_formats_color_off_render_and_final_without_panic() {
  let state = state_running();
  for format in [
    DisplayFormat::Tree,
    DisplayFormat::Plain,
    DisplayFormat::Dashboard,
  ] {
    render_to_string(
      format,
      false,
      LegendStyle::Table,
      SummaryStyle::Concise,
      &state,
      false,
    );
    render_to_string(
      format,
      false,
      LegendStyle::Table,
      SummaryStyle::Concise,
      &state,
      true,
    );
  }
}

#[test]
fn legend_compact_color_permutations() {
  let state = state_completed();
  for use_color in [true, false] {
    render_to_string(
      DisplayFormat::Tree,
      use_color,
      LegendStyle::Compact,
      SummaryStyle::Concise,
      &state,
      true,
    );
  }
}

#[test]
fn legend_table_color_permutations() {
  let state = state_completed();
  for use_color in [true, false] {
    render_to_string(
      DisplayFormat::Tree,
      use_color,
      LegendStyle::Table,
      SummaryStyle::Concise,
      &state,
      true,
    );
  }
}

#[test]
fn legend_verbose_color_permutations() {
  let state = state_completed();
  for use_color in [true, false] {
    render_to_string(
      DisplayFormat::Tree,
      use_color,
      LegendStyle::Verbose,
      SummaryStyle::Concise,
      &state,
      true,
    );
  }
}

#[test]
fn summary_concise_all_formats() {
  let state = state_completed();
  for format in [
    DisplayFormat::Tree,
    DisplayFormat::Plain,
    DisplayFormat::Dashboard,
  ] {
    render_to_string(
      format,
      true,
      LegendStyle::Table,
      SummaryStyle::Concise,
      &state,
      true,
    );
  }
}

#[test]
fn summary_table_all_formats() {
  let state = state_completed();
  for format in [
    DisplayFormat::Tree,
    DisplayFormat::Plain,
    DisplayFormat::Dashboard,
  ] {
    render_to_string(
      format,
      true,
      LegendStyle::Table,
      SummaryStyle::Table,
      &state,
      true,
    );
  }
}

#[test]
fn summary_full_all_formats() {
  let state = state_completed();
  for format in [
    DisplayFormat::Tree,
    DisplayFormat::Plain,
    DisplayFormat::Dashboard,
  ] {
    render_to_string(
      format,
      true,
      LegendStyle::Table,
      SummaryStyle::Full,
      &state,
      true,
    );
  }
}

#[test]
fn dashboard_final_build_state_color_permutations() {
  for use_color in [true, false] {
    for state in [state_running(), state_completed(), state_failed()] {
      render_to_string(
        DisplayFormat::Dashboard,
        use_color,
        LegendStyle::Table,
        SummaryStyle::Concise,
        &state,
        true,
      );
    }
  }
}

#[test]
fn tree_empty_state_no_header() {
  let state = State::new();
  let out = render_tree(&state);
  assert!(
    !out.contains("Dependency Graph"),
    "expected no tree header for empty state, got: {out:?}"
  );
}

#[test]
fn tree_single_building_root_shows_header_and_name() {
  let mut state = State::new();
  let drv_id: DerivationId = 0;
  let info = make_drv_info(
    "my-package-1.0",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree(&state);
  assert!(
    out.contains("Dependency Graph"),
    "expected tree header for building root, got: {out:?}"
  );
  assert!(
    out.contains("my-package-1.0"),
    "expected package name in tree output, got: {out:?}"
  );
}

#[test]
fn tree_planned_root_is_visible() {
  let mut state = State::new();
  let drv_id: DerivationId = 1;
  let info = make_drv_info("planned-pkg", BuildStatus::Planned);
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree(&state);
  assert!(
    out.contains("planned-pkg"),
    "expected planned root to be visible, got: {out:?}"
  );
}

#[test]
fn tree_failed_root_is_visible() {
  use rom_core::state::{BuildFail, FailType};
  let mut state = State::new();
  let drv_id: DerivationId = 2;
  let info = make_drv_info("broken-pkg", BuildStatus::Failed {
    info: BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    },
    fail: BuildFail {
      at:        1.0,
      fail_type: FailType::BuildFailed(1),
    },
  });
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree(&state);
  assert!(
    out.contains("broken-pkg"),
    "expected failed root to be visible, got: {out:?}"
  );
  // Failed node should show exit code
  assert!(
    out.contains("exit 1") || out.contains("failed"),
    "expected failure annotation, got: {out:?}"
  );
}

#[test]
fn tree_built_root_is_visible() {
  let mut state = State::new();
  let drv_id: DerivationId = 3;
  let info = make_drv_info("done-pkg", BuildStatus::Built {
    info: BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    },
    end:  1.0,
  });
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree(&state);
  assert!(
    out.contains("done-pkg"),
    "expected built root to appear in tree, got: {out:?}"
  );
}

#[test]
fn tree_unknown_root_without_summary_hidden() {
  let mut state = State::new();
  let drv_id: DerivationId = 4;
  let info = make_drv_info("ghost-pkg", BuildStatus::Unknown);
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree(&state);
  assert!(
    !out.contains("ghost-pkg"),
    "expected unknown root with empty summary to be hidden, got: {out:?}"
  );
}

#[test]
fn tree_unknown_root_with_summary_visible() {
  use std::path::PathBuf;

  use rom_core::state::{DependencySummary, Derivation, DerivationInfo};

  let mut state = State::new();
  let parent_id: DerivationId = 5;
  let child_id: DerivationId = 6;

  let mut summary = DependencySummary::default();
  summary.running_builds.insert(child_id, BuildInfo {
    start:       0.0,
    host:        cognos::Host::Localhost,
    estimate:    None,
    activity_id: None,
  });

  let parent_info = DerivationInfo {
    name:                   Derivation {
      path: PathBuf::from(
        "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-meta-pkg.drv",
      ),
      name: "meta-pkg".to_string(),
    },
    outputs:                HashMap::new(),
    input_derivations:      Vec::new(),
    input_sources:          HashSet::new(),
    build_status:           BuildStatus::Unknown,
    dependency_summary:     summary,
    dependencies_populated: true,
    cached:                 false,
    derivation_parents:     HashSet::new(),
    pname:                  None,
    platform:               None,
  };
  state.derivation_infos.insert(parent_id, parent_info);
  state.forest_roots.push(parent_id);

  let out = render_tree(&state);
  assert!(
    out.contains("meta-pkg"),
    "expected unknown root with non-empty summary to appear, got: {out:?}"
  );
}

// Child appears above parent. The root node is at the bottom; its
// dependencies are above it.
#[test]
fn tree_child_above_parent_layout() {
  use rom_core::state::InputDerivation;

  let mut state = State::new();
  let parent_id: DerivationId = 10;
  let child_id: DerivationId = 11;

  // Child: planned
  let child_info = make_drv_info("child-dep", BuildStatus::Planned);
  state.derivation_infos.insert(child_id, child_info);

  // Parent: building, with child as input
  let mut parent_info = make_drv_info(
    "parent-pkg",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  parent_info.input_derivations.push(InputDerivation {
    derivation: child_id,
    outputs:    HashSet::new(),
  });
  state.derivation_infos.insert(parent_id, parent_info);
  state.forest_roots.push(parent_id);

  let out = render_tree(&state);

  // Both names must appear
  assert!(
    out.contains("child-dep"),
    "expected child-dep in tree output, got: {out:?}"
  );
  assert!(
    out.contains("parent-pkg"),
    "expected parent-pkg in tree output, got: {out:?}"
  );

  // child-dep must appear ABOVE (earlier line than) parent-pkg
  let child_pos = out.find("child-dep").expect("child-dep not found");
  let parent_pos = out.find("parent-pkg").expect("parent-pkg not found");
  assert!(
    child_pos < parent_pos,
    "child-dep should appear above parent-pkg (child_pos={child_pos}, \
     parent_pos={parent_pos})"
  );
}

#[test]
fn tree_last_child_uses_top_connector() {
  use rom_core::state::InputDerivation;

  let mut state = State::new();
  let parent_id: DerivationId = 20;
  let only_child_id: DerivationId = 21;

  let child_info = make_drv_info("only-child", BuildStatus::Planned);
  state.derivation_infos.insert(only_child_id, child_info);

  let mut parent_info = make_drv_info(
    "parent-pkg",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  parent_info.input_derivations.push(InputDerivation {
    derivation: only_child_id,
    outputs:    HashSet::new(),
  });
  state.derivation_infos.insert(parent_id, parent_info);
  state.forest_roots.push(parent_id);

  let out = render_tree(&state);
  // With a single child (which is the last in sorted order), it gets ┌─
  // since it is the topmost sibling. The └─ connector is never used.
  assert!(
    out.contains("┌─"),
    "expected ┌─ connector for single/last child, got: {out:?}"
  );
  assert!(
    !out.contains("└─"),
    "tree format never uses └─, got: {out:?}"
  );
}

#[test]
fn tree_multiple_children_have_branch_and_top_connectors() {
  use rom_core::state::InputDerivation;

  let mut state = State::new();
  let parent_id: DerivationId = 30;
  let child_a_id: DerivationId = 31;
  let child_b_id: DerivationId = 32;

  // Both Planned, same sort priority, so order is preserved.
  let child_a = make_drv_info("alpha-dep", BuildStatus::Planned);
  let child_b = make_drv_info("beta-dep", BuildStatus::Planned);
  state.derivation_infos.insert(child_a_id, child_a);
  state.derivation_infos.insert(child_b_id, child_b);

  let mut parent_info = make_drv_info(
    "parent-pkg",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  parent_info.input_derivations.push(InputDerivation {
    derivation: child_a_id,
    outputs:    HashSet::new(),
  });
  parent_info.input_derivations.push(InputDerivation {
    derivation: child_b_id,
    outputs:    HashSet::new(),
  });
  state.derivation_infos.insert(parent_id, parent_info);
  state.forest_roots.push(parent_id);

  let out = render_tree(&state);
  // With two children: topmost (last in sorted order) gets ┌─, others get ├─.
  // └─ is never used.
  assert!(
    out.contains("├─"),
    "expected ├─ branch connector for non-top child, got: {out:?}"
  );
  assert!(
    out.contains("┌─"),
    "expected ┌─ connector for topmost child, got: {out:?}"
  );
  assert!(
    !out.contains("└─"),
    "tree format never uses └─, got: {out:?}"
  );
}

#[test]
fn tree_sort_order_failed_before_building() {
  use rom_core::state::{BuildFail, FailType, InputDerivation};

  let mut state = State::new();
  let parent_id: DerivationId = 40;
  let building_id: DerivationId = 41;
  let failed_id: DerivationId = 42;

  // Insert children: building first in insertion order, failed second
  let building_child = make_drv_info(
    "building-dep",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  let failed_child = make_drv_info("failed-dep", BuildStatus::Failed {
    info: BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    },
    fail: BuildFail {
      at:        1.0,
      fail_type: FailType::BuildFailed(1),
    },
  });
  state.derivation_infos.insert(building_id, building_child);
  state.derivation_infos.insert(failed_id, failed_child);

  let mut parent_info = make_drv_info(
    "parent-pkg",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  // Insert building first, failed second, sort should override this order
  parent_info.input_derivations.push(InputDerivation {
    derivation: building_id,
    outputs:    HashSet::new(),
  });
  parent_info.input_derivations.push(InputDerivation {
    derivation: failed_id,
    outputs:    HashSet::new(),
  });
  state.derivation_infos.insert(parent_id, parent_info);
  state.forest_roots.push(parent_id);

  let out = render_tree(&state);

  // Sort: Failed(0) < Building(1), so sorted order is [failed, building].
  // After reverse rendering: building (last) appears at the top; failed
  // appears just above the root. Most urgent work is closest to the root.
  let building_pos = out.find("building-dep").expect("building-dep not found");
  let failed_pos = out.find("failed-dep").expect("failed-dep not found");
  assert!(
    building_pos < failed_pos,
    "building-dep should appear above failed-dep (building closer to top, \
     failed closer to root at bottom): building={building_pos}, \
     failed={failed_pos}"
  );
}

#[test]
fn tree_planned_leaf_shows_waiting_annotation() {
  use std::path::PathBuf;

  use rom_core::state::{
    DependencySummary,
    Derivation,
    DerivationInfo,
    InputDerivation,
  };

  let mut state = State::new();
  let parent_id: DerivationId = 50;
  let leaf_id: DerivationId = 51;
  let blocked_by_id: DerivationId = 52;

  // blocked_by_id represents something the leaf is waiting on
  let mut leaf_summary = DependencySummary::default();
  leaf_summary
    .running_builds
    .insert(blocked_by_id, BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    });

  // The leaf node: Planned, no children (it IS a leaf in the tree), but
  // has a non-empty dependency_summary
  let leaf_info = DerivationInfo {
    name:                   Derivation {
      path: PathBuf::from(
        "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-leaf-pkg.drv",
      ),
      name: "leaf-pkg".to_string(),
    },
    outputs:                HashMap::new(),
    input_derivations:      Vec::new(), // no children -> leaf
    input_sources:          HashSet::new(),
    build_status:           BuildStatus::Planned,
    dependency_summary:     leaf_summary,
    dependencies_populated: true,
    cached:                 false,
    derivation_parents:     HashSet::new(),
    pname:                  None,
    platform:               None,
  };
  state.derivation_infos.insert(leaf_id, leaf_info);

  // Parent has the leaf as child; parent must also be visible
  let mut parent_summary = DependencySummary::default();
  parent_summary.planned_builds.insert(leaf_id);
  let parent_info = DerivationInfo {
    name:                   Derivation {
      path: PathBuf::from(
        "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-parent-pkg.drv",
      ),
      name: "parent-pkg".to_string(),
    },
    outputs:                HashMap::new(),
    input_derivations:      vec![InputDerivation {
      derivation: leaf_id,
      outputs:    HashSet::new(),
    }],
    input_sources:          HashSet::new(),
    build_status:           BuildStatus::Planned,
    dependency_summary:     parent_summary,
    dependencies_populated: true,
    cached:                 false,
    derivation_parents:     HashSet::new(),
    pname:                  None,
    platform:               None,
  };
  state.derivation_infos.insert(parent_id, parent_info);
  state.forest_roots.push(parent_id);

  let out = render_tree(&state);

  assert!(
    out.contains("leaf-pkg"),
    "expected leaf-pkg in output, got: {out:?}"
  );
  // The waiting annotation should appear because is_leaf=true and the
  // dependency_summary has 1 running build
  assert!(
    out.contains("waiting for"),
    "expected 'waiting for' annotation on planned leaf node, got: {out:?}"
  );
}

#[test]
fn tree_planned_non_leaf_no_waiting_annotation() {
  use rom_core::state::InputDerivation;

  let mut state = State::new();
  let parent_id: DerivationId = 60;
  let middle_id: DerivationId = 61;
  let grandchild_id: DerivationId = 62;

  // grandchild is a leaf
  let grandchild = make_drv_info("grandchild", BuildStatus::Planned);
  state.derivation_infos.insert(grandchild_id, grandchild);

  // middle node: Planned, HAS a child (so NOT a leaf)
  let mut middle = make_drv_info("middle-node", BuildStatus::Planned);
  middle.input_derivations.push(InputDerivation {
    derivation: grandchild_id,
    outputs:    HashSet::new(),
  });
  state.derivation_infos.insert(middle_id, middle);

  // root
  let mut parent = make_drv_info(
    "root-pkg",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  parent.input_derivations.push(InputDerivation {
    derivation: middle_id,
    outputs:    HashSet::new(),
  });
  state.derivation_infos.insert(parent_id, parent);
  state.forest_roots.push(parent_id);

  let out = render_tree(&state);

  // middle-node has children, so is NOT a leaf.  It must NOT show
  // "waiting for" (that annotation is only for leaves).
  let middle_line = out
    .lines()
    .find(|l| l.contains("middle-node"))
    .unwrap_or("");
  assert!(
    !middle_line.contains("waiting for"),
    "non-leaf node should not show 'waiting for' annotation, got line: \
     {middle_line:?}"
  );
}

#[test]
fn tree_multi_root_uses_forest_connectors() {
  let mut state = State::new();
  let root_a_id: DerivationId = 70;
  let root_b_id: DerivationId = 71;

  let info_a = make_drv_info(
    "root-a",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  let info_b = make_drv_info(
    "root-b",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );

  state.derivation_infos.insert(root_a_id, info_a);
  state.derivation_infos.insert(root_b_id, info_b);
  state.forest_roots.push(root_a_id);
  state.forest_roots.push(root_b_id);

  let out = render_tree(&state);

  // Both roots should appear
  assert!(out.contains("root-a"), "expected root-a, got: {out:?}");
  assert!(out.contains("root-b"), "expected root-b, got: {out:?}");

  // With multiple roots, cross-tree connectors should appear
  // ┌─ for the last (bottom-most) root, ├─ for earlier roots
  assert!(
    out.contains("┌─") || out.contains("├─"),
    "expected cross-tree forest connectors (┌─ or ├─), got: {out:?}"
  );
}

#[test]
fn tree_single_root_no_forest_connectors() {
  // A single root with no children produces no ┌─ connectors.
  let mut state = State::new();
  let root_id: DerivationId = 80;
  let info = make_drv_info(
    "sole-root",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.derivation_infos.insert(root_id, info);
  state.forest_roots.push(root_id);

  let out = render_tree(&state);

  assert!(
    out.contains("sole-root"),
    "expected sole-root, got: {out:?}"
  );
  // No children -> no ┌─ connector
  assert!(
    !out.contains("┌─"),
    "root with no children should produce no ┌─ connector, got: {out:?}"
  );
  // No ├─ either since there are no siblings
  assert!(
    !out.contains("├─"),
    "root with no children should produce no ├─ connector, got: {out:?}"
  );
}

#[test]
fn tree_color_on_emits_ansi() {
  let mut state = State::new();
  let drv_id: DerivationId = 90;
  let info = make_drv_info(
    "colored-pkg",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree_colored(&state);
  assert!(
    out.contains('\x1b'),
    "expected ANSI escapes in colored tree output, got: {out:?}"
  );
}

#[test]
fn tree_color_off_no_ansi() {
  let mut state = State::new();
  let drv_id: DerivationId = 91;
  let info = make_drv_info(
    "plain-pkg",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree(&state);
  assert!(
    !out.contains('\x1b'),
    "expected no ANSI escapes in plain tree output, got: {out:?}"
  );
}

#[test]
fn tree_failed_node_shows_exit_code() {
  use rom_core::state::{BuildFail, FailType};

  let mut state = State::new();
  let drv_id: DerivationId = 100;
  let info = make_drv_info("failing-pkg", BuildStatus::Failed {
    info: BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    },
    fail: BuildFail {
      at:        2.0,
      fail_type: FailType::BuildFailed(127),
    },
  });
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree(&state);
  assert!(
    out.contains("127"),
    "expected exit code 127 in failed node output, got: {out:?}"
  );
}

#[test]
fn tree_building_on_remote_host_shows_host() {
  let mut state = State::new();
  let drv_id: DerivationId = 110;
  let info = make_drv_info(
    "remote-build",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Remote("builder-01".to_string()),
      estimate:    None,
      activity_id: None,
    }),
  );
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree(&state);
  assert!(
    out.contains("builder-01"),
    "expected remote host name in building node output, got: {out:?}"
  );
}

#[test]
fn tree_deep_nesting_order() {
  use rom_core::state::InputDerivation;

  let mut state = State::new();
  let root_id: DerivationId = 120;
  let child_id: DerivationId = 121;
  let grandchild_id: DerivationId = 122;

  let grandchild = make_drv_info("grandchild-pkg", BuildStatus::Planned);
  state.derivation_infos.insert(grandchild_id, grandchild);

  let mut child = make_drv_info("child-pkg", BuildStatus::Planned);
  child.input_derivations.push(InputDerivation {
    derivation: grandchild_id,
    outputs:    HashSet::new(),
  });
  state.derivation_infos.insert(child_id, child);

  let mut root = make_drv_info(
    "root-pkg",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  root.input_derivations.push(InputDerivation {
    derivation: child_id,
    outputs:    HashSet::new(),
  });
  state.derivation_infos.insert(root_id, root);
  state.forest_roots.push(root_id);

  let out = render_tree(&state);

  let gchild_pos = out
    .find("grandchild-pkg")
    .expect("grandchild-pkg not found");
  let child_pos = out.find("child-pkg").expect("child-pkg not found");
  let root_pos = out.find("root-pkg").expect("root-pkg not found");

  assert!(
    gchild_pos < child_pos,
    "grandchild should appear above child (grandchild={gchild_pos} \
     child={child_pos})"
  );
  assert!(
    child_pos < root_pos,
    "child should appear above root (child={child_pos} root={root_pos})"
  );
}

#[test]
fn tree_cycle_does_not_panic() {
  use rom_core::state::InputDerivation;

  let mut state = State::new();
  let a_id: DerivationId = 200;
  let b_id: DerivationId = 201;

  // Create a cycle: a -> b -> a
  let mut a = make_drv_info(
    "cyclic-a",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  a.input_derivations.push(InputDerivation {
    derivation: b_id,
    outputs:    HashSet::new(),
  });

  let mut b = make_drv_info(
    "cyclic-b",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  b.input_derivations.push(InputDerivation {
    derivation: a_id,
    outputs:    HashSet::new(),
  });

  state.derivation_infos.insert(a_id, a);
  state.derivation_infos.insert(b_id, b);
  state.forest_roots.push(a_id);

  // Must not panic or loop forever
  let out = render_tree(&state);
  assert!(
    out.contains("cyclic-a"),
    "expected cyclic-a in output, got: {out:?}"
  );
}

#[test]
fn tree_unknown_node_shows_no_icon() {
  use rom_core::state::DependencySummary;
  let mut state = State::new();
  let drv_id: DerivationId = 300;
  // Populate dependency_summary so node_is_visible returns true for Unknown
  let mut dep_summary = DependencySummary::default();
  dep_summary.planned_builds.insert(999);
  let mut info = make_drv_info("unknown-pkg", BuildStatus::Unknown);
  info.dependency_summary = dep_summary;
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree(&state);
  assert!(
    out.contains("unknown-pkg"),
    "expected unknown-pkg in output, got: {out:?}"
  );
  // Must not contain '?' as an icon before the name
  assert!(
    !out.contains("? unknown-pkg"),
    "expected no '?' icon before unknown node name, got: {out:?}"
  );
}

#[test]
fn tree_failed_node_shows_full_exit_code_text() {
  use rom_core::state::{BuildFail, FailType};

  let mut state = State::new();
  let drv_id: DerivationId = 310;
  let info = make_drv_info("exit-pkg", BuildStatus::Failed {
    info: BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    },
    fail: BuildFail {
      at:        2.0,
      fail_type: FailType::BuildFailed(42),
    },
  });
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree(&state);
  assert!(
    out.contains("failed with exit code 42"),
    "expected 'failed with exit code 42', got: {out:?}"
  );
  // Old format must not be present
  assert!(
    !out.contains("failed (exit 42)"),
    "old 'failed (exit N)' format should not be present, got: {out:?}"
  );
}

#[test]
fn tree_failed_remote_host_uncolored() {
  use rom_core::state::{BuildFail, FailType};

  let mut state = State::new();
  let drv_id: DerivationId = 320;
  let info = make_drv_info("remote-fail-pkg", BuildStatus::Failed {
    info: BuildInfo {
      start:       0.0,
      host:        cognos::Host::Remote("fail-builder".to_string()),
      estimate:    None,
      activity_id: None,
    },
    fail: BuildFail {
      at:        1.5,
      fail_type: FailType::BuildFailed(1),
    },
  });
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  // In color mode, the host should appear WITHOUT an ANSI escape immediately
  // before it (i.e., plain text " on fail-builder", not colored)
  let out = render_tree_colored(&state);
  // The host text must be present
  assert!(
    out.contains("fail-builder"),
    "expected remote host in failed node, got: {out:?}"
  );
  // The " on fail-builder" should NOT be immediately preceded by a color code
  // We verify by checking the plain output has it too
  let plain_out = render_tree(&state);
  assert!(
    plain_out.contains("on fail-builder"),
    "expected 'on fail-builder' in plain output, got: {plain_out:?}"
  );
}

#[test]
fn tree_building_elapsed_hidden_when_under_one_second() {
  let mut state = State::new();
  let drv_id: DerivationId = 330;
  // start = current_time() so elapsed ≈ 0s (well under 1s)
  let info = make_drv_info(
    "fast-build",
    BuildStatus::Building(BuildInfo {
      start:       current_time(), // elapsed ≈ 0
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree_timed(&state); // show_timers = true
  // Elapsed is ~0s so the clock icon should not appear.
  assert!(
    !out.contains("⏱"),
    "elapsed should be hidden when under 1s, got: {out:?}"
  );
}

#[test]
fn tree_building_estimate_shown_after_elapsed() {
  let mut state = State::new();
  let drv_id: DerivationId = 340;
  // Use start = 0.0 so elapsed > 1s (current_time() is well above 0)
  let info = make_drv_info(
    "estimated-build",
    BuildStatus::Building(BuildInfo {
      start:       0.0,
      host:        cognos::Host::Localhost,
      estimate:    Some(120), // 2 minute estimate
      activity_id: None,
    }),
  );
  state.derivation_infos.insert(drv_id, info);
  state.forest_roots.push(drv_id);

  let out = render_tree_timed(&state);
  // The estimate icon (∅) should appear
  assert!(
    out.contains("∅"),
    "expected estimate icon '∅' in building node with estimate, got: {out:?}"
  );
  // Clock icon should also appear since elapsed > 1s
  assert!(
    out.contains("⏱"),
    "expected clock icon in building node with elapsed > 1s, got: {out:?}"
  );
  // Clock must appear before estimate in output (elapsed then estimate)
  let clock_pos = out.find("⏱").unwrap();
  let est_pos = out.find("∅").unwrap();
  assert!(
    clock_pos < est_pos,
    "elapsed (⏱) should appear before estimate (∅), clock={clock_pos} \
     est={est_pos}"
  );
}
