//! State management for ROM.
use std::{
  collections::{BTreeMap, HashMap, HashSet},
  path::PathBuf,
  time::{Duration, SystemTime},
};

pub use cognos::ProgressState;
use cognos::{Activities, Host, Id};
use indexmap::IndexMap;

/// Unique identifier for store paths.
pub type StorePathId = usize;

/// Unique identifier for derivations.
pub type DerivationId = usize;

/// Unique identifier for activities.
pub type ActivityId = Id;

/// Store path representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorePath {
  /// Full store path.
  pub path: PathBuf,
  /// Hash part of the store path.
  pub hash: String,
  /// Name part of the store path, after the hash.
  pub name: String,
}

impl StorePath {
  /// Parses a `/nix/store` path, returning `None` if it is not one.
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
      hash: parts[0].to_owned(),
      name: parts[1].to_owned(),
    })
  }
}

/// Derivation representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Derivation {
  /// Path of the `.drv` file.
  pub path: PathBuf,
  /// Name without the hash or the `.drv` suffix.
  pub name: String,
}

impl Derivation {
  /// Parses a `.drv` path, returning `None` if it is not one.
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
      parts[1].to_owned()
    } else {
      name.to_owned()
    };

    Some(Self {
      path: path_buf,
      name: display_name,
    })
  }
}

/// Transfer information for a download or upload.
#[derive(Debug, Clone)]
pub struct TransferInfo {
  /// Start time, in seconds since the UNIX epoch.
  pub start:             f64,
  /// Remote end of the transfer.
  pub host:              Host,
  /// Activity performing the transfer.
  pub activity_id:       ActivityId,
  /// Derivation the transfer was started for.
  pub parent:            Option<DerivationId>,
  /// Bytes transferred so far.
  pub bytes_transferred: u64,
  /// Total size, once known.
  pub total_bytes:       Option<u64>,
}

/// Completed transfer information.
#[derive(Debug, Clone)]
pub struct CompletedTransferInfo {
  /// Start time, in seconds since the UNIX epoch.
  pub start:       f64,
  /// End time, in seconds since the UNIX epoch.
  pub end:         f64,
  /// Remote end of the transfer.
  pub host:        Host,
  /// Derivation the transfer was started for.
  pub parent:      Option<DerivationId>,
  /// Total size.
  pub total_bytes: u64,
}

/// Store path information.
#[derive(Debug, Clone)]
pub struct StorePathInfo {
  /// Parsed store path.
  pub name:      StorePath,
  /// Derivation that produces this path.
  pub producer:  Option<DerivationId>,
  /// Derivations that take this path as an input.
  pub input_for: HashSet<DerivationId>,
}

/// Build information.
#[derive(Debug, Clone)]
pub struct BuildInfo {
  /// Start time, in seconds since the UNIX epoch.
  pub start:    f64,
  /// Machine running the build.
  pub host:     Host,
  /// Expected duration in seconds, from build history.
  pub estimate: Option<u64>,
  /// Current build phase, such as `buildPhase`.
  pub phase:    Option<String>,
}

/// Build failure information.
#[derive(Debug, Clone)]
pub struct BuildFail {
  /// Failure time, in seconds since the UNIX epoch.
  pub at:        f64,
  /// Cause of the failure.
  pub fail_type: FailType,
}

/// Failure type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailType {
  /// The builder exited with this status code.
  BuildFailed(i32),
  /// The build timed out.
  Timeout,
  /// An output's hash did not match the expected hash.
  HashMismatch,
  /// The cause is unknown.
  Unknown,
}

/// Build status.
#[derive(Debug, Clone)]
pub enum BuildStatus {
  /// Nothing is known about the derivation yet.
  Unknown,
  /// The derivation will be built.
  Planned,
  /// The derivation's outputs were already in the store.
  Available,
  /// The derivation is building.
  Building(BuildInfo),
  /// The derivation built successfully.
  Built {
    /// Details of the build.
    info:         BuildInfo,
    /// End time, in seconds since the UNIX epoch.
    end:          f64,
    /// Wall-clock time the build completed.
    completed_at: SystemTime,
  },
  /// The build failed.
  Failed {
    /// Details of the build.
    info: BuildInfo,
    /// Details of the failure.
    fail: BuildFail,
  },
  /// A dependency failed, so the derivation cannot build.
  DependencyFailed,
}

/// Derivation information.
#[derive(Debug, Clone)]
pub struct DerivationInfo {
  /// Parsed derivation path.
  pub name:               Derivation,
  /// Derivations this one depends on.
  pub input_derivations:  Vec<DerivationId>,
  /// Current build status.
  pub build_status:       BuildStatus,
  /// Derivations that depend on this one.
  pub derivation_parents: HashSet<DerivationId>,
  /// Package name from the derivation's environment.
  pub pname:              Option<String>,
  /// System the derivation builds for.
  pub platform:           Option<String>,
}

/// Transfer lifecycle summary.
///
/// Build lifecycle has a single source of truth in
/// [`DerivationInfo::build_status`].
#[derive(Debug, Clone, Default)]
pub struct DependencySummary {
  /// Downloads that have not started yet.
  pub planned_downloads:   HashSet<StorePathId>,
  /// Finished downloads.
  pub completed_downloads: HashMap<StorePathId, CompletedTransferInfo>,
  /// Finished uploads.
  pub completed_uploads:   HashMap<StorePathId, CompletedTransferInfo>,
  /// Downloads in progress.
  pub running_downloads:   HashMap<StorePathId, TransferInfo>,
  /// Uploads in progress.
  pub running_uploads:     HashMap<StorePathId, TransferInfo>,
}

/// Activity status tracking.
#[derive(Debug, Clone)]
pub struct ActivityStatus {
  /// Kind of activity.
  pub activity:   Activities,
  /// Parent activity.
  pub parent:     Option<ActivityId>,
  /// Derivation the activity belongs to.
  pub derivation: Option<DerivationId>,
  /// Store path the activity belongs to.
  pub store_path: Option<StorePathId>,
}

/// A local source path being copied into the store.
#[derive(Debug, Clone)]
pub struct SourceFetch {
  /// Source path being copied.
  pub source: String,
  /// Start time, in seconds since the UNIX epoch.
  pub start:  f64,
}

/// Build report for caching.
#[derive(Debug, Clone)]
pub struct BuildReport {
  /// Build duration in seconds.
  pub duration_secs: f64,
  /// Wall-clock time the build completed.
  pub completed_at:  SystemTime,
}

/// Main state for ROM.
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
  source_fetches:     BTreeMap<ActivityId, SourceFetch>,
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
  /// Creates empty state that starts now.
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
      source_fetches:     BTreeMap::new(),
      nix_errors:         Vec::new(),
      nix_error_count:    0,
      next_store_path_id: 0,
      next_derivation_id: 0,
    }
  }

  /// Returns how far the run has progressed.
  #[must_use]
  pub fn progress_state(&self) -> ProgressState {
    self.progress_state.clone()
  }

  /// Returns the start time, in seconds since the UNIX epoch.
  #[must_use]
  pub const fn start_time(&self) -> f64 {
    self.start_time
  }

  /// Returns the transfer summary.
  #[must_use]
  pub const fn summary(&self) -> &DependencySummary {
    &self.full_summary
  }

  /// Returns the derivations no other derivation depends on.
  #[must_use]
  pub fn roots(&self) -> &[DerivationId] {
    &self.forest_roots
  }

  /// Returns every known derivation.
  #[must_use]
  pub const fn derivations(&self) -> &IndexMap<DerivationId, DerivationInfo> {
    &self.derivation_infos
  }

  /// Returns every known store path.
  #[must_use]
  pub const fn store_paths(&self) -> &IndexMap<StorePathId, StorePathInfo> {
    &self.store_path_infos
  }

  /// Returns the number of errors Nix reported.
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
      .get(&(host.name().to_owned(), derivation.to_owned()))
      .map(Vec::as_slice)
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
      input_derivations:  Vec::new(),
      build_status:       BuildStatus::Unknown,
      derivation_parents: HashSet::new(),
      pname:              None,
      platform:           None,
    });
    self.derivation_ids.insert(drv, id);

    id
  }

  pub(crate) fn resolve_derivation(
    &mut self,
    original: Derivation,
    resolved: Derivation,
  ) -> DerivationId {
    if !self.derivation_ids.contains_key(&original)
      && let Some(&id) = self.derivation_ids.get(&resolved)
    {
      self.derivation_ids.insert(original, id);
      return id;
    }
    let original_id = self.get_or_create_derivation_id(original);
    if let Some(resolved_id) = self.derivation_ids.insert(resolved, original_id)
      && resolved_id != original_id
    {
      self.merge_derivations(original_id, resolved_id);
    }
    original_id
  }

  fn merge_derivations(
    &mut self,
    original: DerivationId,
    resolved: DerivationId,
  ) {
    let resolved_info = self
      .derivation_infos
      .shift_remove(&resolved)
      .expect("resolved derivation exists");
    let original_info = &mut self.derivation_infos[&original];
    if matches!(
      (&original_info.build_status, &resolved_info.build_status),
      (BuildStatus::Unknown | BuildStatus::Planned, _)
        | (
          BuildStatus::Available,
          BuildStatus::Building(_)
            | BuildStatus::Built { .. }
            | BuildStatus::Failed { .. }
        )
    ) && !matches!(resolved_info.build_status, BuildStatus::Unknown)
    {
      original_info.build_status = resolved_info.build_status;
    }
    for child in resolved_info.input_derivations {
      if !original_info.input_derivations.contains(&child) {
        original_info.input_derivations.push(child);
      }
    }
    original_info
      .derivation_parents
      .extend(resolved_info.derivation_parents);
    original_info.pname = original_info.pname.take().or(resolved_info.pname);
    original_info.platform =
      original_info.platform.take().or(resolved_info.platform);

    for id in self.derivation_ids.values_mut() {
      if *id == resolved {
        *id = original;
      }
    }
    for (&id, info) in &mut self.derivation_infos {
      let has_original = info.input_derivations.contains(&original);
      info.input_derivations.retain_mut(|child| {
        if *child == resolved {
          *child = original;
          !has_original && *child != id
        } else {
          *child != id
        }
      });
      if info.derivation_parents.remove(&resolved) && original != id {
        info.derivation_parents.insert(original);
      }
      info.derivation_parents.remove(&id);
    }
    for info in self.store_path_infos.values_mut() {
      if info.producer == Some(resolved) {
        info.producer = Some(original);
      }
      if info.input_for.remove(&resolved) {
        info.input_for.insert(original);
      }
    }
    for activity in self.activities.values_mut() {
      if activity.derivation == Some(resolved) {
        activity.derivation = Some(original);
      }
    }
    for transfer in self
      .full_summary
      .running_downloads
      .values_mut()
      .chain(self.full_summary.running_uploads.values_mut())
    {
      if transfer.parent == Some(resolved) {
        transfer.parent = Some(original);
      }
    }
    for transfer in self
      .full_summary
      .completed_downloads
      .values_mut()
      .chain(self.full_summary.completed_uploads.values_mut())
    {
      if transfer.parent == Some(resolved) {
        transfer.parent = Some(original);
      }
    }
    self
      .forest_roots
      .retain(|&id| id != resolved && id != original);
    self.ensure_root(original);
  }

  /// Applies derivation metadata returned by an injected resolver.
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

    // Register the derivation's output store paths.
    for (_, store_path_str) in &parsed.outputs {
      if let Some(sp) = StorePath::parse(store_path_str) {
        let sp_id = self.get_or_create_store_path_id(sp);
        if let Some(sp_info) = self.get_store_path_info_mut(sp_id) {
          sp_info.producer = Some(drv_id);
        }
      }
    }

    // Source paths have consumers even when their producing derivation is
    // unknown. The reverse index is the canonical relation presentation needs.
    for path in parsed.input_srcs {
      if let Some(path) = StorePath::parse(&path) {
        let path_id = self.get_or_create_store_path_id(path);
        if let Some(info) = self.get_store_path_info_mut(path_id) {
          info.input_for.insert(drv_id);
        }
      }
    }

    // Cached inputs remain `Unknown`; only explicit protocol announcements
    // promote them to `Planned` and make them visible.
    for (input_drv_path, _) in parsed.input_drvs {
      if let Some(input_drv) = Derivation::parse(&input_drv_path) {
        let input_drv_id = self.get_or_create_derivation_id(input_drv);
        self.link_dependency(drv_id, input_drv_id);
      }
    }
    self.ensure_root(drv_id);

    let started = self.get_derivation_info(drv_id).is_some_and(|info| {
      matches!(
        info.build_status,
        BuildStatus::Building(_)
          | BuildStatus::Built { .. }
          | BuildStatus::Failed { .. }
      )
    });
    if started {
      self.settle_inputs(drv_id);
    }
  }

  /// Marks the planned inputs of a starting build as available.
  ///
  /// Nix starts a build only once its inputs are valid. A planned input with
  /// no build of its own was realised by another process holding its lock.
  fn settle_inputs(&mut self, id: DerivationId) {
    let Some(info) = self.derivation_infos.get_mut(&id) else {
      return;
    };
    let inputs = std::mem::take(&mut info.input_derivations);
    for input in &inputs {
      if let Some(input_info) = self.derivation_infos.get_mut(input)
        && matches!(input_info.build_status, BuildStatus::Planned)
      {
        input_info.build_status = BuildStatus::Available;
      }
    }
    self.derivation_infos[&id].input_derivations = inputs;
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
  ) -> bool {
    let inserted = self.get_derivation_info_mut(parent).is_some_and(|info| {
      if info.input_derivations.contains(&child) {
        false
      } else {
        info.input_derivations.push(child);
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

  /// Returns the derivation with the given ID.
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

  pub(crate) fn mark_available(&mut self, id: DerivationId) -> bool {
    let Some(info) = self.derivation_infos.get_mut(&id) else {
      return false;
    };
    if !matches!(
      info.build_status,
      BuildStatus::Unknown | BuildStatus::Planned
    ) {
      return false;
    }
    info.build_status = BuildStatus::Available;
    true
  }

  pub(crate) fn start_build(
    &mut self,
    derivation: Derivation,
    build: BuildInfo,
    parent: Option<DerivationId>,
  ) -> DerivationId {
    let id = self.get_or_create_derivation_id(derivation);
    self.update_build_status(id, BuildStatus::Building(build));
    self.settle_inputs(id);
    if let Some(parent) = parent {
      self.link_dependency(parent, id);
    } else {
      self.ensure_root(id);
    }
    id
  }

  pub(crate) fn complete_build(&mut self, id: DerivationId, now: f64) -> bool {
    let Some(info) = self.derivation_infos.get_mut(&id) else {
      return false;
    };
    if !matches!(info.build_status, BuildStatus::Building(_)) {
      return false;
    }
    let BuildStatus::Building(build) =
      std::mem::replace(&mut info.build_status, BuildStatus::Unknown)
    else {
      unreachable!("build status was checked");
    };
    let completed_at = SystemTime::now();
    self
      .build_cache
      .entry((build.host.name().to_owned(), info.name.name.clone()))
      .or_default()
      .push(BuildReport {
        duration_secs: now - build.start,
        completed_at,
      });
    info.build_status = BuildStatus::Built {
      info: build,
      end: now,
      completed_at,
    };
    true
  }

  pub(crate) fn fail_dependency(&mut self, id: DerivationId) {
    let Some(info) = self.derivation_infos.get_mut(&id) else {
      return;
    };
    if matches!(
      info.build_status,
      BuildStatus::Unknown | BuildStatus::Planned
    ) {
      info.build_status = BuildStatus::DependencyFailed;
    }
  }

  pub(crate) fn fail_build(&mut self, id: DerivationId, fail: BuildFail) {
    let Some(info) = self.derivation_infos.get_mut(&id) else {
      return;
    };
    if !matches!(
      info.build_status,
      BuildStatus::Building(_) | BuildStatus::Built { .. }
    ) {
      return;
    }
    let previous =
      std::mem::replace(&mut info.build_status, BuildStatus::Unknown);
    let build = match previous {
      BuildStatus::Building(build) => build,
      BuildStatus::Built {
        info: build,
        completed_at,
        ..
      } => {
        if let Some(reports) = self
          .build_cache
          .get_mut(&(build.host.name().to_owned(), info.name.name.clone()))
        {
          reports.retain(|report| report.completed_at != completed_at);
        }
        build
      },
      BuildStatus::Unknown
      | BuildStatus::Planned
      | BuildStatus::Available
      | BuildStatus::Failed { .. }
      | BuildStatus::DependencyFailed => {
        unreachable!("build status was checked");
      },
    };
    info.build_status = BuildStatus::Failed { info: build, fail };
  }

  pub(crate) fn start_download(
    &mut self,
    path: StorePath,
    transfer: TransferInfo,
  ) -> StorePathId {
    let id = self.get_or_create_store_path_id(path);
    self.full_summary.planned_downloads.remove(&id);
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

  pub(crate) fn start_source_fetch(
    &mut self,
    id: ActivityId,
    fetch: SourceFetch,
  ) {
    self.source_fetches.insert(id, fetch);
  }

  pub(crate) fn finish_source_fetch(&mut self, id: ActivityId) -> bool {
    self.source_fetches.remove(&id).is_some()
  }

  /// Running source fetches in start order.
  pub fn source_fetches(&self) -> impl Iterator<Item = &SourceFetch> {
    self.source_fetches.values()
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
    let Some(derivation) = self.activity_derivation(id) else {
      return false;
    };
    let Some(info) = self.get_derivation_info_mut(derivation) else {
      return false;
    };
    let build = match &mut info.build_status {
      BuildStatus::Building(build)
      | BuildStatus::Built { info: build, .. }
      | BuildStatus::Failed { info: build, .. } => build,
      BuildStatus::Unknown
      | BuildStatus::Planned
      | BuildStatus::Available
      | BuildStatus::DependencyFailed => {
        return false;
      },
    };
    if build.phase.as_deref() == Some(&phase) {
      return false;
    }
    build.phase = Some(phase);
    true
  }

  pub(crate) fn update_activity_progress(
    &mut self,
    id: ActivityId,
    done: u64,
    expected: u64,
  ) -> bool {
    if !self.activities.contains_key(&id) {
      return false;
    }

    for transfer in self
      .full_summary
      .running_downloads
      .values_mut()
      .chain(self.full_summary.running_uploads.values_mut())
      .filter(|transfer| transfer.activity_id == id)
    {
      transfer.bytes_transferred = done;
      transfer.total_bytes = (expected > 0).then_some(expected);
    }
    true
  }

  pub(crate) fn update_build_status(
    &mut self,
    id: DerivationId,
    new_status: BuildStatus,
  ) {
    if let Some(info) = self.derivation_infos.get_mut(&id) {
      info.build_status = new_status;
    }
  }

  pub(crate) fn record_error(&mut self, error: String) {
    self.nix_error_count += 1;
    if self.nix_errors.len() < 128 {
      self.nix_errors.push(error);
    }
  }

  /// Returns whether Nix reported an error or a build failed.
  #[must_use]
  pub fn has_errors(&self) -> bool {
    self.nix_error_count > 0
      || self
        .derivation_infos
        .values()
        .any(|info| matches!(info.build_status, BuildStatus::Failed { .. }))
  }

  /// Returns whether any build or transfer is still running.
  #[must_use]
  pub fn has_unfinished(&self) -> bool {
    self
      .derivation_infos
      .values()
      .any(|info| matches!(info.build_status, BuildStatus::Building(_)))
      || !self.full_summary.running_downloads.is_empty()
      || !self.full_summary.running_uploads.is_empty()
  }

  /// Returns the number of derivations with a known status.
  #[must_use]
  pub fn total_builds(&self) -> usize {
    self
      .derivation_infos
      .values()
      .filter(|info| !matches!(info.build_status, BuildStatus::Unknown))
      .count()
  }

  /// Returns the log prefix for an activity, like `hello> `.
  ///
  /// Walks up the parent chain to the enclosing build activity and uses its
  /// derivation name. `prefix_style` selects the short name (pname only), the
  /// full name, or no prefix.
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
    parent:      transfer.parent,
    total_bytes: transfer.bytes_transferred,
  }
}

/// Returns the current time, in seconds since the UNIX epoch.
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
