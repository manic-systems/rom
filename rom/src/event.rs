//! Typed domain events decoded from Nix and Lix's internal-JSON wire format.

use std::path::PathBuf;

use cognos::{Actions, Activities, Host, Id, ResultType, Verbosity};
use serde_json::Value;

use crate::state::{Blocker, Derivation, Progress, StorePath};

#[derive(Debug, Clone)]
pub enum Event {
  Start(StartEvent),
  Stop { id: Id },
  Message(MessageEvent),
  Result { id: Id, result: ActivityResult },
}

#[derive(Debug, Clone)]
pub struct StartEvent {
  pub id:       Id,
  pub parent:   Option<Id>,
  pub activity: Activities,
  pub subject:  ActivitySubject,
}

#[derive(Debug, Clone)]
pub enum ActivitySubject {
  ResolvedDerivation {
    original: Derivation,
    resolved: Derivation,
  },
  Build {
    derivation: Option<Derivation>,
    host:       Host,
    protocol:   Option<String>,
  },
  /// A build Nix will start once a lock or machine frees up.
  Blocked {
    derivations: Vec<Derivation>,
    outputs:     Vec<StorePath>,
    blocker:     Blocker,
  },
  Substitute {
    path:     Option<StorePath>,
    host:     Host,
    protocol: Option<String>,
  },
  CopyPath {
    path:          Option<StorePath>,
    from:          Host,
    from_protocol: Option<String>,
    to:            Host,
    to_protocol:   Option<String>,
  },
  /// A local source path being copied into, or only hashed for, the store.
  FetchToStore {
    source:  String,
    hashing: bool,
  },
  None,
}

#[derive(Debug, Clone)]
pub struct MessageEvent {
  pub level:                  Verbosity,
  pub is_error:               bool,
  pub styled:                 String,
  pub plain:                  String,
  pub announcement:           Option<Announcement>,
  pub derivation:             Option<Derivation>,
  pub planned_download_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum Announcement {
  Derivation(Derivation),
  StorePath(StorePath),
}

#[derive(Debug, Clone)]
pub enum ActivityResult {
  BuildLog(String),
  PostBuildLog(String),
  SetPhase(String),
  Progress(Progress),
  UntrustedPath(String),
  CorruptedPath(String),
  Ignored,
}

impl Event {
  pub(crate) fn decode(action: Actions) -> Self {
    match action {
      Actions::Start {
        id,
        parent,
        text,
        activity,
        fields,
        ..
      } => {
        Self::Start(StartEvent {
          id,
          parent: (parent != 0).then_some(parent),
          subject: decode_subject(activity, &text, &fields),
          activity,
        })
      },
      Actions::Stop { id } => Self::Stop { id },
      Actions::Message {
        level,
        msg,
        raw_msg,
        ..
      } => {
        let mut diagnostic = msg.as_str();
        while let Some(styled) = diagnostic.strip_prefix("\x1b[") {
          let Some((_, rest)) = styled.split_once('m') else {
            break;
          };
          diagnostic = rest;
        }
        let is_error = matches!(level, Verbosity::Error)
          && (raw_msg.is_some() || diagnostic.starts_with("error:"))
          && !diagnostic.starts_with("trace: ")
          && !diagnostic.starts_with("warning:");
        let plain = raw_msg.unwrap_or_else(|| msg.clone());
        Self::Message(MessageEvent {
          level,
          announcement: announcement(level, &plain),
          derivation: extract_derivation(&plain),
          planned_download_bytes: parse_planned_download_bytes(&plain),
          is_error,
          styled: msg,
          plain,
        })
      },
      Actions::Result {
        id,
        result_type,
        fields,
      } => {
        Self::Result {
          id,
          result: decode_result(result_type, &fields),
        }
      },
    }
  }
}

impl Announcement {
  pub(crate) fn derivation_path(&self) -> Option<PathBuf> {
    match self {
      Self::Derivation(derivation) => Some(derivation.path.clone()),
      Self::StorePath(_) => None,
    }
  }
}

fn decode_subject(
  activity: Activities,
  text: &str,
  fields: &[Value],
) -> ActivitySubject {
  match activity {
    Activities::BuildWaiting => {
      let [Value::String(original), Value::String(resolved)] = fields else {
        return decode_blocked(text);
      };
      match Derivation::parse(original).zip(Derivation::parse(resolved)) {
        Some((original, resolved)) => {
          ActivitySubject::ResolvedDerivation { original, resolved }
        },
        None => ActivitySubject::None,
      }
    },
    Activities::Build => {
      let (host, protocol) =
        parse_host(fields.get(1).and_then(Value::as_str).unwrap_or(""));
      ActivitySubject::Build {
        derivation: fields
          .first()
          .and_then(Value::as_str)
          .and_then(Derivation::parse)
          .or_else(|| extract_derivation(text)),
        host,
        protocol,
      }
    },
    Activities::Substitute => {
      let (host, protocol) =
        parse_host(fields.get(1).and_then(Value::as_str).unwrap_or(""));
      ActivitySubject::Substitute {
        path: fields
          .first()
          .and_then(Value::as_str)
          .and_then(StorePath::parse)
          .or_else(|| extract_store_path(text)),
        host,
        protocol,
      }
    },
    Activities::CopyPath => {
      let (from, from_protocol) =
        parse_host(fields.get(1).and_then(Value::as_str).unwrap_or(""));
      let (to, to_protocol) =
        parse_host(fields.get(2).and_then(Value::as_str).unwrap_or(""));
      ActivitySubject::CopyPath {
        path: fields
          .first()
          .and_then(Value::as_str)
          .and_then(StorePath::parse),
        from,
        from_protocol,
        to,
        to_protocol,
      }
    },
    Activities::FetchToStore => {
      let Some(source) = fields.first().and_then(Value::as_str) else {
        return ActivitySubject::None;
      };
      ActivitySubject::FetchToStore {
        source:  source.to_owned(),
        hashing: fields.get(1).and_then(Value::as_u64) == Some(1),
      }
    },
    _ => ActivitySubject::None,
  }
}

fn decode_result(result_type: ResultType, fields: &[Value]) -> ActivityResult {
  match result_type {
    ResultType::BuildLogLine => {
      fields
        .first()
        .and_then(Value::as_str)
        .map_or(ActivityResult::Ignored, |line| {
          ActivityResult::BuildLog(line.to_owned())
        })
    },
    ResultType::PostBuildLogLine => {
      fields
        .first()
        .and_then(Value::as_str)
        .map_or(ActivityResult::Ignored, |line| {
          ActivityResult::PostBuildLog(line.to_owned())
        })
    },
    ResultType::Progress => {
      match fields {
        [done, expected, ..] => {
          let count =
            |at: usize| fields.get(at).and_then(Value::as_u64).unwrap_or(0);
          match (done.as_u64(), expected.as_u64()) {
            (Some(done), Some(expected)) => {
              ActivityResult::Progress(Progress {
                done,
                expected,
                running: count(2),
                failed: count(3),
              })
            },
            _ => ActivityResult::Ignored,
          }
        },
        _ => ActivityResult::Ignored,
      }
    },
    ResultType::UntrustedPath => {
      first_string(fields)
        .map_or(ActivityResult::Ignored, ActivityResult::UntrustedPath)
    },
    ResultType::CorruptedPath => {
      first_string(fields)
        .map_or(ActivityResult::Ignored, ActivityResult::CorruptedPath)
    },
    ResultType::SetPhase => {
      fields
        .first()
        .and_then(Value::as_str)
        .map_or(ActivityResult::Ignored, |phase| {
          ActivityResult::SetPhase(phase.to_owned())
        })
    },
    ResultType::FileLinked
    | ResultType::SetExpected
    | ResultType::FetchStatus
    | ResultType::FetchToStore => ActivityResult::Ignored,
  }
}

fn first_string(fields: &[Value]) -> Option<String> {
  fields.first()?.as_str().map(str::to_owned)
}

fn announcement(level: Verbosity, message: &str) -> Option<Announcement> {
  if !matches!(
    level,
    Verbosity::Warning | Verbosity::Notice | Verbosity::Info
  ) {
    return None;
  }
  let path = message.strip_prefix("  ")?.trim_end();
  if !path.starts_with("/nix/store/") || path.contains(char::is_whitespace) {
    return None;
  }
  Derivation::parse(path)
    .map(Announcement::Derivation)
    .or_else(|| StorePath::parse(path).map(Announcement::StorePath))
}

fn decode_blocked(text: &str) -> ActivitySubject {
  let blocker = if text.starts_with("waiting for lock on ") {
    Blocker::Lock
  } else if text.starts_with("waiting for a machine to build ") {
    Blocker::Machine
  } else {
    return ActivitySubject::None;
  };
  let (mut derivations, mut outputs) = (Vec::new(), Vec::new());

  // Locks on floating outputs name the derivation plus an output suffix.
  for quoted in text.split('\'').skip(1).step_by(2) {
    if let Some(end) = quoted.find(".drv") {
      derivations.extend(Derivation::parse(&quoted[..end + 4]));
    } else {
      outputs.extend(StorePath::parse(quoted));
    }
  }

  ActivitySubject::Blocked {
    derivations,
    outputs,
    blocker,
  }
}

fn extract_derivation(text: &str) -> Option<Derivation> {
  let start = text.find("/nix/store/")?;
  let end = text[start..].find(".drv")?;
  Derivation::parse(&text[start..start + end + 4])
}

fn extract_store_path(text: &str) -> Option<StorePath> {
  let start = text.find("/nix/store/")?;
  let rest = &text[start..];
  let end = rest
    .find(|character: char| {
      character.is_whitespace() || character == '\'' || character == '"'
    })
    .unwrap_or(rest.len());
  StorePath::parse(&rest[..end])
}

#[expect(
  clippy::cast_sign_loss,
  reason = "negative and non-finite values are rejected before casting"
)]
fn parse_planned_download_bytes(message: &str) -> Option<u64> {
  let (_, rest) = message.split_once("will be fetched (")?;
  let (download_part, _) = rest.split_once(" download")?;
  let mut parts = download_part.split_whitespace();
  let number: f64 = parts.next()?.parse().ok()?;
  if !number.is_finite() || number < 0.0 {
    return None;
  }
  let unit = parts.next()?;
  let multiplier = match unit {
    "B" => 1.0_f64,
    "KiB" => 1024.0_f64,
    "MiB" => 1024.0_f64 * 1024.0,
    "GiB" => 1024.0_f64 * 1024.0 * 1024.0,
    "TiB" => 1024.0_f64 * 1024.0 * 1024.0 * 1024.0,
    _ => return None,
  };
  Some((number * multiplier).round() as u64)
}

fn parse_host(value: &str) -> (Host, Option<String>) {
  let value = value.trim();
  if value.is_empty()
    || matches!(
      value,
      "localhost" | "local" | "local://" | "unix" | "unix://"
    )
  {
    return (Host::Localhost, None);
  }
  let (protocol, store) =
    value
      .split_once("://")
      .map_or((None, value), |(proto, rest)| {
        let proto = (!proto.is_empty()).then(|| proto.to_owned());
        (proto, rest)
      });
  let without_protocol = store
    .split_once('?')
    .map_or(store, |(machine, _)| machine)
    .trim_end_matches('/');
  let hostname = without_protocol
    .split('@')
    .next_back()
    .unwrap_or(without_protocol)
    .trim();
  if hostname.is_empty() || hostname == "localhost" {
    (Host::Localhost, None)
  } else {
    (Host::Remote(hostname.to_owned()), protocol)
  }
}

#[cfg(test)]
#[expect(clippy::panic, reason = "tests fail by panicking")]
mod tests {
  use super::*;

  #[test]
  fn build_payload_is_decoded_once() {
    let event = Event::decode(serde_json::from_str(
      r#"{"action":"start","id":1,"level":3,"parent":0,"text":"building","type":105,"fields":["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-demo.drv","ssh://user@builder/",1,1]}"#,
    ).unwrap());
    let Event::Start(StartEvent {
      subject:
        ActivitySubject::Build {
          derivation,
          host,
          protocol,
        },
      ..
    }) = event
    else {
      panic!("expected build event");
    };
    assert_eq!(derivation.unwrap().name, "demo");
    assert_eq!(host, Host::Remote("builder".to_owned()));
    assert_eq!(protocol.as_deref(), Some("ssh"));
  }
}
