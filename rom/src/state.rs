//! State management for ROM
use std::{
  collections::{HashMap, HashSet},
  path::PathBuf,
  time::{Duration, SystemTime},
};

pub use cognos::ProgressState;
use cognos::{Activities, Host, Id, OutputName};
use indexmap::IndexMap;

/// Unique identifier for store paths
pub type StorePathId = usize;

/// Unique identifier for derivations
pub type DerivationId = usize;

/// Unique identifier for activities
pub type ActivityId = Id;

/// Store path representation
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorePath {
  pub path: PathBuf,
  pub hash: String,
  pub name: String,
}

impl StorePath {
  #[must_use]
  pub fn parse(path: &str) -> Option<Self> {
    if !path.starts_with("/nix/store/") {
      return None;
    }

    let path_buf = PathBuf::from(path);
    let file_name = path_buf.file_name()?.to_str()?;

    let parts: Vec<&str> = file_name.splitn(2, '-').collect();
    if parts.len() != 2 {
      return None;
    }

    Some(Self {
      path: path_buf.clone(),
      hash: parts[0].to_string(),
      name: parts[1].to_string(),
    })
  }
}

/// Derivation representation
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Derivation {
  pub path: PathBuf,
  pub name: String,
}

impl Derivation {
  #[must_use]
  pub fn parse(path: &str) -> Option<Self> {
    let path_buf = PathBuf::from(path);
    let file_name = path_buf.file_name()?.to_str()?;

    if !file_name.ends_with(".drv") {
      return None;
    }

    let name = file_name.strip_suffix(".drv")?;
    let parts: Vec<&str> = name.splitn(2, '-').collect();
    let display_name = if parts.len() == 2 {
      parts[1].to_string()
    } else {
      name.to_string()
    };

    Some(Self {
      path: path_buf,
      name: display_name,
    })
  }
}

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
  pub name:               Derivation,
  pub outputs:            HashMap<OutputName, StorePathId>,
  pub input_derivations:  Vec<InputDerivation>,
  pub input_sources:      HashSet<StorePathId>,
  pub build_status:       BuildStatus,
  pub derivation_parents: HashSet<DerivationId>,
  pub pname:              Option<String>,
  pub platform:           Option<String>,
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
  pub(crate) fn clear_derivation(
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

  pub(crate) fn update_derivation(
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
  pub activity:   Activities,
  pub parent:     Option<ActivityId>,
  pub derivation: Option<DerivationId>,
  pub store_path: Option<StorePathId>,
  pub phase:      Option<String>,
  pub progress:   Option<ActivityProgress>,
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

/// Main state for ROM
#[derive(Debug, Clone)]
pub struct State {
  derivation_infos:   IndexMap<DerivationId, DerivationInfo>,
  store_path_infos:   IndexMap<StorePathId, StorePathInfo>,
  full_summary:       DependencySummary,
  forest_roots:       Vec<DerivationId>,
  build_cache:        HashMap<(String, String), Vec<BuildReport>>,
  start_time:         f64,
  progress_state:     ProgressState,
  store_path_ids:     HashMap<StorePath, StorePathId>,
  derivation_ids:     HashMap<Derivation, DerivationId>,
  activities:         HashMap<ActivityId, ActivityStatus>,
  nix_errors:         Vec<String>,
  nix_error_count:    usize,
  next_store_path_id: StorePathId,
  next_derivation_id: DerivationId,
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
      derivation_infos:   IndexMap::new(),
      store_path_infos:   IndexMap::new(),
      full_summary:       DependencySummary::default(),
      forest_roots:       Vec::new(),
      build_cache:        HashMap::new(),
      start_time:         current_time(),
      progress_state:     ProgressState::JustStarted,
      store_path_ids:     HashMap::new(),
      derivation_ids:     HashMap::new(),
      activities:         HashMap::new(),
      nix_errors:         Vec::new(),
      nix_error_count:    0,
      next_store_path_id: 0,
      next_derivation_id: 0,
    }
  }

  #[must_use]
  pub fn progress_state(&self) -> ProgressState {
    self.progress_state.clone()
  }

  #[must_use]
  pub const fn start_time(&self) -> f64 {
    self.start_time
  }

  #[must_use]
  pub const fn summary(&self) -> &DependencySummary {
    &self.full_summary
  }

  #[must_use]
  pub fn roots(&self) -> &[DerivationId] {
    &self.forest_roots
  }

  #[must_use]
  pub const fn derivations(&self) -> &IndexMap<DerivationId, DerivationInfo> {
    &self.derivation_infos
  }

  #[must_use]
  pub const fn store_paths(&self) -> &IndexMap<StorePathId, StorePathInfo> {
    &self.store_path_infos
  }

  #[must_use]
  pub const fn error_count(&self) -> usize {
    self.nix_error_count
  }

  pub(crate) fn begin_at(&mut self, now: f64) -> bool {
    if self.progress_state != ProgressState::JustStarted {
      return false;
    }
    self.start_time = now;
    self.progress_state = ProgressState::InputReceived;
    true
  }

  pub(crate) fn finish(&mut self) {
    self.progress_state = ProgressState::Finished;
  }

  pub(crate) fn replace_build_history(
    &mut self,
    history: HashMap<(String, String), Vec<BuildReport>>,
  ) {
    self.build_cache = history;
  }

  pub(crate) fn build_history(
    &self,
  ) -> &HashMap<(String, String), Vec<BuildReport>> {
    &self.build_cache
  }

  pub(crate) fn build_reports(
    &self,
    host: &Host,
    derivation: &str,
  ) -> Option<&[BuildReport]> {
    self
      .build_cache
      .get(&(host.name().to_string(), derivation.to_string()))
      .map(Vec::as_slice)
  }

  pub(crate) fn record_build_report(&mut self, report: BuildReport) {
    self
      .build_cache
      .entry((report.host.clone(), report.derivation_name.clone()))
      .or_default()
      .push(report);
  }

  pub(crate) fn get_or_create_store_path_id(
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

  pub(crate) fn get_or_create_derivation_id(
    &mut self,
    drv: Derivation,
  ) -> DerivationId {
    if let Some(&id) = self.derivation_ids.get(&drv) {
      return id;
    }

    let id = self.next_derivation_id;
    self.next_derivation_id += 1;

    self.derivation_infos.insert(id, DerivationInfo {
      name:               drv.clone(),
      outputs:            HashMap::new(),
      input_derivations:  Vec::new(),
      input_sources:      HashSet::new(),
      build_status:       BuildStatus::Unknown,
      derivation_parents: HashSet::new(),
      pname:              None,
      platform:           None,
    });
    self.derivation_ids.insert(drv, id);

    id
  }

  /// Apply derivation metadata returned by an injected resolver.
  pub(crate) fn populate_parsed_derivation(
    &mut self,
    drv_id: DerivationId,
    parsed: cognos::ParsedDerivation,
  ) {
    // Extract metadata
    if let Some(pname) = cognos::extract_pname(&parsed.env)
      && let Some(info) = self.get_derivation_info_mut(drv_id)
    {
      info.pname = Some(pname);
    }

    if let Some(info) = self.get_derivation_info_mut(drv_id) {
      info.platform = Some(parsed.platform);
    }

    // Register the derivation's output store paths
    for (output_name, store_path_str) in &parsed.outputs {
      if let Some(sp) = StorePath::parse(store_path_str) {
        let sp_id = self.get_or_create_store_path_id(sp);
        if let Some(sp_info) = self.get_store_path_info_mut(sp_id) {
          sp_info.producer = Some(drv_id);
        }
        if let Some(drv_info) = self.get_derivation_info_mut(drv_id) {
          drv_info
            .outputs
            .insert(cognos::OutputName::parse(output_name), sp_id);
        }
      }
    }

    // Cached inputs remain `Unknown`; only explicit protocol announcements
    // promote them to `Planned` and make them visible.
    for (input_drv_path, outputs) in parsed.input_drvs {
      if let Some(input_drv) = Derivation::parse(&input_drv_path) {
        let input_drv_id = self.get_or_create_derivation_id(input_drv);
        let outputs = outputs
          .into_iter()
          .map(|output| OutputName::parse(&output))
          .collect();
        self.link_dependency(drv_id, input_drv_id, outputs);
      }
    }
    self.ensure_root(drv_id);
  }

  pub(crate) fn ensure_root(&mut self, id: DerivationId) {
    let is_root = self
      .get_derivation_info(id)
      .is_some_and(|info| info.derivation_parents.is_empty());
    if is_root && !self.forest_roots.contains(&id) {
      self.forest_roots.push(id);
    }
  }

  pub(crate) fn link_dependency(
    &mut self,
    parent: DerivationId,
    child: DerivationId,
    outputs: HashSet<OutputName>,
  ) -> bool {
    let inserted = self.get_derivation_info_mut(parent).is_some_and(|info| {
      if info
        .input_derivations
        .iter()
        .any(|input| input.derivation == child)
      {
        false
      } else {
        info.input_derivations.push(InputDerivation {
          derivation: child,
          outputs,
        });
        true
      }
    });
    if inserted {
      if let Some(info) = self.get_derivation_info_mut(child) {
        info.derivation_parents.insert(parent);
      }
      self.forest_roots.retain(|id| *id != child);
    }
    inserted
  }

  #[must_use]
  pub fn get_derivation_info(
    &self,
    id: DerivationId,
  ) -> Option<&DerivationInfo> {
    self.derivation_infos.get(&id)
  }

  pub(crate) fn get_derivation_info_mut(
    &mut self,
    id: DerivationId,
  ) -> Option<&mut DerivationInfo> {
    self.derivation_infos.get_mut(&id)
  }

  #[must_use]
  pub fn get_store_path_info(&self, id: StorePathId) -> Option<&StorePathInfo> {
    self.store_path_infos.get(&id)
  }

  pub(crate) fn get_store_path_info_mut(
    &mut self,
    id: StorePathId,
  ) -> Option<&mut StorePathInfo> {
    self.store_path_infos.get_mut(&id)
  }

  pub(crate) fn plan_derivation(
    &mut self,
    derivation: Derivation,
  ) -> (DerivationId, bool) {
    let id = self.get_or_create_derivation_id(derivation);
    let changed = self
      .get_derivation_info(id)
      .is_some_and(|info| matches!(info.build_status, BuildStatus::Unknown));
    if changed {
      self.update_build_status(id, BuildStatus::Planned);
    }
    self.ensure_root(id);
    (id, changed)
  }

  pub(crate) fn plan_store_path(&mut self, path: StorePath) -> bool {
    let id = self.get_or_create_store_path_id(path);
    if self.full_summary.running_downloads.contains_key(&id)
      || self.full_summary.completed_downloads.contains_key(&id)
    {
      false
    } else {
      self.full_summary.planned_downloads.insert(id)
    }
  }

  pub(crate) fn start_build(
    &mut self,
    derivation: Derivation,
    build: BuildInfo,
    parent: Option<DerivationId>,
  ) -> DerivationId {
    let id = self.get_or_create_derivation_id(derivation);
    self.update_build_status(id, BuildStatus::Building(build));
    if let Some(parent) = parent {
      self.link_dependency(parent, id, HashSet::new());
    } else {
      self.ensure_root(id);
    }
    id
  }

  pub(crate) fn start_download(
    &mut self,
    path: StorePath,
    transfer: TransferInfo,
    producer: Option<DerivationId>,
  ) -> StorePathId {
    let id = self.get_or_create_store_path_id(path);
    self.full_summary.planned_downloads.remove(&id);
    if let Some(producer) = producer
      && let Some(info) = self.get_store_path_info_mut(id)
    {
      info.producer = Some(producer);
    }
    self.full_summary.running_downloads.insert(id, transfer);
    id
  }

  pub(crate) fn start_upload(
    &mut self,
    path: StorePath,
    transfer: TransferInfo,
  ) -> StorePathId {
    let id = self.get_or_create_store_path_id(path);
    self.full_summary.planned_downloads.remove(&id);
    self.full_summary.running_uploads.insert(id, transfer);
    id
  }

  pub(crate) fn complete_download(
    &mut self,
    path: StorePathId,
    now: f64,
  ) -> bool {
    let Some(transfer) = self.full_summary.running_downloads.remove(&path)
    else {
      return false;
    };
    self
      .full_summary
      .completed_downloads
      .insert(path, complete_transfer(transfer, now));
    true
  }

  pub(crate) fn complete_transfer(
    &mut self,
    path: StorePathId,
    now: f64,
  ) -> bool {
    if self.complete_download(path, now) {
      return true;
    }
    let Some(transfer) = self.full_summary.running_uploads.remove(&path) else {
      return false;
    };
    self
      .full_summary
      .completed_uploads
      .insert(path, complete_transfer(transfer, now));
    true
  }

  pub(crate) fn insert_activity(
    &mut self,
    id: ActivityId,
    activity: ActivityStatus,
  ) {
    self.activities.insert(id, activity);
  }

  pub(crate) fn take_activity(
    &mut self,
    id: ActivityId,
  ) -> Option<ActivityStatus> {
    self.activities.remove(&id)
  }

  pub(crate) fn activity_derivation(
    &self,
    id: ActivityId,
  ) -> Option<DerivationId> {
    self.activities.get(&id)?.derivation
  }

  pub(crate) fn set_activity_phase(
    &mut self,
    id: ActivityId,
    phase: String,
  ) -> bool {
    let Some(activity) = self.activities.get_mut(&id) else {
      return false;
    };
    if activity.phase.as_deref() == Some(&phase) {
      return false;
    }
    activity.phase = Some(phase);
    true
  }

  #[must_use]
  pub(crate) fn activity_phase(&self, id: ActivityId) -> Option<&str> {
    self.activities.get(&id)?.phase.as_deref()
  }

  pub(crate) fn update_activity_progress(
    &mut self,
    id: ActivityId,
    progress: ActivityProgress,
  ) -> bool {
    let Some(activity) = self.activities.get_mut(&id) else {
      return false;
    };
    activity.progress = Some(progress);

    for transfer in self
      .full_summary
      .running_downloads
      .values_mut()
      .chain(self.full_summary.running_uploads.values_mut())
      .filter(|transfer| transfer.activity_id == id)
    {
      transfer.bytes_transferred = progress.done;
      transfer.total_bytes =
        (progress.expected > 0).then_some(progress.expected);
    }
    true
  }

  pub(crate) fn update_build_status(
    &mut self,
    id: DerivationId,
    new_status: BuildStatus,
  ) {
    if let Some(info) = self.derivation_infos.get_mut(&id) {
      let old_status =
        std::mem::replace(&mut info.build_status, new_status.clone());
      self.full_summary.clear_derivation(id, &old_status);
      self.full_summary.update_derivation(id, &new_status);
    }
  }

  pub(crate) fn record_error(&mut self, error: String) {
    self.nix_error_count += 1;
    if self.nix_errors.len() < 128 {
      self.nix_errors.push(error);
    }
  }

  #[must_use]
  pub fn has_errors(&self) -> bool {
    self.nix_error_count > 0 || !self.full_summary.failed_builds.is_empty()
  }

  #[must_use]
  pub fn has_unfinished(&self) -> bool {
    !self.full_summary.running_builds.is_empty()
      || !self.full_summary.running_downloads.is_empty()
      || !self.full_summary.running_uploads.is_empty()
  }

  #[must_use]
  pub fn total_builds(&self) -> usize {
    self.full_summary.planned_builds.len()
      + self.full_summary.running_builds.len()
      + self.full_summary.completed_builds.len()
      + self.full_summary.failed_builds.len()
  }

  /// Get the activity prefix for a given activity ID by walking up the parent
  /// chain to find a Build activity and extracting its derivation name.
  /// Returns a prefix like "hello> " suitable for prepending to log lines.
  /// The `prefix_style` determines whether to use short (pname only), full, or
  /// no prefix.
  #[must_use]
  pub(crate) fn get_activity_prefix(
    &self,
    activity_id: ActivityId,
    prefix_style: &crate::types::LogPrefixStyle,
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
        if activity.activity == Activities::Build
          && let Some(drv_id) = activity.derivation
          && let Some(info) = self.derivation_infos.get(&drv_id)
        {
          let name = if matches!(prefix_style, LogPrefixStyle::Short) {
            info.pname.as_ref().unwrap_or(&info.name.name)
          } else {
            &info.name.name
          };
          return Some(format!("{name}> "));
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

fn complete_transfer(
  transfer: TransferInfo,
  now: f64,
) -> CompletedTransferInfo {
  CompletedTransferInfo {
    start:       transfer.start,
    end:         now,
    host:        transfer.host,
    total_bytes: transfer.bytes_transferred,
  }
}

#[must_use]
pub fn current_time() -> f64 {
  SystemTime::now()
    .duration_since(SystemTime::UNIX_EPOCH)
    .unwrap_or(Duration::ZERO)
    .as_secs_f64()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn store_path_ids_are_stable() {
    let mut state = State::new();
    let path = StorePath::parse("/nix/store/abc123-hello-1.0").unwrap();
    let first = state.get_or_create_store_path_id(path.clone());
    let second = state.get_or_create_store_path_id(path);
    assert_eq!(first, second);
  }
}
