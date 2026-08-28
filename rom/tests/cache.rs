use std::{collections::HashMap, time::SystemTime};

use rom::{
  cache::{BuildReportCache, format_utc_time, parse_utc_time},
  state::BuildReport,
};

fn report(duration_secs: f64) -> BuildReport {
  BuildReport {
    duration_secs,
    completed_at: SystemTime::UNIX_EPOCH,
  }
}

#[test]
fn test_calculate_median_odd() {
  let reports = vec![report(10.0), report(20.0), report(30.0)];
  assert_eq!(BuildReportCache::calculate_median(&reports), Some(20));
}

#[test]
fn test_calculate_median_even() {
  let reports = vec![report(10.0), report(20.0)];
  assert_eq!(BuildReportCache::calculate_median(&reports), Some(15));
}

#[test]
fn test_calculate_median_empty() {
  assert_eq!(BuildReportCache::calculate_median(&[]), None);
}

#[test]
fn test_format_parse_utc_time() {
  let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
  let formatted = format_utc_time(time);
  let parsed = parse_utc_time(&formatted).unwrap();

  let diff = parsed
    .duration_since(time)
    .unwrap_or_else(|e| e.duration())
    .as_secs();
  assert_eq!(diff, 0);
}

#[test]
fn saving_loaded_history_does_not_duplicate_it() {
  let directory = tempfile::tempdir().unwrap();
  let cache = BuildReportCache::new(directory.path().join("history.csv"));
  let mut history = HashMap::new();
  history.insert(("localhost".to_string(), "demo".to_string()), vec![report(
    2.0,
  )]);
  cache.save(&history).unwrap();
  let loaded = cache.load();
  cache.save(&loaded).unwrap();
  assert_eq!(cache.load().values().next().unwrap().len(), 1);
}
