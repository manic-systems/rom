//! Derivation metadata for replayed fixture logs.
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
