use std::{collections::HashMap, time::SystemTime};

use rom::{
  cache::{BuildReportCache, format_utc_time, parse_utc_time},
  state::BuildReport,
};

#[test]
fn test_calculate_median_odd() {
  let reports = vec![
    BuildReport {
      derivation_name: "test".to_string(),
      platform:        "x86_64-linux".to_string(),
      duration_secs:   10.0,
      completed_at:    SystemTime::UNIX_EPOCH,
      host:            "localhost".to_string(),
      success:         true,
    },
    BuildReport {
      derivation_name: "test".to_string(),
      platform:        "x86_64-linux".to_string(),
      duration_secs:   20.0,
      completed_at:    SystemTime::UNIX_EPOCH,
      host:            "localhost".to_string(),
      success:         true,
    },
    BuildReport {
      derivation_name: "test".to_string(),
      platform:        "x86_64-linux".to_string(),
      duration_secs:   30.0,
      completed_at:    SystemTime::UNIX_EPOCH,
      host:            "localhost".to_string(),
      success:         true,
    },
  ];

  assert_eq!(BuildReportCache::calculate_median(&reports), Some(20));
}

#[test]
fn test_calculate_median_even() {
  let reports = vec![
    BuildReport {
      derivation_name: "test".to_string(),
      platform:        "x86_64-linux".to_string(),
      duration_secs:   10.0,
      completed_at:    SystemTime::UNIX_EPOCH,
      host:            "localhost".to_string(),
      success:         true,
    },
    BuildReport {
      derivation_name: "test".to_string(),
      platform:        "x86_64-linux".to_string(),
      duration_secs:   20.0,
      completed_at:    SystemTime::UNIX_EPOCH,
      host:            "localhost".to_string(),
      success:         true,
    },
  ];

  assert_eq!(BuildReportCache::calculate_median(&reports), Some(15));
}

#[test]
fn test_calculate_median_empty() {
  let reports = vec![];
  assert_eq!(BuildReportCache::calculate_median(&reports), None);
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
  let report = BuildReport {
    derivation_name: "demo".to_string(),
    platform:        "x86_64-linux".to_string(),
    duration_secs:   2.0,
    completed_at:    SystemTime::UNIX_EPOCH,
    host:            "localhost".to_string(),
    success:         true,
  };
  let mut history = HashMap::new();
  history.insert(("localhost".to_string(), "demo".to_string()), vec![report]);
  cache.save(&history).unwrap();
  let loaded = cache.load();
  cache.save(&loaded).unwrap();
  assert_eq!(cache.load().values().next().unwrap().len(), 1);
}
