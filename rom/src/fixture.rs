//! Derivation metadata and CPU use for replayed fixture logs.
//!
//! Fixtures name store paths that do not exist, so the `.drv` files ROM would
//! read for dependency edges come from a JSON sidecar instead:
//!
//! ```json
//! {
//!   "derivations": {
//!     "/nix/store/…-app.drv": {
//!       "inputs": ["/nix/store/…-lib.drv"],
//!       "sources": ["/nix/store/…-source"],
//!       "outputs": { "out": "" },
//!       "dynamic": ["/nix/store/…-plan.drv.drv"],
//!       "pname": "app"
//!     }
//!   },
//!   "produced": { "/nix/store/…-plan.drv.drv": "/nix/store/…-plan.drv" },
//!   "realised": ["/nix/store/…-cached.drv"]
//! }
//! ```
//!
//! Every field is optional. A derivation without `outputs` gets an `out` named
//! after itself, and an empty output path makes it content-addressed.

use std::{
  collections::BTreeMap,
  fs,
  io,
  path::{Path, PathBuf},
  sync::Mutex,
};

use serde::Deserialize;

use crate::monitor::{
  Answer,
  DerivationResolver,
  LookupPriority,
  ProducedDrv,
  RealisedDrv,
};

/// Names the sidecar the CLI loads in place of the store's `.drv` files.
pub const FIXTURE_DERIVATIONS_ENV: &str = "ROM_FIXTURE_DERIVATIONS";

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Sidecar {
  derivations: BTreeMap<String, Entry>,
  produced:    BTreeMap<String, String>,
  realised:    Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Entry {
  inputs:  Vec<String>,
  sources: Vec<String>,
  outputs: Option<BTreeMap<String, String>>,
  dynamic: Vec<String>,
  pname:   Option<String>,
}

/// Resolves derivations from a fixture's JSON sidecar.
///
/// Lookups answer immediately, as if Nix's build trace were always at hand.
#[derive(Debug)]
pub struct FixtureResolver {
  sidecar: Sidecar,
  answers: Mutex<Vec<Answer>>,
}

impl FixtureResolver {
  /// Reads the sidecar at `path`.
  ///
  /// # Errors
  ///
  /// Returns an error if the file cannot be read or is not a valid sidecar.
  pub fn load(path: &Path) -> io::Result<Self> {
    let sidecar = serde_json::from_slice(&fs::read(path)?)
      .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(Self {
      sidecar,
      answers: Mutex::new(Vec::new()),
    })
  }

  fn answer(&self, answers: impl IntoIterator<Item = Answer>) {
    if let Ok(mut queue) = self.answers.lock() {
      queue.extend(answers);
    }
  }
}

impl DerivationResolver for FixtureResolver {
  fn resolve(
    &self,
    path: &Path,
  ) -> Result<cognos::ParsedDerivation, cognos::aterm::Error> {
    let key = path.to_string_lossy();
    let Some(entry) = self.sidecar.derivations.get(key.as_ref()) else {
      return Err(cognos::aterm::Error::Read {
        path:   path.to_path_buf(),
        source: io::ErrorKind::NotFound.into(),
      });
    };
    let outputs = entry.outputs.as_ref().map_or_else(
      || {
        let out = key.strip_suffix(".drv").unwrap_or(&key).to_owned();
        vec![("out".to_owned(), out)]
      },
      |outputs| {
        outputs
          .iter()
          .map(|(name, path)| (name.clone(), path.clone()))
          .collect()
      },
    );
    Ok(cognos::ParsedDerivation {
      outputs,
      input_drvs: entry
        .inputs
        .iter()
        .map(|input| (input.clone(), vec!["out".to_owned()]))
        .collect(),
      dynamic_drvs: entry.dynamic.clone(),
      input_srcs: entry.sources.clone(),
      platform: "x86_64-linux".to_owned(),
      builder: "/bin/sh".to_owned(),
      args: Vec::new(),
      env: entry
        .pname
        .iter()
        .map(|pname| ("pname".to_owned(), pname.clone().into_bytes()))
        .collect(),
    })
  }

  fn request_produced(
    &self,
    producers: Vec<PathBuf>,
    _priority: LookupPriority,
  ) {
    self.answer(producers.into_iter().map(|producer| {
      let drv = self
        .sidecar
        .produced
        .get(producer.to_string_lossy().as_ref())
        .map(PathBuf::from);
      Answer::Produced(ProducedDrv { producer, drv })
    }));
  }

  fn request_writer(&self, drv: PathBuf, producers: Vec<PathBuf>) {
    let writer = producers.into_iter().find(|producer| {
      self
        .sidecar
        .produced
        .get(producer.to_string_lossy().as_ref())
        .is_some_and(|written| Path::new(written) == drv)
    });
    self.answer(writer.map(|producer| {
      Answer::Produced(ProducedDrv {
        producer,
        drv: Some(drv),
      })
    }));
  }

  fn request_realised(&self, drvs: Vec<PathBuf>) {
    self.answer(drvs.into_iter().map(|drv| {
      let realised = self
        .sidecar
        .realised
        .iter()
        .any(|realised| Path::new(realised) == drv);
      Answer::Realised(RealisedDrv { drv, realised })
    }));
  }

  fn answers(&self) -> Vec<Answer> {
    self
      .answers
      .lock()
      .map(|mut queue| std::mem::take(&mut *queue))
      .unwrap_or_default()
  }
}

/// Names the CPU sidecar the CLI samples in place of build cgroups.
pub const FIXTURE_CPU_ENV: &str = "ROM_FIXTURE_CPU";

/// Cores a fixture's machine has, so heavy builds read the same everywhere.
pub const FIXTURE_CORES: f32 = 8.0;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CpuRow {
  after_ms: u64,
  drv:      String,
  cores:    f32,
}

/// CPU use of a fixture's builds, from a CSV sidecar:
///
/// ```csv
/// after_ms,drv,cores
/// 0,/nix/store/…-app.drv,6.0
/// 3000,/nix/store/…-app.drv,1.0
/// ```
///
/// Each row sets how many cores a build uses from `after_ms` into it until its
/// next row. Times count from the build's start rather than the log's, which
/// holds whichever clock a replay runs on.
#[derive(Debug, Default)]
pub struct FixtureCpu {
  builds: BTreeMap<String, Vec<(f64, f32)>>,
}

impl FixtureCpu {
  /// Reads the sidecar at `path`.
  ///
  /// # Errors
  ///
  /// Returns an error if the file cannot be read or is not a valid sidecar.
  pub fn load(path: &Path) -> io::Result<Self> {
    let mut builds = BTreeMap::<String, Vec<(f64, f32)>>::new();
    for row in csv::Reader::from_path(path)?.deserialize::<CpuRow>() {
      let row = row
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
      #[expect(
        clippy::cast_precision_loss,
        reason = "fixture offsets are far below f64's integer range"
      )]
      let from = row.after_ms as f64 / 1000.0;
      builds.entry(row.drv).or_default().push((from, row.cores));
    }
    for steps in builds.values_mut() {
      steps.sort_by(|left, right| left.0.total_cmp(&right.0));
    }
    Ok(Self { builds })
  }

  /// Returns the CPU time `drv` used in its first `elapsed` seconds, in
  /// microseconds, like a cgroup's `usage_usec`.
  #[must_use]
  pub fn usage_usec(&self, drv: &Path, elapsed: f64) -> Option<u64> {
    let steps = self.builds.get(drv.to_str()?)?;
    let seconds = steps
      .iter()
      .enumerate()
      .map(|(index, &(from, cores))| {
        let until = steps
          .get(index + 1)
          .map_or(elapsed, |&(next, _)| next.min(elapsed));
        f64::from(cores) * (until - from).max(0.0)
      })
      .sum::<f64>();
    #[expect(
      clippy::cast_possible_truncation,
      clippy::cast_sign_loss,
      reason = "CPU time is never negative and far below u64::MAX"
    )]
    Some((seconds * 1e6) as u64)
  }
}
