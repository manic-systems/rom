use super::record_build_completion;
use crate::state::{
  BuildStatus,
  CompletedTransferInfo,
  DerivationId,
  InputDerivation,
  ProgressState,
  State,
  StorePathId,
  current_time,
};

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

fn subtree_sort_order(state: &State, drv_id: DerivationId) -> (u8, i64) {
  let Some(info) = state.get_derivation_info(drv_id) else {
    return (9, 0);
  };
  let summary = &info.dependency_summary;

  if let Some(fail) = summary
    .failed_builds
    .values()
    .min_by_key(|fail| (fail.end * 1_000_000.0) as i64)
  {
    return (0, (fail.end * 1_000_000.0) as i64);
  }

  if let Some(build) = summary
    .running_builds
    .values()
    .min_by_key(|build| (build.start * 1_000_000.0) as i64)
  {
    return (1, (build.start * 1_000_000.0) as i64);
  }

  if !summary.planned_builds.is_empty() {
    return (4, 0);
  }

  if !summary.completed_builds.is_empty() {
    return (6, 0);
  }

  build_sort_order(state, drv_id)
}

fn sort_key(
  state: &State,
  drv_id: DerivationId,
) -> (u8, i64, u8, i64, usize, usize, usize) {
  let (own_a, own_b) = build_sort_order(state, drv_id);
  let (sub_a, sub_b) = subtree_sort_order(state, drv_id);

  let summary = state
    .get_derivation_info(drv_id)
    .map(|info| &info.dependency_summary);

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
  inputs.sort_by_key(|input| sort_key(state, input.derivation));

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
        if let BuildStatus::Building(build) = &info.build_status {
          Some(build.clone())
        } else {
          None
        }
      });

      if let Some(build_info) = build_info {
        let name = state
          .get_derivation_info(drv_id)
          .map(|info| info.name.name.clone())
          .unwrap_or_default();
        let platform = state
          .get_derivation_info(drv_id)
          .and_then(|info| info.platform.clone());
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

pub fn maintain_state(state: &mut State, _now: f64) {
  if state.touched_ids.is_empty() {
    return;
  }

  let touched: Vec<DerivationId> = state.touched_ids.iter().copied().collect();
  for drv_id in touched {
    sort_tree_children(state, drv_id);
  }

  let mut sorted_roots = state.forest_roots.clone();
  sorted_roots.sort_by_key(|id| sort_key(state, *id));
  state.forest_roots = sorted_roots;

  state.touched_ids.clear();
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
