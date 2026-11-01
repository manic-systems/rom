//! Render snapshot derived from state, shared by every display format.

use std::collections::BTreeMap;

use cognos::Host;
use foldhash::{HashMap, HashMapExt, HashSet, HashSetExt, fast::RandomState};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Direction {
  Download,
  Upload,
}

#[derive(Debug, Clone)]
pub(super) struct Transfer {
  pub direction:     Direction,
  pub name:          String,
  pub done:          u64,
  pub total:         Option<u64>,
  pub unsized_count: usize,
  pub start:         f64,
  pub host:          String,
  pub completed:     bool,
  /// Seconds since the byte count last moved.
  pub idle:          f64,
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
  pub checking:  usize,
  pub failed:    usize,
}

impl StatusCounts {
  pub const fn total(self) -> usize {
    self.running + self.completed + self.waiting + self.checking + self.failed
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
pub(super) const SOURCE_FETCH_DELAY: f64 = 1.0;
/// Unmatched transfers must last this long before entering the tree.
pub(super) const TRANSFER_DISPLAY_DELAY: f64 = 1.0;

/// Immutable presentation input derived once for a complete frame.
pub(super) struct RenderSnapshot<'a> {
  pub state:            &'a State,
  pub derivations:      &'a IndexMap<DerivationId, DerivationInfo, RandomState>,
  pub summary:          &'a DependencySummary,
  pub builds:           Builds<'a>,
  pub roots:            &'a [DerivationId],
  pub start_time:       f64,
  pub error_count:      usize,
  pub counts:           SummaryCounts,
  pub placed_transfers: Vec<PresentedTransfer>,
  pub build_transfers:  HashMap<DerivationId, Vec<Transfer>>,
  pub source_fetches:   Vec<&'a SourceFetch>,
  pub packages:         HashMap<DerivationId, UnitCounts<'a>>,
  pub finished:         bool,
  /// Planned CA derivations only the dry run says will build.
  pub unconfirmed:      HashSet<DerivationId>,
}

/// A floating derivation surely builds once it is unbuilt or an input builds.
fn unconfirmed(
  derivations: &IndexMap<DerivationId, DerivationInfo, RandomState>,
) -> HashSet<DerivationId> {
  let mut builds = HashMap::<DerivationId, bool>::new();
  let mut unconfirmed = HashSet::new();
  for (&root, info) in derivations {
    if !matches!(info.build_status, BuildStatus::Planned) {
      continue;
    }

    let mut stack = vec![(root, false)];
    while let Some((id, expanded)) = stack.pop() {
      if builds.contains_key(&id) {
        continue;
      }
      let Some(info) = derivations.get(&id) else {
        builds.insert(id, false);
        continue;
      };
      let settled = match info.build_status {
        BuildStatus::Building(_)
        | BuildStatus::Built { .. }
        | BuildStatus::Failed { .. } => Some(true),
        BuildStatus::Planned if !info.floating || info.unbuilt => Some(true),
        BuildStatus::Planned => None,
        BuildStatus::Unknown
        | BuildStatus::Available
        | BuildStatus::DependencyFailed => Some(false),
      };
      if let Some(settled) = settled {
        builds.insert(id, settled);
        continue;
      }

      if expanded {
        let builds_for_sure = info
          .input_derivations
          .iter()
          .any(|input| builds.get(input).copied().unwrap_or(false));
        builds.insert(id, builds_for_sure);
        continue;
      }
      stack.push((id, true));
      stack.extend(
        info
          .input_derivations
          .iter()
          .filter(|input| !builds.contains_key(input))
          .map(|&input| (input, false)),
      );
    }

    if builds.get(&root) == Some(&false) {
      unconfirmed.insert(root);
    }
  }
  unconfirmed
}

impl<'a> RenderSnapshot<'a> {
  pub fn new(state: &'a State, now: f64) -> Self {
    let summary = state.summary();
    let mut builds = Builds::default();
    let mut available = 0;
    let mut dependency_failed = 0;
    let mut packages = HashMap::<DerivationId, UnitCounts>::new();
    let unconfirmed = unconfirmed(state.derivations());
    for (&id, info) in state.derivations() {
      // A producer counts through the unit it wrote, once that is known.
      if let Membership::Unit(package) = info.membership
        && info.writes.is_none()
      {
        let counts = packages.entry(package).or_default();
        counts.total += 1;
        let producer = info.writes.is_some();
        let started = match &info.build_status {
          BuildStatus::Unknown | BuildStatus::Planned => None,
          BuildStatus::Available => {
            counts.done += usize::from(!producer);
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
            counts.done += usize::from(!producer);
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
        BuildStatus::Planned if unconfirmed.contains(&id) => {},
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
        waiting:   builds.planned.len().max(
          usize::try_from(state.builds_progress().waiting())
            .unwrap_or(usize::MAX),
        ),
        checking:  unconfirmed.len(),
        failed:    builds.failed.len() + dependency_failed,
      },
      downloads: StatusCounts {
        running:   summary.running_downloads.len(),
        completed: summary.completed_downloads.len(),
        waiting:   summary.planned_downloads.len(),
        checking:  0,
        failed:    0,
      },
      uploads:   StatusCounts {
        running:   summary.running_uploads.len(),
        completed: summary.completed_uploads.len(),
        waiting:   0,
        checking:  0,
        failed:    0,
      },
    };
    let mut snapshot = Self {
      state,
      derivations: state.derivations(),
      summary,
      builds,
      roots: state.roots(),
      start_time: state.start_time(),
      error_count: state.error_count(),
      counts,
      placed_transfers: Vec::new(),
      build_transfers: HashMap::default(),
      source_fetches: state
        .source_fetches()
        .filter(|fetch| elapsed(fetch.start, now) >= SOURCE_FETCH_DELAY)
        .collect(),
      packages,
      finished: state.progress_state() == ProgressState::Finished,
      unconfirmed,
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
    store_paths: &IndexMap<StorePathId, StorePathInfo, RandomState>,
    now: f64,
  ) {
    for (&path, transfer) in &self.summary.running_downloads {
      self.add_transfer(store_paths, path, transfer, Direction::Download, now);
    }
    for (&path, transfer) in &self.summary.running_uploads {
      self.add_transfer(store_paths, path, transfer, Direction::Upload, now);
    }
    for (&path, transfer) in &self.summary.completed_downloads {
      self.add_completed_transfer(
        store_paths,
        path,
        transfer,
        Direction::Download,
        elapsed(transfer.end, now) <= 1.0,
      );
    }
    for (&path, transfer) in &self.summary.completed_uploads {
      self.add_completed_transfer(
        store_paths,
        path,
        transfer,
        Direction::Upload,
        elapsed(transfer.end, now) <= 1.0,
      );
    }
    // Stable detail order, regardless of completion-grace state.
    self.placed_transfers.sort_by(|left, right| {
      let key =
        |item: &PresentedTransfer| item.transfer.direction == Direction::Upload;
      key(left)
        .cmp(&key(right))
        .then_with(|| left.transfer.name.cmp(&right.transfer.name))
        .then_with(|| left.transfer.host.cmp(&right.transfer.host))
    });
  }

  fn add_transfer(
    &mut self,
    store_paths: &IndexMap<StorePathId, StorePathInfo, RandomState>,
    path: StorePathId,
    transfer: &TransferInfo,
    direction: Direction,
    now: f64,
  ) {
    let Some(info) = store_paths.get(&path) else {
      return;
    };
    self.push_transfer(
      info,
      transfer.parent,
      Transfer {
        direction,
        name: info.name.name.clone(),
        done: transfer.bytes_transferred,
        total: transfer.total_bytes,
        unsized_count: 0,
        start: transfer.start,
        host: transfer.host.name().to_owned(),
        completed: false,
        idle: elapsed(transfer.last_progress, now),
      },
      true,
    );
  }

  fn add_completed_transfer(
    &mut self,
    store_paths: &IndexMap<StorePathId, StorePathInfo, RandomState>,
    path: StorePathId,
    transfer: &CompletedTransferInfo,
    direction: Direction,
    visible: bool,
  ) {
    let Some(info) = store_paths.get(&path) else {
      return;
    };
    self.push_transfer(
      info,
      transfer.parent,
      Transfer {
        direction,
        name: info.name.name.clone(),
        done: transfer.total_bytes,
        total: Some(transfer.total_bytes),
        unsized_count: 0,
        start: transfer.start,
        host: transfer.host.name().to_owned(),
        completed: true,
        idle: 0.0,
      },
      visible,
    );
  }

  fn push_transfer(
    &mut self,
    info: &StorePathInfo,
    parent: Option<DerivationId>,
    transfer: Transfer,
    visible: bool,
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
    } else if transfer.direction == Direction::Upload
      && let Some(build) = self.remote_build(&transfer.host)
    {
      // Nix does not say which build an upload is for, but it copies inputs
      // to a builder right before building there.
      TransferPlacement::Source {
        consumer:       build,
        consumer_count: 1,
      }
    } else {
      TransferPlacement::Unmatched
    };
    if let TransferPlacement::Inline(id) = placement {
      self
        .build_transfers
        .entry(id)
        .or_default()
        .push(transfer.clone());
    }
    if visible {
      self.placed_transfers.push(PresentedTransfer {
        transfer,
        placement,
      });
    }
  }
}

impl RenderSnapshot<'_> {
  /// The only running build on `host`, which an unparented upload may be for.
  fn remote_build(&self, host: &str) -> Option<DerivationId> {
    let mut builds = self
      .builds
      .running
      .iter()
      .filter(|(_, build)| {
        matches!(&build.host, Host::Remote(machine) if machine == host)
      });
    let &(id, _) = builds.next()?;
    builds.next().is_none().then_some(id)
  }
}

pub(super) fn aggregate_transfers<'a>(
  transfers: impl IntoIterator<Item = &'a Transfer>,
) -> Vec<Transfer> {
  let mut grouped = BTreeMap::<Direction, Vec<&Transfer>>::new();
  for transfer in transfers {
    grouped
      .entry(transfer.direction)
      .or_default()
      .push(transfer);
  }

  grouped
    .into_values()
    .map(|selected| {
      let has_sized = selected.iter().any(|item| item.total.is_some());
      let done = selected
        .iter()
        .filter(|item| !has_sized || item.total.is_some())
        .fold(0_u64, |done, item| done.saturating_add(item.done));
      let total = has_sized.then(|| {
        selected
          .iter()
          .filter_map(|item| item.total)
          .fold(0_u64, u64::saturating_add)
      });
      Transfer {
        direction: selected[0].direction,
        name: selected[0].name.clone(),
        done,
        total,
        unsized_count: selected
          .iter()
          .filter(|item| item.total.is_none())
          .count(),
        start: selected
          .iter()
          .map(|item| item.start)
          .fold(f64::INFINITY, f64::min),
        host: selected[0].host.clone(),
        completed: selected.iter().all(|item| item.completed),
        idle: selected
          .iter()
          .map(|item| item.idle)
          .fold(f64::INFINITY, f64::min),
      }
    })
    .collect()
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
      unsized_count: 0,
      start: 0.0,
      host: "cache".to_owned(),
      completed,
      idle: 0.0,
    }
  }

  #[test]
  fn transfer_progress_includes_recently_completed_transfers() {
    let [aggregate] = aggregate_transfers(&[
      transfer(312, 312, true),
      transfer(188, 1024, false),
    ])
    .try_into()
    .unwrap();
    assert_eq!(aggregate.done, 500);
    assert_eq!(aggregate.total, Some(1336));
    assert!(!aggregate.completed);
  }
}
