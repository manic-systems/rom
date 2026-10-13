//! State update logic for processing nix messages
use cognos::{
  Actions,
  Activities,
  Host,
  Id,
  ProgressState,
  ResultType,
  Verbosity,
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
    StorePathId,
    TransferInfo,
    current_time,
  },
};

/// Process a nix JSON message and update state
pub fn process_message(state: &mut State, action: Actions) -> bool {
  let now = current_time();
  let mut changed = false;

  // Mark that we've received input
  if state.progress_state == ProgressState::JustStarted {
    state.progress_state = ProgressState::InputReceived;
    changed = true;
  }

  trace!("Processing action: {:?}", action);

  match action {
    Actions::Start {
      id,
      level: _,
      parent,
      text,
      activity,
      fields,
    } => {
      changed |= handle_start(state, id, parent, text, activity, fields, now);
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

fn handle_start(
  state: &mut State,
  id: Id,
  parent: Id,
  text: String,
  activity: Activities,
  fields: Vec<serde_json::Value>,
  now: f64,
) -> bool {
  // Store activity status
  let parent_id = if parent == 0 { None } else { Some(parent) };

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
  // Store all build logs for display
  state.build_logs.push(msg.clone());

  // Extract phase from log messages like "Running phase: configurePhase"
  if let Some(phase_start) = msg.find("Running phase: ") {
    let phase_name = &msg[phase_start + 15..]; // skip "Running phase: "
    let phase = phase_name.trim().to_string();

    // Find the active build and update its phase
    for activity in state.activities.values_mut() {
      if activity.activity == 105 {
        // Build activity
        activity.phase = Some(phase.clone());
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
      false
    },
    Verbosity::Info | Verbosity::Notice => {
      // Track info messages for evaluation progress
      if msg.contains("evaluating") || msg.contains("copying") {
        // Update evaluation state
        if let Some(file_name) = extract_file_name(&msg) {
          state.evaluation_state.last_file_name = Some(file_name);
          state.evaluation_state.count += 1;
          state.evaluation_state.at = current_time();
        }
      }
      true // return true since we stored the log
    },
    Verbosity::Talkative
    | Verbosity::Chatty
    | Verbosity::Debug
    | Verbosity::Vomit => {
      // These are trace-level messages, store separately
      state.traces.push(msg);
      true
    },
    _ => {
      true // return true since we stored the log
    },
  }
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
    ResultType::BuildLogLine => {
      if let Some(line) = fields.first().and_then(|f| f.as_str()) {
        state.build_logs.push(line.to_string());
        return true;
      }
      false
    },
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
    ResultType::PostBuildLogLine => {
      if let Some(line) = fields.first().and_then(|f| f.as_str()) {
        state.build_logs.push(format!("[post-build] {line}"));
        return true;
      }
      false
    },
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
fn record_build_completion(
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

      // Parse .drv file to populate dependency tree
      state.populate_derivation_dependencies(drv_id);
      debug!(
        "After populate_derivation_dependencies, state has {} derivations",
        state.derivation_infos.len()
      );

      // Mark as forest root if no parent
      if parent_id.is_none() && !state.forest_roots.contains(&drv_id) {
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

fn build_sort_order(state: &State, drv_id: DerivationId) -> (u8, i64) {
  let Some(info) = state.get_derivation_info(drv_id) else {
    return (9, 0);
  };
  match &info.build_status {
    BuildStatus::Failed { fail, .. } => (0, (fail.at * 1_000_000.0) as i64),
    BuildStatus::Building(build_info) => {
      (1, (build_info.start * 1_000_000.0) as i64)
    },
    BuildStatus::Planned => (4, 0),
    BuildStatus::Built { end, .. } => (6, -(*end * 1_000_000.0) as i64),
    BuildStatus::Unknown => (9, 0),
  }
}

fn best_subtree_sort_order(
  state: &State,
  drv_id: DerivationId,
  depth: u8,
) -> (u8, i64) {
  let own = build_sort_order(state, drv_id);
  if depth == 0 {
    return own;
  }
  let Some(info) = state.get_derivation_info(drv_id) else {
    return own;
  };
  let children: Vec<DerivationId> = info
    .input_derivations
    .iter()
    .map(|d| d.derivation)
    .collect();
  children
    .into_iter()
    .map(|child_id| best_subtree_sort_order(state, child_id, depth - 1))
    .fold(
      own,
      |best, candidate| {
        if candidate < best { candidate } else { best }
      },
    )
}

fn sort_key(
  state: &State,
  drv_id: DerivationId,
) -> (u8, i64, u8, i64, usize, usize, usize) {
  let (own_a, own_b) = build_sort_order(state, drv_id);
  let (sub_a, sub_b) = best_subtree_sort_order(state, drv_id, 20);

  let summary = state
    .get_derivation_info(drv_id)
    .map(|i| &i.dependency_summary);

  let running_builds = summary.map_or(0, |s| s.running_builds.len());
  let running_downloads = summary.map_or(0, |s| s.running_downloads.len());
  let planned =
    summary.map_or(0, |s| s.planned_builds.len() + s.planned_downloads.len());

  (
    own_a,
    own_b,
    sub_a,
    sub_b,
    usize::MAX.saturating_sub(running_builds),
    usize::MAX.saturating_sub(running_downloads),
    planned,
  )
}

fn sort_tree_children(state: &mut State, drv_id: DerivationId) {
  let Some(info) = state.derivation_infos.get(&drv_id) else {
    return;
  };
  let mut inputs: Vec<InputDerivation> = info.input_derivations.clone();
  inputs.sort_by_key(|d| sort_key(state, d.derivation));

  if let Some(info) = state.derivation_infos.get_mut(&drv_id) {
    info.input_derivations = inputs;
  }
}

pub fn detect_local_completed_builds(state: &mut State, now: f64) -> bool {
  let local_building: Vec<DerivationId> = state
    .full_summary
    .running_builds
    .iter()
    .filter(|(_, info)| info.host == cognos::Host::Localhost)
    .map(|(id, _)| *id)
    .collect();

  let mut any_completed = false;

  for drv_id in local_building {
    let output_paths: Vec<std::path::PathBuf> = state
      .get_derivation_info(drv_id)
      .map(|info| {
        info
          .outputs
          .values()
          .filter_map(|&sp_id| {
            state
              .get_store_path_info(sp_id)
              .map(|sp_info| sp_info.name.path.clone())
          })
          .collect()
      })
      .unwrap_or_default();

    let all_exist =
      !output_paths.is_empty() && output_paths.iter().all(|p| p.exists());
    if all_exist {
      let build_info = state.get_derivation_info(drv_id).and_then(|info| {
        if let BuildStatus::Building(b) = &info.build_status {
          Some(b.clone())
        } else {
          None
        }
      });

      if let Some(build_info) = build_info {
        let name = state
          .get_derivation_info(drv_id)
          .map(|i| i.name.name.clone())
          .unwrap_or_default();
        let platform = state
          .get_derivation_info(drv_id)
          .and_then(|i| i.platform.clone());
        let start = build_info.start;
        let host = build_info.host.clone();
        state.update_build_status(drv_id, BuildStatus::Built {
          info: build_info,
          end:  now,
        });
        record_build_completion(state, name, platform, start, now, &host);
        any_completed = true;
      }
    }
  }

  any_completed
}

/// Maintain state consistency
pub fn maintain_state(state: &mut State, _now: f64) {
  if !state.touched_ids.is_empty() {
    let touched: Vec<DerivationId> =
      state.touched_ids.iter().copied().collect();
    for drv_id in touched {
      sort_tree_children(state, drv_id);
    }

    let roots: Vec<DerivationId> = state.forest_roots.clone();
    let mut sorted_roots = roots;
    sorted_roots.sort_by_key(|&id| sort_key(state, id));
    state.forest_roots = sorted_roots;

    state.touched_ids.clear();
  }
}

fn complete_build_success(state: &mut State, drv_id: DerivationId, now: f64) {
  let build_info = state.get_derivation_info(drv_id).and_then(|info| {
    if let BuildStatus::Building(build_info) = &info.build_status {
      Some(build_info.clone())
    } else {
      None
    }
  });

  if let Some(build_info) = build_info {
    state.update_build_status(drv_id, BuildStatus::Built {
      info: build_info,
      end:  now,
    });
  }
}

pub fn finish_state(state: &mut State) {
  state.progress_state = ProgressState::Finished;

  let building: Vec<DerivationId> = state
    .derivation_infos
    .iter()
    .filter_map(|(drv_id, info)| {
      if matches!(info.build_status, BuildStatus::Building(_)) {
        Some(*drv_id)
      } else {
        None
      }
    })
    .collect();

  for drv_id in building {
    complete_build_success(state, drv_id, current_time());
  }

  let downloading: Vec<StorePathId> = state
    .full_summary
    .running_downloads
    .keys()
    .copied()
    .collect();
  for path_id in downloading {
    if let Some(transfer) =
      state.full_summary.running_downloads.remove(&path_id)
    {
      state.full_summary.completed_downloads.insert(
        path_id,
        CompletedTransferInfo {
          start:       transfer.start,
          end:         current_time(),
          host:        transfer.host,
          total_bytes: transfer.total_bytes.unwrap_or(0),
        },
      );
    }
  }

  let uploading: Vec<StorePathId> =
    state.full_summary.running_uploads.keys().copied().collect();
  for path_id in uploading {
    if let Some(transfer) = state.full_summary.running_uploads.remove(&path_id)
    {
      state.full_summary.completed_uploads.insert(
        path_id,
        CompletedTransferInfo {
          start:       transfer.start,
          end:         current_time(),
          host:        transfer.host,
          total_bytes: transfer.total_bytes.unwrap_or(0),
        },
      );
    }
  }
}
