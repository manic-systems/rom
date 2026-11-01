//! CPU use of local builds, read from the cgroups Nix gives them.

use std::{
  fs,
  num::NonZeroUsize,
  path::{Path, PathBuf},
  thread,
};

use foldhash::{HashMap, HashMapExt};

use crate::state::{BuildCpu, BuildLoad, DerivationId};

/// Seconds a local build may run without a matching cgroup before it is no
/// longer looked for.
const GIVE_UP: f64 = 10.0;

/// Shortest span a core count averages over, since a near-zero span inflates
/// it.
const MIN_WINDOW: f64 = 0.5;

const SERIAL_CORES: f32 = 1.5;
const SERIAL_AFTER: f64 = 60.0;
const IDLE_CORES: f32 = 0.1;
const IDLE_AFTER: f64 = 30.0;
const HEAVY_SHARE: f32 = 0.5;
const HEAVY_AFTER: f64 = 60.0;

/// CPU use from one sample.
pub(crate) struct Usage {
  /// Each build's use, or [`None`] for a build whose cgroup could not be read.
  pub builds:  Vec<(DerivationId, Option<BuildCpu>)>,
  /// Share of the machine's cores every build is using together.
  pub machine: Option<f32>,
}

/// Samples the CPU time of local builds from their cgroups.
///
/// Nix names a sandboxed build's cgroup `nix-build@<hash>-<owner>`.
#[derive(Debug)]
pub struct CgroupUsage {
  roots:   Vec<PathBuf>,
  builds:  HashMap<DerivationId, Sample>,
  matched: bool,
  cores:   f32,
}

#[derive(Debug)]
struct Sample {
  seen:        f64,
  cgroup:      Cgroup,
  last:        Option<(u64, f64)>,
  low_since:   Option<f64>,
  idle_since:  Option<f64>,
  heavy_since: Option<f64>,
}

#[derive(Debug)]
enum Cgroup {
  Searching,
  Found(PathBuf),
  Missing,
}

impl CgroupUsage {
  /// Finds the cgroups Nix creates build cgroups under.
  ///
  /// Returns [`None`] off Linux or when none exists.
  #[must_use]
  pub fn detect() -> Option<Self> {
    if !cfg!(target_os = "linux") {
      return None;
    }

    let base = Path::new("/sys/fs/cgroup");
    let own = fs::read_to_string("/proc/self/cgroup")
      .ok()
      .and_then(|text| {
        let line = text.lines().find_map(|line| line.strip_prefix("0::"))?;
        Some(base.join(line.trim_start_matches('/')))
      });
    let daemon = base.join("system.slice/nix-daemon.service");
    // Newer daemons run in a `nix-daemon` child cgroup and create builds under
    // it rather than beside it.
    let roots = [Some(daemon.join("nix-daemon")), Some(daemon), own]
      .into_iter()
      .flatten()
      .filter(|root| root.is_dir())
      .collect::<Vec<PathBuf>>();

    (!roots.is_empty()).then(|| {
      #[expect(
        clippy::cast_precision_loss,
        reason = "core counts are far below f32's integer range"
      )]
      let cores =
        thread::available_parallelism().map_or(1, NonZeroUsize::get) as f32;
      Self {
        roots,
        builds: HashMap::new(),
        matched: false,
        cores,
      }
    })
  }

  /// Returns each build's CPU use since the previous sample, and all of
  /// theirs against the machine.
  pub(crate) fn sample(
    &mut self,
    running: &[(DerivationId, &Path, f64)],
    now: f64,
  ) -> Usage {
    if self.roots.is_empty() {
      return Usage {
        builds:  Vec::new(),
        machine: None,
      };
    }

    self
      .builds
      .retain(|id, _| running.iter().any(|&(build, ..)| build == *id));

    let mut listing = None;
    let mut searching = false;
    let mut builds = Vec::new();
    let mut total = None;
    for &(id, drv, start) in running {
      let sample = self.builds.entry(id).or_insert(Sample {
        seen:        now,
        cgroup:      Cgroup::Searching,
        last:        None,
        low_since:   None,
        idle_since:  None,
        heavy_since: None,
      });

      if matches!(sample.cgroup, Cgroup::Searching) {
        let entries = listing.get_or_insert_with(|| list(&self.roots));
        match find(entries, drv) {
          Some(cgroup) => {
            sample.cgroup = Cgroup::Found(cgroup);
            self.matched = true;
          },
          None if now - sample.seen < GIVE_UP => searching = true,
          None => {
            tracing::debug!("no build cgroup for {}", drv.display());
            sample.cgroup = Cgroup::Missing;
          },
        }
      }

      let Cgroup::Found(cgroup) = &sample.cgroup else {
        continue;
      };

      let Some(usec) = read_usage(cgroup) else {
        builds.push((id, None));
        continue;
      };

      let (before, at) = sample.last.unwrap_or((0, start));
      if now - at >= MIN_WINDOW {
        #[expect(
          clippy::cast_precision_loss,
          clippy::cast_possible_truncation,
          reason = "a core count needs one decimal"
        )]
        let cores =
          (usec.saturating_sub(before) as f64 / ((now - at) * 1e6)) as f32;
        sample.low_since =
          (cores < SERIAL_CORES).then(|| sample.low_since.unwrap_or(now));
        sample.idle_since =
          (cores < IDLE_CORES).then(|| sample.idle_since.unwrap_or(now));
        sample.heavy_since = (cores / self.cores > HEAVY_SHARE
          && running.len() > 1)
          .then(|| sample.heavy_since.unwrap_or(now));

        let lasted = |since: Option<f64>, after: f64| {
          since.is_some_and(|since| now - since >= after)
        };
        let load = if lasted(sample.idle_since, IDLE_AFTER) {
          BuildLoad::Idle
        } else if lasted(sample.low_since, SERIAL_AFTER) {
          BuildLoad::Serial
        } else if lasted(sample.heavy_since, HEAVY_AFTER) {
          BuildLoad::Heavy
        } else {
          BuildLoad::Parallel
        };

        builds.push((id, Some(BuildCpu { cores, load })));
        *total.get_or_insert(0.0) += cores;
      }

      sample.last = Some((usec, now));
    }

    if !self.matched && !searching && !running.is_empty() {
      self.roots.clear();
    }

    let machine = total.map(|total| total / self.cores);

    Usage { builds, machine }
  }
}

fn list(roots: &[PathBuf]) -> Vec<PathBuf> {
  roots
    .iter()
    .filter_map(|root| fs::read_dir(root).ok())
    .flatten()
    .filter_map(|entry| {
      let path = entry.ok()?.path();
      path
        .file_name()?
        .to_str()?
        .starts_with("nix-build@")
        .then_some(path)
    })
    .collect()
}

/// Picks the populated cgroup named after `drv`, since an interrupted build
/// of the same derivation can leave one behind for another owner.
fn find(listing: &[PathBuf], drv: &Path) -> Option<PathBuf> {
  let (hash, _) = drv.file_name()?.to_str()?.split_once('-')?;
  let prefix = format!("nix-build@{hash}-");
  listing
    .iter()
    .filter(|cgroup| {
      cgroup
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(&prefix))
    })
    .find(|cgroup| {
      fs::read_to_string(cgroup.join("cgroup.events"))
        .is_ok_and(|events| events.lines().any(|line| line == "populated 1"))
    })
    .cloned()
}

fn read_usage(cgroup: &Path) -> Option<u64> {
  fs::read_to_string(cgroup.join("cpu.stat"))
    .ok()?
    .lines()
    .find_map(|line| line.strip_prefix("usage_usec "))?
    .parse()
    .ok()
}
