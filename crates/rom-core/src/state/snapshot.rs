use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use super::{
  ActivityId,
  ActivityStatus,
  BuildStatus,
  DependencySummary,
  DerivationId,
  DerivationInfo,
  EvalInfo,
  State,
  StorePathId,
  StorePathInfo,
  current_time,
};
use crate::state::ProgressState;

const MAX_RENDER_SNAPSHOT_ROOTS: usize = 256;
const RENDER_COMPLETED_LINGER_SECONDS: f64 = 3.0;

/// Pruned, read-only state model for live rendering.
///
/// This intentionally does not carry mutation indexes, caches, traces, or
/// final-error diagnostics. Code that needs the full build state should keep
/// using [`State`].
#[derive(Debug, Clone)]
pub struct RenderSnapshot {
  pub derivation_infos: IndexMap<DerivationId, DerivationInfo>,
  pub store_path_infos: IndexMap<StorePathId, StorePathInfo>,
  pub full_summary:     DependencySummary,
  pub forest_roots:     Vec<DerivationId>,
  pub start_time:       f64,
  pub progress_state:   ProgressState,
  pub activities:       HashMap<ActivityId, ActivityStatus>,
  pub build_platform:   Option<String>,
  pub evaluation_state: EvalInfo,
  pub builds_activity:  Option<ActivityId>,

  derivation_name_index: HashMap<String, HashSet<DerivationId>>,
}

impl State {
  #[must_use]
  pub fn render_snapshot(&self) -> RenderSnapshot {
    let now = current_time();
    let focus_ids = self.render_focus_derivations(now);
    let forest_roots = self.render_forest_roots(&focus_ids);
    let derivation_infos =
      self.render_derivation_infos(&focus_ids, &forest_roots);
    let derivation_name_index = derivation_name_index(&derivation_infos);
    let activities = self.render_activities(&derivation_infos);

    RenderSnapshot {
      derivation_infos,
      store_path_infos: self.render_store_path_infos(),
      full_summary: self.render_dependency_summary(),
      forest_roots,
      start_time: self.start_time,
      progress_state: self.progress_state.clone(),
      derivation_name_index,
      activities,
      build_platform: self.build_platform.clone(),
      evaluation_state: self.evaluation_state.clone(),
      builds_activity: self.builds_activity,
    }
  }

  fn render_dependency_summary(&self) -> DependencySummary {
    DependencySummary {
      planned_builds:      self.full_summary.planned_builds.clone(),
      running_builds:      self.full_summary.running_builds.clone(),
      completed_builds:    self.full_summary.completed_builds.clone(),
      failed_builds:       self.full_summary.failed_builds.clone(),
      planned_downloads:   self.full_summary.planned_downloads.clone(),
      completed_downloads: HashMap::new(),
      completed_uploads:   HashMap::new(),
      running_downloads:   self.full_summary.running_downloads.clone(),
      running_uploads:     self.full_summary.running_uploads.clone(),
    }
  }

  fn render_focus_derivations(&self, now: f64) -> HashSet<DerivationId> {
    let mut focus = HashSet::new();

    focus.extend(self.full_summary.failed_builds.keys().copied());
    focus.extend(self.full_summary.running_builds.keys().copied());
    focus.extend(self.full_summary.completed_builds.iter().filter_map(
      |(drv_id, build)| {
        (now - build.end < RENDER_COMPLETED_LINGER_SECONDS).then_some(*drv_id)
      },
    ));

    for path_id in self
      .full_summary
      .running_downloads
      .keys()
      .chain(self.full_summary.planned_downloads.iter())
    {
      if let Some(producer) = self
        .store_path_infos
        .get(path_id)
        .and_then(|info| info.producer)
      {
        focus.insert(producer);
      }
    }

    let mut stack = focus.iter().copied().collect::<Vec<_>>();
    while let Some(drv_id) = stack.pop() {
      let Some(info) = self.derivation_infos.get(&drv_id) else {
        continue;
      };
      for parent_id in &info.derivation_parents {
        if focus.insert(*parent_id) {
          stack.push(*parent_id);
        }
      }
    }

    focus
  }

  fn render_forest_roots(
    &self,
    focus_ids: &HashSet<DerivationId>,
  ) -> Vec<DerivationId> {
    let mut roots = if self.forest_roots.is_empty() {
      let mut roots = Vec::new();
      roots.extend(self.full_summary.failed_builds.keys().copied());
      roots.extend(self.full_summary.running_builds.keys().copied());
      roots.extend(self.full_summary.planned_builds.iter().copied());
      roots
    } else {
      self.forest_roots.clone()
    };

    roots.extend(focus_ids.iter().copied());
    dedup_derivation_ids(&mut roots);
    if roots.len() <= MAX_RENDER_SNAPSHOT_ROOTS {
      return roots;
    }

    let mut selected = Vec::with_capacity(MAX_RENDER_SNAPSHOT_ROOTS);
    selected.extend(
      roots
        .iter()
        .filter(|id| focus_ids.contains(id))
        .take(MAX_RENDER_SNAPSHOT_ROOTS)
        .copied(),
    );

    let remaining = MAX_RENDER_SNAPSHOT_ROOTS.saturating_sub(selected.len());
    if remaining > 0 {
      let front_len = remaining.div_ceil(2).min(roots.len());
      let tail_len = remaining.saturating_sub(front_len);
      let tail_start = roots.len().saturating_sub(tail_len);
      selected.extend(roots.iter().take(front_len).copied());
      selected.extend(roots.iter().skip(tail_start).copied());
    }

    dedup_derivation_ids(&mut selected);
    selected.truncate(MAX_RENDER_SNAPSHOT_ROOTS);
    selected
  }

  fn render_derivation_infos(
    &self,
    focus_ids: &HashSet<DerivationId>,
    forest_roots: &[DerivationId],
  ) -> IndexMap<DerivationId, DerivationInfo> {
    let mut render_ids = focus_ids.clone();
    render_ids.extend(forest_roots.iter().copied());

    let mut snapshot_ids = render_ids.clone();
    for drv_id in &render_ids {
      if let Some(info) = self.derivation_infos.get(drv_id) {
        snapshot_ids
          .extend(info.input_derivations.iter().map(|input| input.derivation));
      }
    }

    let mut ids = snapshot_ids.iter().copied().collect::<Vec<_>>();
    ids.sort_unstable();
    ids
      .into_iter()
      .filter_map(|drv_id| {
        let mut info = self.derivation_infos.get(&drv_id)?.clone();
        info
          .input_derivations
          .retain(|input| snapshot_ids.contains(&input.derivation));
        info
          .derivation_parents
          .retain(|parent_id| snapshot_ids.contains(parent_id));
        if !render_ids.contains(&drv_id) {
          info.input_derivations.clear();
        }

        Some((drv_id, info))
      })
      .collect()
  }

  fn render_activities(
    &self,
    derivation_infos: &IndexMap<DerivationId, DerivationInfo>,
  ) -> HashMap<ActivityId, ActivityStatus> {
    let mut ids = HashSet::new();
    for info in derivation_infos.values() {
      match &info.build_status {
        BuildStatus::Building(build)
        | BuildStatus::Built { info: build, .. }
        | BuildStatus::Failed { info: build, .. } => {
          if let Some(activity_id) = build.activity_id {
            ids.insert(activity_id);
          }
        },
        BuildStatus::Unknown | BuildStatus::Planned => {},
      }
    }

    ids
      .into_iter()
      .filter_map(|id| {
        self.activities.get(&id).cloned().map(|status| (id, status))
      })
      .collect()
  }

  fn render_store_path_infos(&self) -> IndexMap<StorePathId, StorePathInfo> {
    let summary = &self.full_summary;
    let mut ids = HashSet::new();
    ids.extend(summary.planned_downloads.iter().copied());
    ids.extend(summary.running_downloads.keys().copied());
    ids.extend(summary.running_uploads.keys().copied());

    ids
      .into_iter()
      .filter_map(|id| {
        self
          .store_path_infos
          .get(&id)
          .map(|info| (id, info.clone()))
      })
      .collect()
  }
}

impl RenderSnapshot {
  #[must_use]
  pub fn get_derivation_info(
    &self,
    id: DerivationId,
  ) -> Option<&DerivationInfo> {
    self.derivation_infos.get(&id)
  }

  #[must_use]
  pub fn get_store_path_info(&self, id: StorePathId) -> Option<&StorePathInfo> {
    self.store_path_infos.get(&id)
  }

  #[must_use]
  pub fn derivation_ids_with_name(&self, name: &str) -> Vec<DerivationId> {
    self
      .derivation_name_index
      .get(name)
      .into_iter()
      .flat_map(|ids| ids.iter().copied())
      .collect()
  }
}

fn derivation_name_index(
  derivation_infos: &IndexMap<DerivationId, DerivationInfo>,
) -> HashMap<String, HashSet<DerivationId>> {
  let mut index: HashMap<String, HashSet<DerivationId>> = HashMap::new();
  for (id, info) in derivation_infos {
    index.entry(info.name.name.clone()).or_default().insert(*id);
  }
  index
}

fn dedup_derivation_ids(ids: &mut Vec<DerivationId>) {
  let mut seen = HashSet::new();
  ids.retain(|id| seen.insert(*id));
}
