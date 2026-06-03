//! State management for ROM
mod identity;
mod snapshot;

use std::{
  collections::{HashMap, HashSet, VecDeque},
  time::{Duration, SystemTime},
};

pub use cognos::ProgressState;
use cognos::{Host, Id, OutputName};
pub use identity::{Derivation, StorePath};
use indexmap::IndexMap;
pub use snapshot::RenderSnapshot;

const MAX_RETAINED_TRACES: usize = 1_000;

/// Unique identifier for store paths
pub type StorePathId = usize;

/// Unique identifier for derivations
pub type DerivationId = usize;

/// Unique identifier for activities
pub type ActivityId = Id;

/// Transfer information (download/upload)
#[derive(Debug, Clone)]
pub struct TransferInfo {
  pub start:             f64,
  pub host:              Host,
  pub activity_id:       ActivityId,
  pub bytes_transferred: u64,
  pub total_bytes:       Option<u64>,
}

/// Completed transfer information
#[derive(Debug, Clone)]
pub struct CompletedTransferInfo {
  pub start:       f64,
  pub end:         f64,
  pub host:        Host,
  pub total_bytes: u64,
}

/// Store path information
#[derive(Debug, Clone)]
pub struct StorePathInfo {
  pub name:      StorePath,
  pub producer:  Option<DerivationId>,
  pub input_for: HashSet<DerivationId>,
}

/// Build information
#[derive(Debug, Clone)]
pub struct BuildInfo {
  pub start:       f64,
  pub host:        Host,
  pub estimate:    Option<u64>,
  pub activity_id: Option<ActivityId>,
}

/// Build failure information
#[derive(Debug, Clone)]
pub struct BuildFail {
  pub at:        f64,
  pub fail_type: FailType,
}

/// Failure type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailType {
  BuildFailed(i32),
  Timeout,
  HashMismatch,
  DependencyFailed,
  Unknown,
}

/// Build status
#[derive(Debug, Clone)]
pub enum BuildStatus {
  Unknown,
  Planned,
  Building(BuildInfo),
  Built { info: BuildInfo, end: f64 },
  Failed { info: BuildInfo, fail: BuildFail },
}

/// Input derivation for dependency tracking
#[derive(Debug, Clone)]
pub struct InputDerivation {
  pub derivation: DerivationId,
  pub outputs:    HashSet<OutputName>,
}

/// Derivation information
#[derive(Debug, Clone)]
pub struct DerivationInfo {
  pub name:                   Derivation,
  pub outputs:                HashMap<OutputName, StorePathId>,
  pub input_derivations:      Vec<InputDerivation>,
  pub input_sources:          HashSet<StorePathId>,
  pub build_status:           BuildStatus,
  pub dependency_summary:     DependencySummary,
  pub dependencies_populated: bool,
  pub cached:                 bool,
  pub derivation_parents:     HashSet<DerivationId>,
  pub pname:                  Option<String>,
  pub platform:               Option<String>,
}

/// Dependency summary for tracking build progress
#[derive(Debug, Clone, Default)]
pub struct DependencySummary {
  pub planned_builds:      HashSet<DerivationId>,
  pub running_builds:      HashMap<DerivationId, BuildInfo>,
  pub completed_builds:    HashMap<DerivationId, CompletedBuildInfo>,
  pub failed_builds:       HashMap<DerivationId, FailedBuildInfo>,
  pub planned_downloads:   HashSet<StorePathId>,
  pub completed_downloads: HashMap<StorePathId, CompletedTransferInfo>,
  pub completed_uploads:   HashMap<StorePathId, CompletedTransferInfo>,
  pub running_downloads:   HashMap<StorePathId, TransferInfo>,
  pub running_uploads:     HashMap<StorePathId, TransferInfo>,
}

impl DependencySummary {
  pub fn merge(&mut self, other: &Self) {
    self
      .planned_builds
      .extend(other.planned_builds.iter().copied());
    self
      .running_builds
      .extend(other.running_builds.iter().map(|(k, v)| (*k, v.clone())));
    self
      .completed_builds
      .extend(other.completed_builds.iter().map(|(k, v)| (*k, v.clone())));
    self
      .failed_builds
      .extend(other.failed_builds.iter().map(|(k, v)| (*k, v.clone())));
    self
      .planned_downloads
      .extend(other.planned_downloads.iter().copied());
    self.completed_downloads.extend(
      other
        .completed_downloads
        .iter()
        .map(|(k, v)| (*k, v.clone())),
    );
    self
      .completed_uploads
      .extend(other.completed_uploads.iter().map(|(k, v)| (*k, v.clone())));
    self
      .running_downloads
      .extend(other.running_downloads.iter().map(|(k, v)| (*k, v.clone())));
    self
      .running_uploads
      .extend(other.running_uploads.iter().map(|(k, v)| (*k, v.clone())));
  }

  pub fn clear_derivation(
    &mut self,
    id: DerivationId,
    old_status: &BuildStatus,
  ) {
    match old_status {
      BuildStatus::Unknown => {},
      BuildStatus::Planned => {
        self.planned_builds.remove(&id);
      },
      BuildStatus::Building(_) => {
        self.running_builds.remove(&id);
      },
      BuildStatus::Built { .. } => {
        self.completed_builds.remove(&id);
      },
      BuildStatus::Failed { .. } => {
        self.failed_builds.remove(&id);
      },
    }
  }

  pub fn update_derivation(
    &mut self,
    id: DerivationId,
    new_status: &BuildStatus,
  ) {
    match new_status {
      BuildStatus::Unknown => {},
      BuildStatus::Planned => {
        self.planned_builds.insert(id);
      },
      BuildStatus::Building(info) => {
        self.running_builds.insert(id, info.clone());
      },
      BuildStatus::Built { info, end } => {
        self.completed_builds.insert(id, CompletedBuildInfo {
          start: info.start,
          end:   *end,
          host:  info.host.clone(),
        });
      },
      BuildStatus::Failed { info, fail } => {
        self.failed_builds.insert(id, FailedBuildInfo {
          start:     info.start,
          end:       fail.at,
          host:      info.host.clone(),
          fail_type: fail.fail_type.clone(),
        });
      },
    }
  }
}

/// Completed build information
#[derive(Debug, Clone)]
pub struct CompletedBuildInfo {
  pub start: f64,
  pub end:   f64,
  pub host:  Host,
}

/// Failed build information
#[derive(Debug, Clone)]
pub struct FailedBuildInfo {
  pub start:     f64,
  pub end:       f64,
  pub host:      Host,
  pub fail_type: FailType,
}

/// Activity status tracking
#[derive(Debug, Clone)]
pub struct ActivityStatus {
  pub activity: u8,
  pub text:     String,
  pub parent:   Option<ActivityId>,
  pub phase:    Option<String>,
  pub progress: Option<ActivityProgress>,
}

/// Activity progress for downloads/uploads/builds
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityProgress {
  /// Bytes completed
  pub done:     u64,
  /// Total bytes expected
  pub expected: u64,
  /// Currently running transfers
  pub running:  u64,
  /// Failed transfers
  pub failed:   u64,
}

/// Build report for caching
#[derive(Debug, Clone)]
pub struct BuildReport {
  pub derivation_name: String,
  pub platform:        String,
  pub duration_secs:   f64,
  pub completed_at:    SystemTime,
  pub host:            String,
  pub success:         bool,
}

/// Evaluation information
#[derive(Debug, Clone, Default)]
pub struct EvalInfo {
  pub last_file_name: Option<String>,
  pub count:          usize,
  pub at:             f64,
}

/// Main state for ROM
#[derive(Debug, Clone)]
pub struct State {
  pub derivation_infos:  IndexMap<DerivationId, DerivationInfo>,
  pub store_path_infos:  IndexMap<StorePathId, StorePathInfo>,
  pub full_summary:      DependencySummary,
  pub forest_roots:      Vec<DerivationId>,
  pub build_cache:       HashMap<(String, String), Vec<BuildReport>>,
  pub start_time:        f64,
  pub progress_state:    ProgressState,
  pub store_path_ids:    HashMap<StorePath, StorePathId>,
  pub derivation_ids:    HashMap<Derivation, DerivationId>,
  derivation_name_index: HashMap<String, HashSet<DerivationId>>,
  pub touched_ids:       HashSet<DerivationId>,
  pub activities:        HashMap<ActivityId, ActivityStatus>,
  pub nix_errors:        Vec<String>,
  pub traces:            Vec<String>,
  pub build_platform:    Option<String>,
  pub evaluation_state:  EvalInfo,
  pub builds_activity:   Option<ActivityId>,
  next_store_path_id:    StorePathId,
  next_derivation_id:    DerivationId,
}

impl Default for State {
  fn default() -> Self {
    Self::new()
  }
}

impl State {
  #[must_use]
  pub fn new() -> Self {
    Self {
      derivation_infos:      IndexMap::new(),
      store_path_infos:      IndexMap::new(),
      full_summary:          DependencySummary::default(),
      forest_roots:          Vec::new(),
      build_cache:           HashMap::new(),
      start_time:            current_time(),
      progress_state:        ProgressState::JustStarted,
      store_path_ids:        HashMap::new(),
      derivation_ids:        HashMap::new(),
      derivation_name_index: HashMap::new(),
      touched_ids:           HashSet::new(),
      activities:            HashMap::new(),
      nix_errors:            Vec::new(),
      traces:                Vec::new(),
      build_platform:        None,
      evaluation_state:      EvalInfo::default(),
      builds_activity:       None,
      next_store_path_id:    0,
      next_derivation_id:    0,
    }
  }

  #[must_use]
  pub fn with_platform(platform: Option<String>) -> Self {
    let mut state = Self::new();
    state.build_platform = platform;
    state
  }

  pub fn push_trace(&mut self, line: impl Into<String>) {
    self.traces.push(line.into());
    trim_vec_front(&mut self.traces, MAX_RETAINED_TRACES);
  }

  pub fn get_or_create_store_path_id(
    &mut self,
    path: StorePath,
  ) -> StorePathId {
    if let Some(&id) = self.store_path_ids.get(&path) {
      return id;
    }

    let id = self.next_store_path_id;
    self.next_store_path_id += 1;

    self.store_path_infos.insert(id, StorePathInfo {
      name:      path.clone(),
      producer:  None,
      input_for: HashSet::new(),
    });
    self.store_path_ids.insert(path, id);

    id
  }

  pub fn get_or_create_derivation_id(
    &mut self,
    drv: Derivation,
  ) -> DerivationId {
    if let Some(&id) = self.derivation_ids.get(&drv) {
      return id;
    }

    let id = self.next_derivation_id;
    self.next_derivation_id += 1;

    self.derivation_infos.insert(id, DerivationInfo {
      name:                   drv.clone(),
      outputs:                HashMap::new(),
      input_derivations:      Vec::new(),
      input_sources:          HashSet::new(),
      build_status:           BuildStatus::Unknown,
      dependency_summary:     DependencySummary::default(),
      dependencies_populated: false,
      cached:                 false,
      derivation_parents:     HashSet::new(),
      pname:                  None,
      platform:               None,
    });
    self
      .derivation_name_index
      .entry(drv.name.clone())
      .or_default()
      .insert(id);
    self.derivation_ids.insert(drv, id);

    id
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

  pub fn plan_derivation(&mut self, drv: Derivation) -> DerivationId {
    let drv_id = self.get_or_create_derivation_id(drv);
    let should_mark_planned =
      self.get_derivation_info(drv_id).is_some_and(|info| {
        matches!(
          info.build_status,
          BuildStatus::Unknown | BuildStatus::Planned
        )
      });

    if should_mark_planned {
      self.update_build_status(drv_id, BuildStatus::Planned);
    }

    if self.derivation_parents(drv_id).is_empty()
      && !self.forest_roots.contains(&drv_id)
    {
      self.forest_roots.push(drv_id);
    }

    drv_id
  }

  pub(crate) fn dependencies_populated(&self, drv_id: DerivationId) -> bool {
    self
      .get_derivation_info(drv_id)
      .is_some_and(|info| info.dependencies_populated)
  }

  pub(crate) fn mark_dependencies_populated(&mut self, drv_id: DerivationId) {
    if let Some(info) = self.get_derivation_info_mut(drv_id) {
      info.dependencies_populated = true;
    }
  }

  #[must_use]
  pub fn get_derivation_info(
    &self,
    id: DerivationId,
  ) -> Option<&DerivationInfo> {
    self.derivation_infos.get(&id)
  }

  pub fn get_derivation_info_mut(
    &mut self,
    id: DerivationId,
  ) -> Option<&mut DerivationInfo> {
    self.derivation_infos.get_mut(&id)
  }

  #[must_use]
  pub fn get_store_path_info(&self, id: StorePathId) -> Option<&StorePathInfo> {
    self.store_path_infos.get(&id)
  }

  pub fn get_store_path_info_mut(
    &mut self,
    id: StorePathId,
  ) -> Option<&mut StorePathInfo> {
    self.store_path_infos.get_mut(&id)
  }

  pub fn update_build_status(
    &mut self,
    id: DerivationId,
    new_status: BuildStatus,
  ) {
    if let Some(info) = self.derivation_infos.get_mut(&id) {
      let old_status =
        std::mem::replace(&mut info.build_status, new_status.clone());
      self.full_summary.clear_derivation(id, &old_status);
      self.full_summary.update_derivation(id, &new_status);
      self.touched_ids.insert(id);
    }

    self.recompute_derivation_summary(id);

    // Propagate changes up the parent chain
    self.propagate_to_parents(id);
  }

  /// Recompute a derivation's own dependency_summary based on its build_status.
  /// This does NOT include children's summaries. That's done by
  /// `propagate_to_parents`.
  fn recompute_own_summary(&mut self, id: DerivationId) {
    let info = match self.derivation_infos.get(&id) {
      Some(info) => info,
      None => return,
    };

    let mut summary = DependencySummary::default();
    summary.update_derivation(id, &info.build_status);

    if let Some(info_mut) = self.derivation_infos.get_mut(&id) {
      info_mut.dependency_summary = summary;
    }
  }

  /// Recompute a derivation's full dependency_summary by merging:
  /// 1. Its own contribution (based on build_status)
  /// 2. All its children's dependency_summaries
  pub(crate) fn recompute_derivation_summary(&mut self, id: DerivationId) {
    // First, compute our own contribution
    self.recompute_own_summary(id);

    // Then merge all children's summaries
    let children_ids: Vec<DerivationId> = {
      let info = match self.derivation_infos.get(&id) {
        Some(info) => info,
        None => return,
      };
      info
        .input_derivations
        .iter()
        .map(|input| input.derivation)
        .collect()
    };

    let mut merged = DependencySummary::default();
    // Our own summary
    if let Some(info) = self.derivation_infos.get(&id) {
      merged.merge(&info.dependency_summary);
    }
    // Merge children's summaries
    for child_id in children_ids {
      if let Some(child_info) = self.derivation_infos.get(&child_id) {
        merged.merge(&child_info.dependency_summary);
      }
    }

    if let Some(info_mut) = self.derivation_infos.get_mut(&id) {
      info_mut.dependency_summary = merged;
    }
  }

  /// Propagate a status change up the parent chain by recomputing each
  /// ancestor's dependency_summary. This is for O(1) subtree aggregation.
  pub(crate) fn propagate_to_parents(&mut self, id: DerivationId) {
    // Collect all ancestors first to avoid borrowing issues
    let mut ancestors: Vec<DerivationId> = Vec::new();
    let mut current_parents = self
      .derivation_parents(id)
      .into_iter()
      .collect::<VecDeque<_>>();
    let mut visited: HashSet<DerivationId> = HashSet::new();

    while let Some(parent_id) = current_parents.pop_front() {
      if visited.insert(parent_id) {
        ancestors.push(parent_id);
        for grandparent_id in self.derivation_parents(parent_id) {
          current_parents.push_back(grandparent_id);
        }
      }
    }

    // Direct parents are collected before their parents, so this recomputes
    // summaries from the changed node toward the roots.
    for ancestor_id in ancestors {
      self.recompute_derivation_summary(ancestor_id);
    }
  }

  /// Get the parent derivations (derivations that depend on this one)
  fn derivation_parents(&self, id: DerivationId) -> Vec<DerivationId> {
    let info = match self.derivation_infos.get(&id) {
      Some(info) => info,
      None => return Vec::new(),
    };
    info.derivation_parents.iter().copied().collect()
  }

  #[must_use]
  pub fn has_errors(&self) -> bool {
    !self.nix_errors.is_empty() || !self.full_summary.failed_builds.is_empty()
  }

  #[must_use]
  pub fn total_builds(&self) -> usize {
    self.full_summary.planned_builds.len()
      + self.full_summary.running_builds.len()
      + self.full_summary.completed_builds.len()
      + self.full_summary.failed_builds.len()
  }

  #[must_use]
  pub fn running_builds_for_host(
    &self,
    host: &Host,
  ) -> Vec<(DerivationId, &BuildInfo)> {
    self
      .full_summary
      .running_builds
      .iter()
      .filter(|(_, info)| &info.host == host)
      .map(|(id, info)| (*id, info))
      .collect()
  }

  /// Check if a derivation has a platform mismatch
  #[must_use]
  pub fn has_platform_mismatch(&self, id: DerivationId) -> bool {
    if let (Some(build_platform), Some(info)) =
      (&self.build_platform, self.get_derivation_info(id))
      && let Some(drv_platform) = &info.platform
    {
      return build_platform != drv_platform;
    }
    false
  }

  /// Get all derivations with platform mismatches
  #[must_use]
  pub fn platform_mismatches(&self) -> Vec<DerivationId> {
    self
      .derivation_infos
      .keys()
      .filter(|&&id| self.has_platform_mismatch(id))
      .copied()
      .collect()
  }

  /// Get the activity prefix for a given activity ID by walking up the parent
  /// chain to find a Build activity and extracting its derivation name.
  /// Returns a prefix like "hello> " suitable for prepending to log lines.
  /// If `use_color` is true and stderr is a TTY, the prefix will be blue.
  /// The `prefix_style` determines whether to use short (pname only), full, or
  /// no prefix.
  #[must_use]
  pub fn get_activity_prefix(
    &self,
    activity_id: ActivityId,
    prefix_style: &crate::types::LogPrefixStyle,
    use_color: bool,
  ) -> Option<String> {
    use cognos::Activities;

    use crate::types::LogPrefixStyle;

    // If prefix style is None, return empty string
    if matches!(prefix_style, LogPrefixStyle::None) {
      return Some(String::new());
    }

    let mut current_id = activity_id;
    let max_depth = 10; // Prevent infinite loops
    let mut depth = 0;

    while depth < max_depth {
      if let Some(activity) = self.activities.get(&current_id) {
        // Check if this is a Build activity (type 105)
        if activity.activity == Activities::Build as u8 {
          // Extract derivation path from the text field
          // The text field typically contains something like:
          // "building '/nix/store/...-hello-2.10.drv'"
          if let Some(drv) = extract_derivation_from_text(&activity.text) {
            // Look up the DerivationInfo for this derivation
            let drv_id = self.derivation_ids.get(&drv);
            let name = if matches!(prefix_style, LogPrefixStyle::Short) {
              // Try to use pname if available
              if let Some(id) = drv_id {
                if let Some(drv_info) = self.derivation_infos.get(id) {
                  if let Some(pname) = &drv_info.pname {
                    pname.clone()
                  } else {
                    drv.name.clone()
                  }
                } else {
                  drv.name.clone()
                }
              } else {
                drv.name.clone()
              }
            } else {
              // Full style - use full derivation name
              drv.name.clone()
            };

            // Apply color if requested and stderr is a TTY
            let colored_name = if use_color
              && std::io::IsTerminal::is_terminal(&std::io::stderr())
            {
              format!("\x1b[34m{name}\x1b[0m")
            } else {
              name
            };

            return Some(format!("{colored_name}> "));
          }
        }

        // Move to parent activity
        if let Some(parent_id) = activity.parent {
          if parent_id == 0 {
            break; // Reached root
          }
          current_id = parent_id;
          depth += 1;
        } else {
          break;
        }
      } else {
        break;
      }
    }

    None
  }
}

/// Extract derivation from activity text like "building
/// '/nix/store/...-hello-2.10.drv'" Returns the Derivation object
fn extract_derivation_from_text(text: &str) -> Option<Derivation> {
  // Look for .drv path in text
  if let Some(start) = text.find("/nix/store/")
    && let Some(end) = text[start..].find(".drv")
  {
    let drv_path = &text[start..start + end + 4]; // Include .drv
    return Derivation::parse(drv_path);
  }
  None
}

#[must_use]
pub fn current_time() -> f64 {
  SystemTime::now()
    .duration_since(SystemTime::UNIX_EPOCH)
    .unwrap_or(Duration::ZERO)
    .as_secs_f64()
}

fn trim_vec_front<T>(items: &mut Vec<T>, max_len: usize) {
  let trim_at = max_len + max_len / 10;
  if items.len() > trim_at {
    let excess = items.len() - max_len;
    items.drain(0..excess);
  }
}
