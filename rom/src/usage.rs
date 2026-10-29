//! CPU use of local builds, read from the cgroups Nix gives them.

use std::{
  fs,
  path::{Path, PathBuf},
};

use foldhash::{HashMap, HashMapExt};

use crate::state::DerivationId;

/// Seconds a local build may run without a matching cgroup before it is no
/// longer looked for.
const GIVE_UP: f64 = 10.0;

/// Shortest span a core count averages over, since a near-zero span inflates
/// it.
const MIN_WINDOW: f64 = 0.5;

/// Samples the CPU time of local builds from their cgroups.
///
/// Nix names a sandboxed build's cgroup `nix-build@<hash>-<owner>`.
#[derive(Debug)]
pub struct CgroupUsage {
  roots:   Vec<PathBuf>,
  builds:  HashMap<DerivationId, Sample>,
  matched: bool,
}

#[derive(Debug)]
struct Sample {
  seen:   f64,
  cgroup: Cgroup,
  last:   Option<(u64, f64)>,
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
  /// Returns [`None`] off Linux or when neither exists.
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
    let roots = [Some(base.join("system.slice/nix-daemon.service")), own]
      .into_iter()
      .flatten()
      .filter(|root| root.is_dir())
      .collect::<Vec<PathBuf>>();

    (!roots.is_empty()).then(|| {
      Self {
        roots,
        builds: HashMap::new(),
        matched: false,
      }
    })
  }

  /// Returns the cores each build used since the previous sample, or [`None`]
  /// for a build whose cgroup could not be read.
  pub(crate) fn sample(
    &mut self,
    running: &[(DerivationId, &Path, f64)],
    now: f64,
  ) -> Vec<(DerivationId, Option<f32>)> {
    if self.roots.is_empty() {
      return Vec::new();
    }

    self
      .builds
      .retain(|id, _| running.iter().any(|&(build, ..)| build == *id));

    let mut listing = None;
    let mut searching = false;
    let mut cores = Vec::new();
    for &(id, drv, start) in running {
      let sample = self.builds.entry(id).or_insert(Sample {
        seen:   now,
        cgroup: Cgroup::Searching,
        last:   None,
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
        cores.push((id, None));
        continue;
      };

      let (before, at) = sample.last.unwrap_or((0, start));
      if now - at >= MIN_WINDOW {
        #[expect(
          clippy::cast_precision_loss,
          clippy::cast_possible_truncation,
          reason = "a core count needs one decimal"
        )]
        let busy =
          (usec.saturating_sub(before) as f64 / ((now - at) * 1e6)) as f32;
        cores.push((id, Some(busy)));
      }

      sample.last = Some((usec, now));
    }

    if !self.matched && !searching && !running.is_empty() {
      self.roots.clear();
    }

    cores
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
