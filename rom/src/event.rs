//! Typed domain events decoded from Nix and Lix's internal-JSON wire format.

use std::path::PathBuf;

use cognos::{Actions, Activities, Host, Id, ResultType, Verbosity};
use serde_json::Value;

use crate::state::{Derivation, StorePath};

#[derive(Debug, Clone)]
pub(crate) enum Event {
  Start(StartEvent),
  Stop { id: Id },
  Message(MessageEvent),
  Result { id: Id, result: ActivityResult },
}

#[derive(Debug, Clone)]
pub(crate) struct StartEvent {
  pub id:       Id,
  pub parent:   Option<Id>,
  pub activity: Activities,
  pub subject:  ActivitySubject,
}

#[derive(Debug, Clone)]
pub(crate) enum ActivitySubject {
  Build {
    derivation: Option<Derivation>,
    host:       Host,
  },
  Substitute {
    path: Option<StorePath>,
    host: Host,
  },
  CopyPath {
    path: Option<StorePath>,
    from: Host,
    to:   Host,
  },
  None,
}

#[derive(Debug, Clone)]
pub(crate) struct MessageEvent {
  pub level:        Verbosity,
  pub styled:       String,
  pub plain:        String,
  pub announcement: Option<Announcement>,
  pub derivation:   Option<Derivation>,
}

#[derive(Debug, Clone)]
pub(crate) enum Announcement {
  Derivation(Derivation),
  StorePath(StorePath),
}

#[derive(Debug, Clone)]
pub(crate) enum ActivityResult {
  BuildLog(String),
  PostBuildLog(String),
  SetPhase(String),
  Progress {
    done:     u64,
    expected: u64,
    running:  u64,
    failed:   u64,
  },
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
        let plain = raw_msg.unwrap_or_else(|| msg.clone());
        Self::Message(MessageEvent {
          announcement: announcement(level, &plain),
          derivation: extract_derivation(&plain),
          level,
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
    Activities::Build => {
      ActivitySubject::Build {
        derivation: fields
          .first()
          .and_then(Value::as_str)
          .and_then(Derivation::parse)
          .or_else(|| extract_derivation(text)),
        host:       parse_host(
          fields.get(1).and_then(Value::as_str).unwrap_or(""),
        ),
      }
    },
    Activities::Substitute => {
      ActivitySubject::Substitute {
        path: fields
          .first()
          .and_then(Value::as_str)
          .and_then(StorePath::parse)
          .or_else(|| extract_store_path(text)),
        host: parse_host(fields.get(1).and_then(Value::as_str).unwrap_or("")),
      }
    },
    Activities::CopyPath => {
      ActivitySubject::CopyPath {
        path: fields
          .first()
          .and_then(Value::as_str)
          .and_then(StorePath::parse),
        from: parse_host(fields.get(1).and_then(Value::as_str).unwrap_or("")),
        to:   parse_host(fields.get(2).and_then(Value::as_str).unwrap_or("")),
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
        .map(|line| ActivityResult::BuildLog(line.to_string()))
        .unwrap_or(ActivityResult::Ignored)
    },
    ResultType::PostBuildLogLine => {
      fields
        .first()
        .and_then(Value::as_str)
        .map(|line| ActivityResult::PostBuildLog(line.to_string()))
        .unwrap_or(ActivityResult::Ignored)
    },
    ResultType::Progress => {
      match fields {
        [done, expected, running, failed, ..] => {
          match (
            done.as_u64(),
            expected.as_u64(),
            running.as_u64(),
            failed.as_u64(),
          ) {
            (Some(done), Some(expected), Some(running), Some(failed)) => {
              ActivityResult::Progress {
                done,
                expected,
                running,
                failed,
              }
            },
            _ => ActivityResult::Ignored,
          }
        },
        _ => ActivityResult::Ignored,
      }
    },
    ResultType::UntrustedPath => {
      first_string(fields)
        .map(ActivityResult::UntrustedPath)
        .unwrap_or(ActivityResult::Ignored)
    },
    ResultType::CorruptedPath => {
      first_string(fields)
        .map(ActivityResult::CorruptedPath)
        .unwrap_or(ActivityResult::Ignored)
    },
    ResultType::SetPhase => {
      fields
        .first()
        .and_then(Value::as_str)
        .map(|phase| ActivityResult::SetPhase(phase.to_string()))
        .unwrap_or(ActivityResult::Ignored)
    },
    ResultType::FileLinked
    | ResultType::SetExpected
    | ResultType::FetchStatus => ActivityResult::Ignored,
  }
}

fn first_string(fields: &[Value]) -> Option<String> {
  fields.first()?.as_str().map(str::to_string)
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

fn parse_host(value: &str) -> Host {
  let value = value.trim();
  if value.is_empty()
    || matches!(
      value,
      "localhost" | "local" | "local://" | "unix" | "unix://"
    )
  {
    return Host::Localhost;
  }
  let without_protocol = value
    .strip_prefix("ssh://")
    .or_else(|| value.strip_prefix("https://"))
    .or_else(|| value.strip_prefix("http://"))
    .unwrap_or(value)
    .trim_end_matches('/');
  let hostname = without_protocol
    .split('@')
    .next_back()
    .unwrap_or(without_protocol)
    .trim();
  if hostname.is_empty() || hostname == "localhost" {
    Host::Localhost
  } else {
    Host::Remote(hostname.to_string())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn build_payload_is_decoded_once() {
    let event = Event::decode(serde_json::from_str(
      r#"{"action":"start","id":1,"level":3,"parent":0,"text":"building","type":105,"fields":["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-demo.drv","ssh://user@builder/",1,1]}"#,
    ).unwrap());
    let Event::Start(StartEvent {
      subject: ActivitySubject::Build { derivation, host },
      ..
    }) = event
    else {
      panic!("expected build event");
    };
    assert_eq!(derivation.unwrap().name, "demo");
    assert_eq!(host, Host::Remote("builder".to_string()));
  }
}
