use std::collections::HashSet;

use cognos::{Actions, Activities, Verbosity};
use rom_core::{
  state::{BuildInfo, BuildStatus, Derivation, InputDerivation, State},
  update::{action_may_update_state, process_message},
};

#[test]
fn internal_json_plan_line_marks_derivation_planned() {
  let mut state = State::new();

  let changed = process_message(&mut state, Actions::Message {
    level:   Verbosity::Info,
    msg:     "  /nix/store/abc123-nixos-system-fool.drv".to_string(),
    raw_msg: None,
    file:    None,
    line:    None,
    column:  None,
  });

  assert!(changed);
  let (drv_id, info) = state.derivation_infos.iter().next().unwrap();
  assert_eq!(info.name.name, "nixos-system-fool");
  assert!(matches!(info.build_status, BuildStatus::Planned));
  assert!(state.full_summary.planned_builds.contains(drv_id));
  assert!(state.forest_roots.contains(drv_id));
}

#[test]
fn build_start_does_not_promote_known_dependency_to_root() {
  let mut state = State::new();
  let root_id = add_drv(&mut state, "/nix/store/aaaaaaaa-root.drv");
  let child_path = "/nix/store/bbbbbbbb-child.drv";
  let child_id = add_drv(&mut state, child_path);

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
  state.forest_roots.push(root_id);

  let changed = process_message(&mut state, Actions::Start {
    id:       1,
    level:    Verbosity::Info,
    parent:   0,
    text:     format!("building '{child_path}'"),
    activity: Activities::Build,
    fields:   vec![serde_json::json!(child_path), serde_json::json!("")],
  });

  assert!(changed);
  assert!(matches!(
    state.get_derivation_info(child_id).unwrap().build_status,
    BuildStatus::Building(BuildInfo { .. })
  ));
  assert!(state.forest_roots.contains(&root_id));
  assert!(!state.forest_roots.contains(&child_id));
}

#[test]
fn ordinary_messages_are_log_only() {
  let action = Actions::Message {
    level:   Verbosity::Info,
    msg:     "kio-extras> -- Found samba: /nix/store/example".to_string(),
    raw_msg: None,
    file:    None,
    line:    None,
    column:  None,
  };

  assert!(!action_may_update_state(&action));

  let mut state = State::new();
  assert!(!process_message(&mut state, action));
  assert_eq!(state.evaluation_state.count, 0);
  assert!(state.nix_errors.is_empty());
  assert!(state.derivation_infos.is_empty());
}

#[test]
fn warning_messages_are_log_only_unless_structural() {
  let action = Actions::Message {
    level:   Verbosity::Warning,
    msg:     "warning: noisy configure output".to_string(),
    raw_msg: None,
    file:    None,
    line:    None,
    column:  None,
  };

  assert!(!action_may_update_state(&action));

  let mut state = State::new();
  assert!(!process_message(&mut state, action));
}

#[test]
fn evaluation_and_error_messages_still_update_state() {
  let eval = Actions::Message {
    level:   Verbosity::Info,
    msg:     "evaluating file '/nix/store/source/default.nix'".to_string(),
    raw_msg: None,
    file:    None,
    line:    None,
    column:  None,
  };
  let error = Actions::Message {
    level:   Verbosity::Error,
    msg:     "error: builder failed".to_string(),
    raw_msg: None,
    file:    None,
    line:    None,
    column:  None,
  };

  assert!(action_may_update_state(&eval));
  assert!(action_may_update_state(&error));

  let mut state = State::new();
  assert!(process_message(&mut state, eval));
  assert_eq!(state.evaluation_state.count, 1);
  assert!(process_message(&mut state, error));
  assert_eq!(state.nix_errors, vec!["error: builder failed"]);
}

fn add_drv(state: &mut State, path: &str) -> usize {
  state.get_or_create_derivation_id(Derivation::parse(path).unwrap())
}
