//! State update logic for processing nix messages
mod maintenance;

use cognos::{
  Actions,
  Activities,
  Host,
  Id,
  ProgressState,
  ResultType,
  Verbosity,
};
pub use maintenance::{
  detect_local_completed_builds,
  finish_state,
  maintain_state,
};
use tracing::{debug, trace};

use crate::{
  cache::BuildReportCache,
  state::{
    ActivityProgress,
    ActivityStatus,
    BuildFail,
    BuildInfo,
    BuildReport,
    BuildStatus,
    CompletedTransferInfo,
    Derivation,
    DerivationId,
    FailType,
    InputDerivation,
    State,
    StorePath,
    TransferInfo,
    current_time,
  },
};

/// Process a nix JSON message and update state
pub fn process_message(state: &mut State, action: Actions) -> bool {
  let now = current_time();
  let mut changed = false;

  trace!("Processing action: {:?}", action);

  if !action_may_update_state(&action) {
    return false;
  }

  // Mark that we've received input
  if state.progress_state == ProgressState::JustStarted {
    state.progress_state = ProgressState::InputReceived;
    changed = true;
  }

  match action {
    Actions::Start {
      id,
      parent,
      text,
      activity,
      fields,
      ..
    } => {
      changed |= handle_start(state, StartAction {
        id,
        parent_id: if parent == 0 { None } else { Some(parent) },
        text,
        activity,
        fields,
        now,
      });
    },
    Actions::Stop { id } => {
      changed |= handle_stop(state, id, now);
    },
    Actions::Message {
      level,
      msg,
      raw_msg,
      ..
    } => {
      // Prefer raw_msg (Lix). It's the message without ANSI escape codes.
      // Fall back to msg for Nix, which doesn't provide raw_msg.
      let clean = raw_msg.unwrap_or(msg);
      changed |= handle_message(state, level, clean);
    },
    Actions::Result {
      id,
      result_type,
      fields,
    } => {
      changed |= handle_result(state, id, result_type, fields, now);
    },
  }

  changed
}

#[must_use]
pub fn action_may_update_state(action: &Actions) -> bool {
  match action {
    Actions::Start { .. } | Actions::Stop { .. } => true,
    Actions::Message {
      level,
      msg,
      raw_msg,
      ..
    } => {
      let clean = raw_msg.as_deref().unwrap_or(msg.as_str());
      message_may_update_state(*level, clean)
    },
    Actions::Result { result_type, .. } => {
      matches!(
        result_type,
        ResultType::UntrustedPath
          | ResultType::CorruptedPath
          | ResultType::SetPhase
          | ResultType::Progress
      )
    },
  }
}

fn message_may_update_state(level: Verbosity, msg: &str) -> bool {
  if message_is_indented_plan_line(msg) || msg.contains("Running phase: ") {
    return true;
  }

  match level {
    Verbosity::Error => msg.contains("error:") || msg.contains("failed"),
    Verbosity::Info | Verbosity::Notice => {
      (msg.contains("evaluating") || msg.contains("copying"))
        && extract_file_name(msg).is_some()
    },
    Verbosity::Talkative
    | Verbosity::Chatty
    | Verbosity::Debug
    | Verbosity::Vomit => true,
    Verbosity::Warning => false,
  }
}

struct StartAction {
  id:        Id,
  parent_id: Option<Id>,
  text:      String,
  activity:  Activities,
  fields:    Vec<serde_json::Value>,
  now:       f64,
}

fn handle_start(state: &mut State, start: StartAction) -> bool {
  let StartAction {
    id,
    parent_id,
    text,
    activity,
    fields,
    now,
  } = start;

  let activity_u8 = activity as u8;

  state.activities.insert(id, ActivityStatus {
    activity: activity_u8,
    text:     text.clone(),
    parent:   parent_id,
    phase:    None,
    progress: None,
  });

  let changed = match activity_u8 {
    105 => handle_build_start(state, id, parent_id, &text, &fields, now), /* Build */
    108 => handle_substitute_start(state, id, &text, &fields, now), /* Substitute */
    109 => handle_query_path_info_start(state, id, &text, &fields, now), /* QueryPathInfo */
    110 => handle_post_build_hook_start(state, id, &text, &fields, now), /* PostBuildHook */
    101 => handle_file_transfer_start(state, id, &text, &fields, now), /* FileTransfer */
    100 => handle_copy_path_start(state, id, &text, &fields, now), /* CopyPath */
    104 => {
      // Builds activity - track this as the top-level builds activity
      if state.builds_activity.is_none() {
        state.builds_activity = Some(id);
        true
      } else {
        false
      }
    },
    102 | 103 | 106 | 107 | 111 | 112 => {
      // Realise, CopyPaths, OptimiseStore, VerifyPaths, BuildWaiting, FetchTree
      // These activities have no fields and are just tracked
      true
    },
    _ => {
      debug!("Unknown activity type: {}", activity_u8);
      false
    },
  };

  // Track parent-child relationships for dependency tree
  if changed
    && activity_u8 == 105
    && let Some(parent_act_id) = parent_id
  {
    // Find parent and child derivation IDs
    let parent_drv_id = find_derivation_by_activity(state, parent_act_id);
    let child_drv_id = find_derivation_by_activity(state, id);

    if let Some(parent_drv_id) = parent_drv_id
      && let Some(child_drv_id) = child_drv_id
    {
      debug!(
        "Establishing parent-child relationship: parent={parent_drv_id}, \
         child={child_drv_id}"
      );

      // Add child as a dependency of parent
      if let Some(parent_info) = state.get_derivation_info_mut(parent_drv_id) {
        let input = InputDerivation {
          derivation: child_drv_id,
          outputs:    std::collections::HashSet::new(),
        };
        if !parent_info
          .input_derivations
          .iter()
          .any(|d| d.derivation == child_drv_id)
        {
          parent_info.input_derivations.push(input);
          debug!("Added child to parent's input_derivations");
        }
      }
      // Mark child as having a parent
      if let Some(child_info) = state.get_derivation_info_mut(child_drv_id) {
        child_info.derivation_parents.insert(parent_drv_id);
      }
      // Remove child from forest roots since it has a parent
      state.forest_roots.retain(|&id| id != child_drv_id);
    }
  }

  changed
}

fn handle_stop(state: &mut State, id: Id, now: f64) -> bool {
  let activity = state.activities.get(&id).cloned();

  if let Some(activity_status) = activity {
    state.activities.remove(&id);

    match activity_status.activity {
      105 => handle_build_stop(state, id, now), // Build
      108 => handle_substitute_stop(state, id, now), // Substitute
      101 | 100 => handle_transfer_stop(state, id, now), // FileTransfer,
      // CopyPath
      109 | 110 => {
        // QueryPathInfo, PostBuildHook - just acknowledge stop
        false
      },
      102 | 103 | 104 | 106 | 107 | 111 | 112 => {
        // Realise, CopyPaths, Builds, OptimiseStore, VerifyPaths, BuildWaiting,
        // FetchTree
        false
      },
      _ => false,
    }
  } else {
    false
  }
}

fn handle_message(state: &mut State, level: Verbosity, msg: String) -> bool {
  let mut changed = handle_indented_plan_line(state, &msg);

  // Extract phase from log messages like "Running phase: configurePhase"
  if let Some(phase_start) = msg.find("Running phase: ") {
    let phase_name = &msg[phase_start + 15..]; // skip "Running phase: "
    let phase = phase_name.trim().to_string();

    // Find the active build and update its phase
    for activity in state.activities.values_mut() {
      if activity.activity == 105 {
        // Build activity
        activity.phase = Some(phase.clone());
        changed = true;
      }
    }
  }

  match level {
    Verbosity::Error => {
      // Track errors
      if msg.contains("error:") || msg.contains("failed") {
        state.nix_errors.push(msg.clone());

        // Try to extract which build failed
        if let Some(drv_path) = extract_derivation_from_error(&msg)
          && let Some(drv) = Derivation::parse(&drv_path)
        {
          let drv_id = state.get_or_create_derivation_id(drv);

          // Get build info first
          let build_info_opt =
            state.get_derivation_info(drv_id).and_then(|info| {
              if let BuildStatus::Building(build_info) = &info.build_status {
                Some(build_info.clone())
              } else {
                None
              }
            });

          if let Some(build_info) = build_info_opt {
            let fail = BuildFail {
              at:        current_time(),
              fail_type: parse_fail_type(&msg),
            };

            state.update_build_status(drv_id, BuildStatus::Failed {
              info: build_info,
              fail,
            });
          }
        }
        return true;
      }
      changed
    },
    Verbosity::Info | Verbosity::Notice => {
      // Track info messages for evaluation progress
      if msg.contains("evaluating") || msg.contains("copying") {
        // Update evaluation state
        if let Some(file_name) = extract_file_name(&msg) {
          state.evaluation_state.last_file_name = Some(file_name);
          state.evaluation_state.count += 1;
          state.evaluation_state.at = current_time();
          changed = true;
        }
      }
      changed
    },
    Verbosity::Talkative
    | Verbosity::Chatty
    | Verbosity::Debug
    | Verbosity::Vomit => {
      // These are trace-level messages, store separately
      state.push_trace(msg);
      true
    },
    _ => changed,
  }
}

fn handle_indented_plan_line(state: &mut State, msg: &str) -> bool {
  if !message_is_indented_plan_line(msg) {
    return false;
  }

  let path = msg.trim();
  if let Some(drv) = Derivation::parse(path) {
    state.plan_derivation(drv);
    return true;
  }

  if let Some(store_path) = StorePath::parse(path) {
    let store_path_id = state.get_or_create_store_path_id(store_path);
    return state.full_summary.planned_downloads.insert(store_path_id);
  }

  false
}

fn message_is_indented_plan_line(msg: &str) -> bool {
  msg.starts_with("  /nix/store/") || msg.starts_with("\t/nix/store/")
}

fn handle_result(
  state: &mut State,
  id: Id,
  result_type: ResultType,
  fields: Vec<serde_json::Value>,
  _now: f64,
) -> bool {
  match result_type {
    ResultType::FileLinked => {
      if fields.len() >= 2 {
        debug!(
          "FileLinked: {}/{}",
          fields[0].as_u64().unwrap_or(0),
          fields[1].as_u64().unwrap_or(0)
        );
      }
      false
    },
    ResultType::BuildLogLine => false,
    ResultType::UntrustedPath => {
      if let Some(path) = fields.first().and_then(|f| f.as_str()) {
        debug!("Untrusted path: {}", path);
        state.nix_errors.push(format!("Untrusted path: {path}"));
        return true;
      }
      false
    },
    ResultType::CorruptedPath => {
      if let Some(path) = fields.first().and_then(|f| f.as_str()) {
        state.nix_errors.push(format!("Corrupted path: {path}"));
        return true;
      }
      false
    },
    ResultType::SetPhase => {
      if let Some(phase) = fields.first().and_then(|f| f.as_str())
        && let Some(activity) = state.activities.get_mut(&id)
      {
        activity.phase = Some(phase.to_string());
        return true;
      }
      false
    },
    ResultType::Progress => {
      if fields.len() >= 4
        && let (Some(done), Some(expected), Some(running), Some(failed)) = (
          fields[0].as_u64(),
          fields[1].as_u64(),
          fields[2].as_u64(),
          fields[3].as_u64(),
        )
        && let Some(activity) = state.activities.get_mut(&id)
      {
        activity.progress = Some(ActivityProgress {
          done,
          expected,
          running,
          failed,
        });
        return true;
      }
      false
    },
    ResultType::SetExpected => {
      if fields.len() >= 2 {
        debug!(
          "SetExpected: activity_type={}, count={}",
          fields[0].as_u64().unwrap_or(0),
          fields[1].as_u64().unwrap_or(0)
        );
      }
      false
    },
    ResultType::PostBuildLogLine => false,
    ResultType::FetchStatus => {
      if let Some(status) = fields.first().and_then(|f| f.as_str()) {
        debug!("Fetch status: {status}");
      }
      false
    },
  }
}

/// Get build time estimate from cache
fn get_build_estimate(
  state: &State,
  derivation_name: &str,
  host: &Host,
) -> Option<u64> {
  // Use pname if available, otherwise derivation name
  let lookup_name = derivation_name.to_string();
  let host_str = host.name();

  BuildReportCache::calculate_median(
    state
      .build_cache
      .get(&(host_str.to_string(), lookup_name))?
      .as_slice(),
  )
}

/// Record completed build for future predictions
pub(super) fn record_build_completion(
  state: &mut State,
  derivation_name: String,
  platform: Option<String>,
  start: f64,
  end: f64,
  host: &Host,
) {
  let duration_secs = end - start;
  let completed_at = std::time::SystemTime::now();

  let report = BuildReport {
    derivation_name: derivation_name.clone(),
    platform: platform.unwrap_or_default(),
    duration_secs,
    completed_at,
    host: host.name().to_string(),
    success: true,
  };

  // Store in state for later CSV persistence
  let key = (host.name().to_string(), derivation_name);
  state.build_cache.entry(key).or_default().push(report);
}

fn handle_build_start(
  state: &mut State,
  id: Id,
  parent_id: Option<Id>,
  text: &str,
  fields: &[serde_json::Value],
  now: f64,
) -> bool {
  debug!(
    "handle_build_start: id={}, text={}, fields={:?}",
    id, text, fields
  );

  // First try to get derivation path from fields
  let drv_path = if fields.is_empty() {
    extract_derivation_path(text)
  } else {
    fields[0].as_str().map(std::string::ToString::to_string)
  };

  if let Some(drv_path) = drv_path {
    debug!("Extracted derivation path: {}", drv_path);
    if let Some(drv) = Derivation::parse(&drv_path) {
      let drv_id = state.get_or_create_derivation_id(drv.clone());
      let host =
        parse_host(fields.get(1).and_then(|v| v.as_str()).unwrap_or(""));

      // Get build time estimate from cache
      let estimate = get_build_estimate(state, &drv.name, &host);

      let build_info = BuildInfo {
        start: now,
        host,
        estimate,
        activity_id: Some(id),
      };

      debug!("Setting derivation {} to Building status", drv_id);
      state.update_build_status(drv_id, BuildStatus::Building(build_info));
      debug!(
        "After update_build_status, state has {} derivations",
        state.derivation_infos.len()
      );

      // Mark as forest root only if there is no protocol parent and no
      // already-discovered derivation parent.
      let has_derivation_parent = state
        .get_derivation_info(drv_id)
        .is_some_and(|info| !info.derivation_parents.is_empty());
      if parent_id.is_none()
        && !has_derivation_parent
        && !state.forest_roots.contains(&drv_id)
      {
        state.forest_roots.push(drv_id);
      }

      return true;
    }
    debug!("Failed to parse derivation from path: {}", drv_path);
  } else {
    debug!(
      "No derivation path in fields for Build activity {} - this should not \
       happen",
      id
    );
  }
  false
}

fn handle_build_stop(state: &mut State, id: Id, now: f64) -> bool {
  // Find the derivation associated with this Build activity. Per NOM's design,
  // Stop for a Build activity means the build completed.
  let result = state.derivation_infos.iter().find_map(|(drv_id, info)| {
    if let BuildStatus::Building(build_info) = &info.build_status {
      if build_info.activity_id == Some(id) {
        Some((
          *drv_id,
          build_info.clone(),
          info.name.name.clone(),
          info.platform.clone(),
        ))
      } else {
        None
      }
    } else {
      None
    }
  });

  if let Some((drv_id, build_info, name, platform)) = result {
    let start = build_info.start;
    let host = build_info.host.clone();
    state.update_build_status(drv_id, BuildStatus::Built {
      info: build_info,
      end:  now,
    });
    record_build_completion(state, name, platform, start, now, &host);
    debug!("Build completed for derivation {drv_id}");
    return true;
  }

  debug!(
    "Build stopped for activity {id} but no matching building derivation found"
  );
  false
}

fn handle_substitute_start(
  state: &mut State,
  id: Id,
  text: &str,
  fields: &[serde_json::Value],
  now: f64,
) -> bool {
  // Extract store path
  let path_str = if fields.is_empty() {
    extract_store_path(text)
  } else {
    fields[0].as_str().map(std::string::ToString::to_string)
  };

  if let Some(path_str) = path_str
    && let Some(path) = StorePath::parse(&path_str)
  {
    let path_id = state.get_or_create_store_path_id(path);
    let host = parse_host(fields.get(1).and_then(|v| v.as_str()).unwrap_or(""));

    let transfer = TransferInfo {
      start: now,
      host,
      activity_id: id,
      bytes_transferred: 0,
      total_bytes: None,
    };

    state
      .full_summary
      .running_downloads
      .insert(path_id, transfer);
    state.full_summary.planned_downloads.remove(&path_id);

    return true;
  }
  false
}

fn handle_substitute_stop(state: &mut State, id: Id, now: f64) -> bool {
  // Find the store path associated with this activity
  let result = state.full_summary.running_downloads.iter().find_map(
    |(path_id, transfer_info)| {
      if transfer_info.activity_id == id {
        Some((*path_id, transfer_info.clone()))
      } else {
        None
      }
    },
  );

  if let Some((path_id, transfer_info)) = result {
    state.full_summary.running_downloads.remove(&path_id);
    state.full_summary.planned_downloads.remove(&path_id);
    state.full_summary.completed_downloads.insert(
      path_id,
      CompletedTransferInfo {
        start:       transfer_info.start,
        end:         now,
        host:        transfer_info.host,
        total_bytes: transfer_info.bytes_transferred,
      },
    );
    return true;
  }

  false
}

fn handle_file_transfer_start(
  _state: &mut State,
  id: Id,
  _text: &str,
  fields: &[serde_json::Value],
  _now: f64,
) -> bool {
  // FileTransfer expects 1 text field: URL or description
  if fields.is_empty() {
    debug!("FileTransfer activity {} has no fields", id);
    return false;
  }

  // Just track the activity, actual progress comes via Result messages
  true
}

fn handle_copy_path_start(
  state: &mut State,
  id: Id,
  _text: &str,
  fields: &[serde_json::Value],
  now: f64,
) -> bool {
  // CopyPath expects 3 text fields: path, from, to
  if fields.len() < 3 {
    debug!("CopyPath activity {} has insufficient fields", id);
    return false;
  }

  let path_str = fields[0].as_str();
  let _from_host = fields[1].as_str().map(|s| {
    if s.is_empty() || s == "localhost" {
      Host::Localhost
    } else {
      Host::Remote(s.to_string())
    }
  });
  let to_host = fields[2].as_str().map(|s| {
    if s.is_empty() || s == "localhost" {
      Host::Localhost
    } else {
      Host::Remote(s.to_string())
    }
  });

  if let (Some(path_str), Some(to)) = (path_str, to_host)
    && let Some(path) = StorePath::parse(path_str)
  {
    let path_id = state.get_or_create_store_path_id(path);

    let transfer = TransferInfo {
      start:             now,
      host:              to, // destination host
      activity_id:       id,
      bytes_transferred: 0,
      total_bytes:       None,
    };

    // CopyPath is an upload from 'from' to 'to'
    state.full_summary.running_uploads.insert(path_id, transfer);
    return true;
  }

  false
}

fn handle_query_path_info_start(
  _state: &mut State,
  id: Id,
  _text: &str,
  fields: &[serde_json::Value],
  _now: f64,
) -> bool {
  // QueryPathInfo expects 2 text fields: path, host
  if fields.len() < 2 {
    debug!("QueryPathInfo activity {} has insufficient fields", id);
    return false;
  }

  // Just track the activity
  true
}

fn handle_post_build_hook_start(
  _state: &mut State,
  id: Id,
  _text: &str,
  fields: &[serde_json::Value],
  _now: f64,
) -> bool {
  // PostBuildHook expects 1 text field: derivation path
  if fields.is_empty() {
    debug!("PostBuildHook activity {} has no fields", id);
    return false;
  }

  let drv_path = fields[0].as_str();
  if let Some(drv_path) = drv_path
    && let Some(_drv) = Derivation::parse(drv_path)
  {
    // Just track that the hook is running
    return true;
  }

  false
}

fn handle_transfer_stop(state: &mut State, id: Id, now: f64) -> bool {
  // Check downloads; find the matching path_id without cloning the entire map
  if let Some(path_id) = state
    .full_summary
    .running_downloads
    .iter()
    .find(|(_, transfer_info)| transfer_info.activity_id == id)
    .map(|(&id, _)| id)
  {
    let transfer_info = state.full_summary.running_downloads.remove(&path_id);

    if let Some(transfer_info) = transfer_info {
      let completed = CompletedTransferInfo {
        start:       transfer_info.start,
        end:         now,
        host:        transfer_info.host.clone(),
        total_bytes: transfer_info.bytes_transferred,
      };

      state
        .full_summary
        .completed_downloads
        .insert(path_id, completed);
    }
    return true;
  }

  // Check uploads
  if let Some(path_id) = state
    .full_summary
    .running_uploads
    .iter()
    .find(|(_, transfer_info)| transfer_info.activity_id == id)
    .map(|(&id, _)| id)
  {
    let transfer_info = state.full_summary.running_uploads.remove(&path_id);

    if let Some(transfer_info) = transfer_info {
      let completed = CompletedTransferInfo {
        start:       transfer_info.start,
        end:         now,
        host:        transfer_info.host.clone(),
        total_bytes: transfer_info.bytes_transferred,
      };

      state
        .full_summary
        .completed_uploads
        .insert(path_id, completed);
    }
    return true;
  }

  false
}

fn extract_derivation_path(text: &str) -> Option<String> {
  // Look for .drv paths in the text
  if let Some(start) = text.find("/nix/store/")
    && let Some(end) = text[start..].find(".drv")
  {
    return Some(text[start..start + end + 4].to_string());
  }
  None
}

fn extract_store_path(text: &str) -> Option<String> {
  // Look for store paths in the text
  if let Some(start) = text.find("/nix/store/") {
    // Find the end of the path (space or end of string)
    let rest = &text[start..];
    let end = rest
      .find(|c: char| c.is_whitespace() || c == '\'' || c == '"')
      .unwrap_or(rest.len());
    return Some(rest[..end].to_string());
  }
  None
}

/// Parse a host from the fields[1] string of a Build or Substitute activity.
///
/// - Empty string, "local", "local://", "unix", "unix://" -> Localhost
/// - Properly strips proto:// prefix and extracts just the hostname
/// - Strips user@ from user@host format
fn parse_host(s: &str) -> Host {
  let s = s.trim();

  // Handle known localhost aliases
  if s.is_empty()
    || s == "localhost"
    || s == "local"
    || s == "local://"
    || s == "unix"
    || s == "unix://"
  {
    return Host::Localhost;
  }

  // Strip protocol prefix (ssh://, https://, http://, etc.)
  let after_proto = s
    .strip_prefix("ssh://")
    .or_else(|| s.strip_prefix("https://"))
    .or_else(|| s.strip_prefix("http://"))
    .unwrap_or(s)
    .trim_end_matches('/');

  if after_proto.is_empty() || after_proto == "localhost" {
    return Host::Localhost;
  }

  // Strip user@ prefix if present (e.g., "user@hostname" -> "hostname")
  let hostname = after_proto
    .split('@')
    .next_back()
    .unwrap_or(after_proto)
    .trim();

  if hostname.is_empty() || hostname == "localhost" {
    Host::Localhost
  } else {
    Host::Remote(hostname.to_string())
  }
}

fn extract_derivation_from_error(msg: &str) -> Option<String> {
  extract_derivation_path(msg)
}

fn extract_file_name(msg: &str) -> Option<String> {
  // Try to extract file name from evaluation messages
  if let Some(start) = msg.find('\'')
    && let Some(end) = msg[start + 1..].find('\'')
  {
    return Some(msg[start + 1..start + 1 + end].to_string());
  }
  None
}

fn parse_fail_type(msg: &str) -> FailType {
  if msg.contains("timeout") {
    FailType::Timeout
  } else if msg.contains("hash mismatch") || msg.contains("hash") {
    FailType::HashMismatch
  } else if msg.contains("dependency failed") {
    FailType::DependencyFailed
  } else {
    FailType::Unknown
  }
}

fn find_derivation_by_activity(
  state: &State,
  activity_id: Id,
) -> Option<DerivationId> {
  // Try to find in running builds first
  for (drv_id, build_info) in &state.full_summary.running_builds {
    if build_info.activity_id == Some(activity_id) {
      return Some(*drv_id);
    }
  }

  // Search through all derivations
  for (drv_id, info) in &state.derivation_infos {
    match &info.build_status {
      BuildStatus::Building(build_info)
        if build_info.activity_id == Some(activity_id) =>
      {
        return Some(*drv_id);
      },
      BuildStatus::Built { info, .. }
        if info.activity_id == Some(activity_id) =>
      {
        return Some(*drv_id);
      },
      BuildStatus::Failed { info, .. }
        if info.activity_id == Some(activity_id) =>
      {
        return Some(*drv_id);
      },
      _ => {},
    }
  }

  None
}
