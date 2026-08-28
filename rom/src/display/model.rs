use std::collections::HashMap;

use indexmap::IndexMap;

use crate::state::{
  CompletedTransferInfo,
  DependencySummary,
  DerivationId,
  DerivationInfo,
  State,
  StorePathId,
  StorePathInfo,
  TransferInfo,
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

/// Immutable presentation input derived once for a complete frame.
pub(super) struct RenderSnapshot<'a> {
  pub derivations:         &'a IndexMap<DerivationId, DerivationInfo>,
  pub summary:             &'a DependencySummary,
  pub roots:               &'a [DerivationId],
  pub start_time:          f64,
  pub error_count:         usize,
  pub counts:              SummaryCounts,
  pub transfers_by_drv:    HashMap<DerivationId, Vec<Transfer>>,
  pub unmatched_transfers: Vec<Transfer>,
}

impl<'a> RenderSnapshot<'a> {
  pub fn new(state: &'a State, now: f64) -> Self {
    let summary = state.summary();
    let counts = SummaryCounts {
      builds:    StatusCounts {
        running:   summary.running_builds.len(),
        completed: summary.completed_builds.len(),
        waiting:   summary.planned_builds.len(),
        failed:    summary.failed_builds.len(),
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
    let mut snapshot = Self {
      derivations: state.derivations(),
      summary,
      roots: state.roots(),
      start_time: state.start_time(),
      error_count: state.error_count(),
      counts,
      transfers_by_drv: HashMap::new(),
      unmatched_transfers: Vec::new(),
    };
    snapshot.collect_transfers(state.store_paths(), now);
    snapshot
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
      if now - transfer.end <= 1.0 {
        self.add_completed_transfer(
          store_paths,
          path,
          transfer,
          Direction::Download,
        );
      }
    }
    for (&path, transfer) in &self.summary.completed_uploads {
      if now - transfer.end <= 1.0 {
        self.add_completed_transfer(
          store_paths,
          path,
          transfer,
          Direction::Upload,
        );
      }
    }
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
    self.push_transfer(info, Transfer {
      direction,
      name: info.name.name.clone(),
      done: transfer.bytes_transferred,
      total: transfer.total_bytes,
      start: transfer.start,
      host: transfer.host.name().to_string(),
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
    self.push_transfer(info, Transfer {
      direction,
      name: info.name.name.clone(),
      done: transfer.total_bytes,
      total: Some(transfer.total_bytes),
      start: transfer.start,
      host: transfer.host.name().to_string(),
      completed: true,
    });
  }

  fn push_transfer(&mut self, info: &StorePathInfo, transfer: Transfer) {
    if let Some(producer) = info.producer {
      self
        .transfers_by_drv
        .entry(producer)
        .or_default()
        .push(transfer);
    } else {
      self.unmatched_transfers.push(transfer);
    }
  }
}

pub(super) fn aggregate_transfers(transfers: &[Transfer]) -> Transfer {
  // A just-completed transfer remains visible briefly, but must not be folded
  // into an active transfer's progress. Mixing those scopes makes the total
  // jump whenever the completed-transfer grace period starts or expires.
  let has_active = transfers.iter().any(|item| !item.completed);
  let selected: Vec<_> = transfers
    .iter()
    .filter(|item| !has_active || !item.completed)
    .collect();
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
      name: "demo".to_string(),
      done,
      total: Some(total),
      start: 0.0,
      host: "cache".to_string(),
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
