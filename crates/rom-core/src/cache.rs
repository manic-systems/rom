use std::{
  cmp::Reverse,
  collections::HashMap,
  fs::{self, File, OpenOptions},
  io::{BufReader, BufWriter},
  path::PathBuf,
  time::SystemTime,
};

use chrono::{DateTime, NaiveDateTime, Utc};
use csv::{Reader, Writer};
use serde::{Deserialize, Serialize};

use crate::state::BuildReport;

/// Maximum number of historical builds to keep per derivation
const HISTORY_LIMIT: usize = 10;

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
  #[must_use]
  pub fn default_cache_path() -> PathBuf {
    dirs::state_dir()
      .unwrap_or_else(|| {
        dirs::home_dir().unwrap_or_default().join(".local/state")
      })
      .join("rom")
      .join("build-reports.csv")
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
        derivation_name: row.derivation_name.clone(),
        duration_secs: row.build_seconds as f64,
        completed_at,
        host: row.hostname.clone(),
        success: true, // only successful builds are cached

        // FIXME: not stored in CSV. This is for simplicity, and because I'm
        // lazy
        platform: String::new(),
      };

      let key = (row.hostname, row.derivation_name);
      reports.entry(key).or_default().push(report);
    }

    // Sort each entry by timestamp (newest first) and limit to HISTORY_LIMIT
    for entries in reports.values_mut() {
      entries.sort_by_key(|entry| Reverse(entry.completed_at));
      entries.truncate(HISTORY_LIMIT);
    }

    reports
  }

  /// Save build reports to CSV
  ///
  /// Merges with existing reports and enforces history limit
  pub fn save(
    &self,
    reports: &HashMap<(String, String), Vec<BuildReport>>,
  ) -> Result<(), std::io::Error> {
    // Ensure directory exists
    if let Some(parent) = self.cache_path.parent() {
      fs::create_dir_all(parent)?;
    }

    // Load existing reports to merge
    let mut merged = self.load();

    // Merge new reports
    for ((host, drv_name), new_reports) in reports {
      let key = (host.clone(), drv_name.clone());
      let existing = merged.entry(key).or_default();

      // Add new reports
      existing.extend(new_reports.iter().cloned());

      // Sort by timestamp (newest first)
      existing.sort_by_key(|entry| Reverse(entry.completed_at));

      // Keep only most recent HISTORY_LIMIT entries
      existing.truncate(HISTORY_LIMIT);
    }

    // Write to a temp file in the same directory, then rename atomically.
    // This prevents a concurrent save() from corrupting the cache file.
    let tmp_path = self.cache_path.with_extension("csv.tmp");

    let file = OpenOptions::new()
      .write(true)
      .create(true)
      .truncate(true)
      .open(&tmp_path)?;

    let writer = BufWriter::new(file);
    let mut csv_writer = Writer::from_writer(writer);

    // Flatten and write all reports
    for ((hostname, derivation_name), entries) in merged {
      for report in entries {
        let row = BuildReportRow {
          hostname:        hostname.clone(),
          derivation_name: derivation_name.clone(),
          utc_time:        format_utc_time(report.completed_at),
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

  /// Get median build time for a specific derivation on a host
  #[must_use]
  pub fn get_estimate(
    &self,
    reports: &HashMap<(String, String), Vec<BuildReport>>,
    host: &str,
    derivation_name: &str,
  ) -> Option<u64> {
    let key = (host.to_string(), derivation_name.to_string());
    let entries = reports.get(&key)?;
    Self::calculate_median(entries)
  }
}

pub fn parse_utc_time(s: &str) -> Option<SystemTime> {
  let ndt = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()?;
  let dt: DateTime<Utc> = ndt.and_utc();
  let secs = dt.timestamp();
  if secs < 0 {
    return None;
  }
  Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64))
}

pub fn format_utc_time(time: SystemTime) -> String {
  let duration = time
    .duration_since(SystemTime::UNIX_EPOCH)
    .unwrap_or_default();
  let dt = DateTime::<Utc>::from_timestamp(duration.as_secs() as i64, 0)
    .unwrap_or_default();
  dt.format("%Y-%m-%d %H:%M:%S").to_string()
}
