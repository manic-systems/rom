use std::{
  collections::{HashMap, HashSet},
  fs::{self, File, OpenOptions},
  io::{self, BufReader, BufWriter},
  path::PathBuf,
  sync::atomic::{AtomicU64, Ordering},
  time::SystemTime,
};

use csv::{Reader, Writer};
use etcetera::{BaseStrategy, HomeDirError, choose_base_strategy};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::state::BuildReport;

/// Maximum number of historical builds to keep per derivation
const HISTORY_LIMIT: usize = 10;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Build report cache for CSV persistence
pub struct BuildReportCache {
  cache_path: PathBuf,
}

/// CSV row format for build reports
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BuildReportRow {
  hostname:        String,
  derivation_name: String,
  utc_time:        String,
  build_seconds:   u64,
}

impl BuildReportCache {
  /// Create a new cache instance with the given path
  #[must_use]
  pub const fn new(cache_path: PathBuf) -> Self {
    Self { cache_path }
  }

  /// Get the default cache file path
  pub fn default_cache_path() -> Result<PathBuf, HomeDirError> {
    let strategy = choose_base_strategy()?;
    Ok(
      strategy
        .state_dir()
        .unwrap_or_else(|| strategy.data_dir())
        .join("rom")
        .join("build-reports-v1.csv"),
    )
  }

  /// Load build reports from CSV
  ///
  /// Returns empty [`HashMap`] if file doesn't exist or parsing fails
  #[must_use]
  pub fn load(&self) -> HashMap<(String, String), Vec<BuildReport>> {
    if !self.cache_path.exists() {
      return HashMap::new();
    }

    let file = match File::open(&self.cache_path) {
      Ok(f) => f,
      Err(_) => return HashMap::new(),
    };

    let reader = BufReader::new(file);
    let mut csv_reader = Reader::from_reader(reader);

    let mut reports: HashMap<(String, String), Vec<BuildReport>> =
      HashMap::new();

    for result in csv_reader.deserialize() {
      let row: BuildReportRow = match result {
        Ok(r) => r,
        Err(_) => continue,
      };

      let completed_at = match parse_utc_time(&row.utc_time) {
        Some(t) => t,
        None => continue,
      };

      let report = BuildReport {
        duration_secs: row.build_seconds as f64,
        completed_at,
      };

      let key = (row.hostname, row.derivation_name);
      reports.entry(key).or_default().push(report);
    }

    // Sort each entry by timestamp (newest first) and limit to HISTORY_LIMIT
    for entries in reports.values_mut() {
      entries.sort_by_key(|entry| std::cmp::Reverse(entry.completed_at));
      entries.truncate(HISTORY_LIMIT);
    }

    reports
  }

  /// Save build reports to CSV
  ///
  /// Atomically replaces the store and enforces the history limit.
  pub fn save(
    &self,
    reports: &HashMap<(String, String), Vec<BuildReport>>,
  ) -> Result<(), std::io::Error> {
    // Ensure directory exists
    if let Some(parent) = self.cache_path.parent() {
      fs::create_dir_all(parent)?;
    }

    let lock_path = self.cache_path.with_extension("lock");
    let lock_file = OpenOptions::new()
      .read(true)
      .write(true)
      .create(true)
      .truncate(false)
      .open(&lock_path)?;
    lock_file.lock()?;

    let mut merged = self.load();
    for ((hostname, derivation_name), entries) in reports {
      let key = (hostname.clone(), derivation_name.clone());
      merged
        .entry(key)
        .or_default()
        .extend(entries.iter().cloned());
    }
    for entries in merged.values_mut() {
      let mut seen = HashSet::new();
      entries.retain(|report| {
        seen.insert((report.completed_at, report.duration_secs as u64))
      });
      entries.sort_by_key(|entry| std::cmp::Reverse(entry.completed_at));
      entries.truncate(HISTORY_LIMIT);
    }

    // Write to a temp file in the same directory, then rename atomically.
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let tmp_path = self.cache_path.with_extension(format!(
      "csv.{}.{}.tmp",
      std::process::id(),
      sequence,
    ));

    let file = OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(&tmp_path)?;

    let result: io::Result<()> = (|| {
      let writer = BufWriter::new(file);
      let mut csv_writer = Writer::from_writer(writer);

      // Flatten and write all reports
      for ((hostname, derivation_name), entries) in &merged {
        for report in entries {
          let row = BuildReportRow {
            hostname:        hostname.clone(),
            derivation_name: derivation_name.clone(),
            utc_time:        format_utc_time(report.completed_at)
              .map_err(io::Error::other)?,
            build_seconds:   report.duration_secs as u64,
          };
          csv_writer.serialize(row)?;
        }
      }

      csv_writer.flush()?;
      drop(csv_writer);

      // Atomic replace
      fs::rename(&tmp_path, &self.cache_path)?;
      Ok(())
    })();
    if result.is_err() {
      let _ = fs::remove_file(&tmp_path);
    }
    result
  }

  /// Calculate median build time from historical reports
  ///
  /// Returns [`None`] if there are no reports
  #[must_use]
  pub fn calculate_median(reports: &[BuildReport]) -> Option<u64> {
    if reports.is_empty() {
      return None;
    }

    let mut durations: Vec<u64> =
      reports.iter().map(|r| r.duration_secs as u64).collect();
    durations.sort_unstable();

    let len = durations.len();
    if len % 2 == 1 {
      Some(durations[len / 2])
    } else {
      let mid1 = durations[len / 2 - 1];
      let mid2 = durations[len / 2];
      Some(u64::midpoint(mid1, mid2))
    }
  }
}

pub fn parse_utc_time(input: &str) -> Option<SystemTime> {
  Some(input.parse::<Timestamp>().ok()?.into())
}

pub fn format_utc_time(time: SystemTime) -> Result<String, jiff::Error> {
  Ok(Timestamp::try_from(time)?.to_string())
}
