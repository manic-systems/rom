//! Canonical state reduction for typed Nix and Lix events.

use std::path::PathBuf;

use cognos::{Activities, Host, Id, Verbosity};

use crate::{
  cache::BuildReportCache,
  event::{
    ActivityResult,
    ActivitySubject,
    Announcement,
    Event,
    MessageEvent,
    StartEvent,
  },
  state::{
    ActivityProgress,
    ActivityStatus,
    BuildFail,
    BuildInfo,
    BuildReport,
    BuildStatus,
    DerivationId,
    FailType,
    State,
    TransferInfo,
  },
};

#[derive(Debug, Default)]
pub(crate) struct Effects {
  pub changed: bool,
  pub log:     Option<LogEffect>,
  pub resolve: Vec<PathBuf>,
  pub stopped: Option<Id>,
}

#[derive(Debug)]
pub(crate) struct LogEffect {
  pub activity: Option<Id>,
  pub styled:   String,
  pub plain:    String,
  pub force:    bool,
}

/// Apply one typed event and return every external effect caused by it.
pub(crate) fn apply_event_at(
  state: &mut State,
  event: Event,
  now: f64,
) -> Effects {
  let mut effects = Effects::default();
  effects.changed |= state.begin_at(now);

  match event {
    Event::Start(start) => apply_start(state, start, now, &mut effects),
    Event::Stop { id } => {
      effects.changed |= apply_stop(state, id, now);
      effects.stopped = Some(id);
    },
    Event::Message(message) => {
      effects.changed |= apply_message(state, &message, now, &mut effects);
      effects.log = Some(LogEffect {
        activity: None,
        force:    matches!(message.level, Verbosity::Error),
        styled:   message.styled,
        plain:    message.plain,
      });
    },
    Event::Result { id, result } => {
      match result {
        ActivityResult::BuildLog(line) => {
          effects.log = Some(LogEffect {
            activity: Some(id),
            styled:   line.clone(),
            plain:    line,
            force:    false,
          });
        },
        ActivityResult::PostBuildLog(line) => {
          let line = format!("[post-build] {line}");
          effects.log = Some(LogEffect {
            activity: Some(id),
            styled:   line.clone(),
            plain:    line,
            force:    false,
          });
        },
        ActivityResult::Progress {
          done,
          expected,
          running,
          failed,
        } => {
          effects.changed |=
            apply_progress(state, id, done, expected, running, failed);
        },
        ActivityResult::UntrustedPath(path) => {
          state.record_error(format!("Untrusted path: {path}"));
          effects.changed = true;
        },
        ActivityResult::CorruptedPath(path) => {
          state.record_error(format!("Corrupted path: {path}"));
          effects.changed = true;
        },
        ActivityResult::Ignored => {},
      }
    },
  }

  effects
}

fn apply_start(
  state: &mut State,
  start: StartEvent,
  now: f64,
  effects: &mut Effects,
) {
  let mut derivation = None;
  let mut store_path = None;

  match start.subject {
    ActivitySubject::Build {
      derivation: Some(drv),
      host,
    } => {
      effects.resolve.push(drv.path.clone());
      let estimate = get_build_estimate(state, &drv.name, &host);
      let parent = start
        .parent
        .and_then(|parent| state.activity_derivation(parent));
      let id = state.start_build(
        drv,
        BuildInfo {
          start: now,
          host,
          estimate,
          activity_id: Some(start.id),
        },
        parent,
      );
      derivation = Some(id);
      effects.changed = true;
    },
    ActivitySubject::Substitute {
      path: Some(path),
      host,
    } => {
      let producer = start
        .parent
        .and_then(|parent| state.activity_derivation(parent));
      let id = state.start_download(
        path,
        TransferInfo {
          start: now,
          host,
          activity_id: start.id,
          bytes_transferred: 0,
          total_bytes: None,
        },
        producer,
      );
      store_path = Some(id);
      effects.changed = true;
    },
    ActivitySubject::CopyPath {
      path: Some(path),
      from,
      to,
    } => {
      let download =
        matches!(to, Host::Localhost) && !matches!(from, Host::Localhost);
      let transfer = TransferInfo {
        start:             now,
        host:              if matches!(to, Host::Localhost) {
          from
        } else {
          to
        },
        activity_id:       start.id,
        bytes_transferred: 0,
        total_bytes:       None,
      };
      let id = if download {
        state.start_download(path, transfer, None)
      } else {
        state.start_upload(path, transfer)
      };
      store_path = Some(id);
      effects.changed = true;
    },
    ActivitySubject::Build {
      derivation: None, ..
    }
    | ActivitySubject::Substitute { path: None, .. }
    | ActivitySubject::CopyPath { path: None, .. }
    | ActivitySubject::None => {},
  }

  state.insert_activity(start.id, ActivityStatus {
    activity: start.activity,
    parent: start.parent,
    derivation,
    store_path,
    progress: None,
  });
}

fn apply_stop(state: &mut State, id: Id, now: f64) -> bool {
  let Some(activity) = state.take_activity(id) else {
    return false;
  };
  match activity.activity {
    Activities::Build => {
      activity
        .derivation
        .is_some_and(|derivation| complete_build(state, derivation, now))
    },
    Activities::Substitute => {
      activity
        .store_path
        .is_some_and(|path| state.complete_download(path, now))
    },
    Activities::CopyPath | Activities::FileTransfer => {
      activity
        .store_path
        .is_some_and(|path| state.complete_transfer(path, now))
    },
    _ => false,
  }
}

fn apply_message(
  state: &mut State,
  message: &MessageEvent,
  now: f64,
  effects: &mut Effects,
) -> bool {
  let mut changed = false;
  if let Some(announcement) = &message.announcement {
    changed |= match announcement {
      Announcement::Derivation(derivation) => {
        state.plan_derivation(derivation.clone()).1
      },
      Announcement::StorePath(path) => state.plan_store_path(path.clone()),
    };
    if let Some(path) = announcement.derivation_path() {
      effects.resolve.push(path);
    }
  }

  if matches!(message.level, Verbosity::Error)
    && (message.plain.contains("error:") || message.plain.contains("failed"))
  {
    state.record_error(message.plain.clone());
    changed = true;
    if let Some(derivation) = &message.derivation {
      let id = state.get_or_create_derivation_id(derivation.clone());
      let build = state.get_derivation_info(id).and_then(|info| {
        if let BuildStatus::Building(build) = &info.build_status {
          Some(build.clone())
        } else {
          None
        }
      });
      if let Some(build) = build {
        state.update_build_status(id, BuildStatus::Failed {
          info: build,
          fail: BuildFail {
            at:        now,
            fail_type: parse_fail_type(&message.plain),
          },
        });
      }
    }
  }
  changed
}

fn apply_progress(
  state: &mut State,
  id: Id,
  done: u64,
  expected: u64,
  running: u64,
  failed: u64,
) -> bool {
  state.update_activity_progress(id, ActivityProgress {
    done,
    expected,
    running,
    failed,
  })
}

fn complete_build(state: &mut State, id: DerivationId, now: f64) -> bool {
  let Some((build, name, platform)) =
    state.get_derivation_info(id).and_then(|info| {
      if let BuildStatus::Building(build) = &info.build_status {
        Some((build.clone(), info.name.name.clone(), info.platform.clone()))
      } else {
        None
      }
    })
  else {
    return false;
  };
  let start = build.start;
  let host = build.host.clone();
  state.update_build_status(id, BuildStatus::Built {
    info: build,
    end:  now,
  });
  record_build_completion(state, name, platform, start, now, &host);
  true
}

fn get_build_estimate(
  state: &State,
  derivation_name: &str,
  host: &Host,
) -> Option<u64> {
  BuildReportCache::calculate_median(
    state.build_reports(host, derivation_name)?,
  )
}

fn record_build_completion(
  state: &mut State,
  derivation_name: String,
  platform: Option<String>,
  start: f64,
  end: f64,
  host: &Host,
) {
  let report = BuildReport {
    derivation_name: derivation_name.clone(),
    platform:        platform.unwrap_or_default(),
    duration_secs:   end - start,
    completed_at:    std::time::SystemTime::now(),
    host:            host.name().to_string(),
    success:         true,
  };
  state.record_build_report(report);
}

fn parse_fail_type(message: &str) -> FailType {
  if message.contains("timeout") {
    FailType::Timeout
  } else if message.contains("hash mismatch") || message.contains("hash") {
    FailType::HashMismatch
  } else if message.contains("dependency failed") {
    FailType::DependencyFailed
  } else {
    FailType::Unknown
  }
}
