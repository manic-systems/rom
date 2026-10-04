//! Render snapshot derived from state, shared by every display format.

use std::collections::{BTreeMap, HashMap, HashSet};

use cognos::Host;
use indexmap::IndexMap;

use crate::state::{
  BuildFail,
  BuildInfo,
  BuildStatus,
  CompletedTransferInfo,
  DependencySummary,
  DerivationId,
  DerivationInfo,
  Membership,
  ProgressState,
  SourceFetch,
  State,
  StorePathId,
  StorePathInfo,
  TransferInfo,
  elapsed,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
  Download,
  Upload,
}

#[derive(Debug, Clone)]
pub(super) struct Transfer {
  pub direction: Direction,
  pub name:      String,
  pub done:      u64,
  pub total:     Option<u64>,
  pub start:     f64,
  pub host:      String,
  pub completed: bool,
}

pub(super) struct PresentedTransfer {
  pub transfer:  Transfer,
  pub placement: TransferPlacement,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum TransferPlacement {
  Inline(DerivationId),
  Source {
    consumer:       DerivationId,
    consumer_count: usize,
  },
  Unmatched,
}

impl TransferPlacement {
  pub const fn derivation(self) -> Option<DerivationId> {
    match self {
      Self::Inline(id) | Self::Source { consumer: id, .. } => Some(id),
      Self::Unmatched => None,
    }
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct StatusCounts {
  pub running:   usize,
  pub completed: usize,
  pub waiting:   usize,
  pub failed:    usize,
}

impl StatusCounts {
  pub const fn total(self) -> usize {
    self.running + self.completed + self.waiting + self.failed
  }

  pub const fn is_empty(self) -> bool {
    self.total() == 0
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SummaryCounts {
  pub builds:    StatusCounts,
  pub downloads: StatusCounts,
  pub uploads:   StatusCounts,
}

/// Progress of the units of one package.
#[derive(Debug, Clone, Default)]
pub(super) struct UnitCounts<'a> {
  pub total:   usize,
  pub done:    usize,
  pub running: usize,
  /// Running units per remote machine.
  pub remote:  BTreeMap<&'a str, usize>,
  pub failed:  usize,
  /// When the earliest unit started.
  pub start:   Option<f64>,
}

impl UnitCounts<'_> {
  pub const fn unfinished(&self) -> bool {
    self.done + self.failed < self.total
  }
}

#[derive(Default)]
pub(super) struct Builds<'a> {
  pub planned:   Vec<DerivationId>,
  pub running:   Vec<(DerivationId, &'a BuildInfo)>,
  pub completed: Vec<(DerivationId, &'a BuildInfo)>,
  pub failed:    Vec<(DerivationId, &'a BuildInfo, &'a BuildFail)>,
}

/// Seconds a source copy must run before it is shown.
///
/// Evaluation copies many already-cached sources in milliseconds; showing each
/// would flash rows in and out of the graph.
const SOURCE_FETCH_DELAY: f64 = 1.0;

/// Immutable presentation input derived once for a complete frame.
pub(super) struct RenderSnapshot<'a> {
  /// Names shared by several derivations, whose history mixes unrelated
  /// builds and so gives no usable estimate.
  pub shared_names:     HashSet<&'a str>,
  pub derivations:      &'a IndexMap<DerivationId, DerivationInfo>,
  pub summary:          &'a DependencySummary,
  pub builds:           Builds<'a>,
  pub roots:            &'a [DerivationId],
  pub start_time:       f64,
  pub error_count:      usize,
  pub counts:           SummaryCounts,
  pub placed_transfers: Vec<PresentedTransfer>,
  pub source_fetches:   Vec<&'a SourceFetch>,
  pub packages:         HashMap<DerivationId, UnitCounts<'a>>,
  pub finished:         bool,
}

impl<'a> RenderSnapshot<'a> {
  pub fn new(state: &'a State, now: f64) -> Self {
    let summary = state.summary();
    let mut builds = Builds::default();
    let mut available = 0;
    let mut dependency_failed = 0;
    let mut packages = HashMap::<DerivationId, UnitCounts>::new();
    for (&id, info) in state.derivations() {
      // A producer counts through the unit it wrote, once that is known.
      if let Membership::Unit(package) = info.membership
        && info.writes.is_none()
      {
        let counts = packages.entry(package).or_default();
        counts.total += 1;
        let started = match &info.build_status {
          BuildStatus::Unknown | BuildStatus::Planned => None,
          BuildStatus::Available => {
            counts.done += 1;
            None
          },
          BuildStatus::Building(build) => {
            counts.running += 1;
            if let Host::Remote(machine) = &build.host {
              *counts.remote.entry(machine.as_str()).or_default() += 1;
            }
            Some(build.start)
          },
          BuildStatus::Built { info: build, .. } => {
            counts.done += 1;
            Some(build.start)
          },
          BuildStatus::Failed { info: build, .. } => {
            counts.failed += 1;
            Some(build.start)
          },
          BuildStatus::DependencyFailed => {
            counts.failed += 1;
            None
          },
        };
        if let Some(started) = started {
          counts.start = Some(
            counts
              .start
              .map_or(started, |earliest| earliest.min(started)),
          );
        }
      }
      match &info.build_status {
        BuildStatus::Unknown => {},
        BuildStatus::Planned => builds.planned.push(id),
        BuildStatus::Available => available += 1,
        BuildStatus::Building(build) => builds.running.push((id, build)),
        BuildStatus::Built { info, .. } => builds.completed.push((id, info)),
        BuildStatus::Failed { info, fail } => {
          builds.failed.push((id, info, fail));
        },
        BuildStatus::DependencyFailed => dependency_failed += 1,
      }
    }
    let counts = SummaryCounts {
      builds:    StatusCounts {
        running:   builds.running.len(),
        completed: builds.completed.len() + available,
        waiting:   builds.planned.len(),
        failed:    builds.failed.len() + dependency_failed,
      },
      downloads: StatusCounts {
        running:   summary.running_downloads.len(),
        completed: summary.completed_downloads.len(),
        waiting:   summary.planned_downloads.len(),
        failed:    0,
      },
      uploads:   StatusCounts {
        running:   summary.running_uploads.len(),
        completed: summary.completed_uploads.len(),
        waiting:   0,
        failed:    0,
      },
    };
    let mut seen_names = HashSet::new();
    let shared_names = state
      .derivations()
      .values()
      .map(|info| info.name.name.as_str())
      .filter(|name| !seen_names.insert(*name))
      .collect();
    let mut snapshot = Self {
      shared_names,
      derivations: state.derivations(),
      summary,
      builds,
      roots: state.roots(),
      start_time: state.start_time(),
      error_count: state.error_count(),
      counts,
      placed_transfers: Vec::new(),
      source_fetches: state
        .source_fetches()
        .filter(|fetch| elapsed(fetch.start, now) >= SOURCE_FETCH_DELAY)
        .collect(),
      packages,
      finished: state.progress_state() == ProgressState::Finished,
    };
    snapshot.collect_transfers(state.store_paths(), now);
    snapshot
  }

  pub fn transfers(&self) -> impl Iterator<Item = &Transfer> {
    self.placed_transfers.iter().map(|item| &item.transfer)
  }

  pub fn derivation(&self, id: DerivationId) -> Option<&DerivationInfo> {
    self.derivations.get(&id)
  }

  fn collect_transfers(
    &mut self,
    store_paths: &IndexMap<StorePathId, StorePathInfo>,
    now: f64,
  ) {
    for (&path, transfer) in &self.summary.running_downloads {
      self.add_transfer(store_paths, path, transfer, Direction::Download);
    }
    for (&path, transfer) in &self.summary.running_uploads {
      self.add_transfer(store_paths, path, transfer, Direction::Upload);
    }
    for (&path, transfer) in &self.summary.completed_downloads {
      if elapsed(transfer.end, now) <= 1.0 {
        self.add_completed_transfer(
          store_paths,
          path,
          transfer,
          Direction::Download,
        );
      }
    }
    for (&path, transfer) in &self.summary.completed_uploads {
      if elapsed(transfer.end, now) <= 1.0 {
        self.add_completed_transfer(
          store_paths,
          path,
          transfer,
          Direction::Upload,
        );
      }
    }
    // Stable detail order, with active work ahead of completion-grace rows.
    self.placed_transfers.sort_by(|left, right| {
      let key = |item: &PresentedTransfer| {
        (
          item.transfer.completed,
          item.transfer.direction == Direction::Upload,
        )
      };
      key(left)
        .cmp(&key(right))
        .then_with(|| left.transfer.name.cmp(&right.transfer.name))
        .then_with(|| left.transfer.host.cmp(&right.transfer.host))
    });
  }

  fn add_transfer(
    &mut self,
    store_paths: &IndexMap<StorePathId, StorePathInfo>,
    path: StorePathId,
    transfer: &TransferInfo,
    direction: Direction,
  ) {
    let Some(info) = store_paths.get(&path) else {
      return;
    };
    self.push_transfer(info, transfer.parent, Transfer {
      direction,
      name: info.name.name.clone(),
      done: transfer.bytes_transferred,
      total: transfer.total_bytes,
      start: transfer.start,
      host: transfer.host.name().to_owned(),
      completed: false,
    });
  }

  fn add_completed_transfer(
    &mut self,
    store_paths: &IndexMap<StorePathId, StorePathInfo>,
    path: StorePathId,
    transfer: &CompletedTransferInfo,
    direction: Direction,
  ) {
    let Some(info) = store_paths.get(&path) else {
      return;
    };
    self.push_transfer(info, transfer.parent, Transfer {
      direction,
      name: info.name.name.clone(),
      done: transfer.total_bytes,
      total: Some(transfer.total_bytes),
      start: transfer.start,
      host: transfer.host.name().to_owned(),
      completed: true,
    });
  }

  fn push_transfer(
    &mut self,
    info: &StorePathInfo,
    parent: Option<DerivationId>,
    transfer: Transfer,
  ) {
    let placement = if let Some(producer) = info.producer {
      TransferPlacement::Inline(producer)
    } else if transfer.direction == Direction::Download
      && let Some(consumer) = info.input_for.iter().copied().min_by_key(|id| {
        // Prefer work in progress, then planned consumers. IDs break ties
        // deterministically rather than depending on HashSet iteration order.
        let priority = self.derivation(*id).map_or(4, |info| {
          match info.build_status {
            BuildStatus::Building(_) => 0,
            BuildStatus::Planned => 1,
            BuildStatus::Failed { .. } => 2,
            BuildStatus::Unknown => 3,
            BuildStatus::Built { .. }
            | BuildStatus::Available
            | BuildStatus::DependencyFailed => 4,
          }
        });
        (priority, *id)
      })
    {
      TransferPlacement::Source {
        consumer,
        consumer_count: info.input_for.len(),
      }
    } else if let Some(parent) = parent {
      TransferPlacement::Inline(parent)
    } else {
      TransferPlacement::Unmatched
    };
    self.placed_transfers.push(PresentedTransfer {
      transfer,
      placement,
    });
  }
}

pub(super) fn aggregate_transfers<'a>(
  transfers: impl IntoIterator<Item = &'a Transfer>,
) -> Transfer {
  let mut selected: Vec<_> = transfers.into_iter().collect();
  // A just-completed transfer remains visible briefly, but must not be folded
  // into an active transfer's progress. Mixing those scopes makes the total
  // jump whenever the completed-transfer grace period starts or expires.
  if selected.iter().any(|item| !item.completed) {
    selected.retain(|item| !item.completed);
  }
  let direction = if selected
    .iter()
    .all(|item| item.direction == Direction::Upload)
  {
    Direction::Upload
  } else {
    Direction::Download
  };
  let total = selected
    .iter()
    .try_fold(0_u64, |sum, item| Some(sum.saturating_add(item.total?)));
  Transfer {
    direction,
    name: selected
      .first()
      .map(|item| item.name.clone())
      .unwrap_or_default(),
    done: selected.iter().map(|item| item.done).sum(),
    total,
    start: selected
      .iter()
      .map(|item| item.start)
      .fold(f64::INFINITY, f64::min),
    host: selected
      .first()
      .map(|item| item.host.clone())
      .unwrap_or_default(),
    completed: selected.iter().all(|item| item.completed),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn transfer(done: u64, total: u64, completed: bool) -> Transfer {
    Transfer {
      direction: Direction::Download,
      name: "demo".to_owned(),
      done,
      total: Some(total),
      start: 0.0,
      host: "cache".to_owned(),
      completed,
    }
  }

  #[test]
  fn active_transfer_progress_excludes_recently_completed_transfers() {
    let aggregate = aggregate_transfers(&[
      transfer(312, 312, true),
      transfer(188, 1024, false),
    ]);
    assert_eq!(aggregate.done, 188);
    assert_eq!(aggregate.total, Some(1024));
    assert!(!aggregate.completed);
  }
}
