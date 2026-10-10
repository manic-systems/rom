//! Decoder for `--log-format internal-json` records.

use serde::Deserialize;
use serde_repr::Deserialize_repr;

/// Activity types used in `start` actions.
#[derive(Deserialize_repr, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Activities {
  /// Activity of unknown kind.
  Unknown       = 0,
  /// Copying one store path between stores.
  CopyPath      = 100,
  /// Downloading a file.
  FileTransfer  = 101,
  /// Realising derivation outputs.
  Realise       = 102,
  /// Copying a set of store paths.
  CopyPaths     = 103,
  /// Building a set of derivations.
  Builds        = 104,
  /// Building one derivation.
  Build         = 105,
  /// Deduplicating files in the store.
  OptimiseStore = 106,
  /// Verifying a store path.
  VerifyPath    = 107,
  /// Substituting a store path from a binary cache.
  Substitute    = 108,
  /// Querying store path metadata from a substituter.
  QueryPathInfo = 109,
  /// Running a post-build hook.
  PostBuildHook = 110,
  /// Waiting for another process to release a build lock.
  BuildWaiting  = 111,
  /// Fetching a source tree, such as a flake input.
  FetchTree     = 112,
  /// Copying a source into the store.
  FetchToStore  = 113,
}

/// Result types used in `result` actions. Numerically overlap with
/// `Activities` but carry entirely different semantics; do not conflate.
#[derive(Deserialize_repr, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResultType {
  /// Two ints: (`linked_count`, `total_count`).
  FileLinked       = 100,
  /// One string: a log line emitted by the builder.
  BuildLogLine     = 101,
  /// One string: store path that is not trusted.
  UntrustedPath    = 102,
  /// One string: store path that is corrupted.
  CorruptedPath    = 103,
  /// One string: current build phase name, such as `configurePhase`.
  SetPhase         = 104,
  /// Four ints: (done, expected, running, failed).
  Progress         = 105,
  /// Two ints: (`activity_type`, `expected_count`).
  SetExpected      = 106,
  /// One string: a log line from a post-build hook.
  PostBuildLogLine = 107,
  /// One string: fetch status message.
  FetchStatus      = 108,
  /// One string: resulting store path from a fetch-to-store activity.
  FetchToStore     = 109,
}

/// Log level of an action, from most to least severe.
#[derive(
  Deserialize_repr, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord,
)]
#[repr(u8)]
pub enum Verbosity {
  /// Errors.
  Error     = 0,
  /// Warnings.
  Warning   = 1,
  /// Notices.
  Notice    = 2,
  /// Informational messages.
  Info      = 3,
  /// Talkative messages.
  Talkative = 4,
  /// Chatty messages.
  Chatty    = 5,
  /// Debugging messages.
  Debug     = 6,
  /// Everything, including the noisiest tracing.
  Vomit     = 7,
}

/// Identifier of an activity.
pub type Id = u64;

/// One action record from `--log-format internal-json`.
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "action")]
pub enum Actions {
  /// An activity has started.
  #[serde(rename = "start")]
  Start {
    /// Identifier of the new activity.
    id:       Id,
    /// Log level of the activity.
    level:    Verbosity,
    /// Identifier of the parent activity, or `0` for none.
    #[serde(default)]
    parent:   Id,
    /// Human-readable description.
    text:     String,
    /// Kind of activity.
    #[serde(rename = "type")]
    activity: Activities,
    /// Activity-specific fields.
    #[serde(default)]
    fields:   Vec<serde_json::Value>,
  },

  /// An activity has stopped.
  #[serde(rename = "stop")]
  Stop {
    /// Identifier of the stopped activity.
    id: Id,
  },

  /// A log/diagnostic message.
  ///
  /// Lix extends this with optional source-location fields (`file`, `line`,
  /// `column`) and `raw_msg` (the message text without ANSI escape sequences).
  /// Nix omits these fields entirely; serde defaults them to `None` so the
  /// same struct parses both.
  #[serde(rename = "msg")]
  Message {
    /// Log level of the message.
    level:   Verbosity,
    /// Message text, possibly with ANSI escape codes.
    msg:     String,
    /// Message without ANSI escape codes (Lix only).
    #[serde(default)]
    raw_msg: Option<String>,
    /// Source file that produced this message (Lix only).
    #[serde(default)]
    file:    Option<String>,
    /// Source line number (Lix only).
    #[serde(default)]
    line:    Option<u32>,
    /// Source column number (Lix only).
    #[serde(default)]
    column:  Option<u32>,
  },

  /// An activity reported a result.
  #[serde(rename = "result")]
  Result {
    /// Result-specific fields.
    #[serde(default)]
    fields:      Vec<serde_json::Value>,
    /// Identifier of the activity that reported the result.
    id:          Id,
    /// Kind of result.
    #[serde(rename = "type")]
    result_type: ResultType,
  },
}

/// A well-formed record that uses a protocol extension this crate does not
/// know.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UnsupportedRecord {
  /// Value of the record's `action` field.
  pub action: String,
  /// Value of the record's `type` field, if it is an integer.
  pub kind:   Option<u64>,
}

/// Outcome of decoding one internal-JSON record.
#[derive(Debug, Clone)]
pub enum DecodedAction {
  /// A record of a known action and kind.
  Known(Actions),
  /// A well-formed record from a newer protocol version.
  Unsupported(UnsupportedRecord),
}

#[derive(Deserialize)]
struct Envelope {
  action: String,
  #[serde(rename = "type")]
  kind:   Option<serde_json::Value>,
  level:  Option<serde_json::Value>,
}

/// Decode one internal-JSON payload while distinguishing valid future protocol
/// extensions from malformed instances of the current protocol.
///
/// # Errors
///
/// Returns an error if `json` is not a JSON object with an `action` field, or
/// if it is a malformed record of a known action and kind.
pub fn decode_action(json: &[u8]) -> Result<DecodedAction, serde_json::Error> {
  let value: serde_json::Value = serde_json::from_slice(json)?;
  let envelope: Envelope = serde_json::from_value(value.clone())?;
  let kind = envelope.kind.as_ref().and_then(serde_json::Value::as_u64);
  let level = envelope.level.as_ref().and_then(serde_json::Value::as_u64);
  let unsupported = match envelope.action.as_str() {
    "start" => {
      kind.is_some_and(|kind| !matches!(kind, 0 | 100..=113))
        || level.is_some_and(|level| level > 7)
    },
    "msg" => level.is_some_and(|level| level > 7),
    "result" => kind.is_some_and(|kind| !matches!(kind, 100..=109)),
    "stop" => false,
    _ => true,
  };
  if unsupported {
    return Ok(DecodedAction::Unsupported(UnsupportedRecord {
      action: envelope.action,
      kind,
    }));
  }
  serde_json::from_value(value).map(DecodedAction::Known)
}

/// Parse a single line of `--log-format internal-json` output.
/// Lines are prefixed with `@nix ` followed by a JSON object.
/// Returns `None` for lines that are not internal-json messages.
#[must_use]
pub fn parse_line(line: &str) -> Option<Actions> {
  let json = line.strip_prefix("@nix ")?;
  match decode_action(json.as_bytes()).ok()? {
    DecodedAction::Known(action) => Some(action),
    DecodedAction::Unsupported(_) => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parse(json: &str) -> Actions {
    serde_json::from_str(json).expect("parse failed")
  }

  #[test]
  fn start_build_nix() {
    // Standard Nix/Lix Build start: fields = [drv_path, host, round, nrRounds]
    let json = r#"{
      "action":"start",
      "id":1234,
      "level":3,
      "parent":0,
      "text":"building '/nix/store/abc-hello.drv'",
      "type":105,
      "fields":["/nix/store/abc-hello.drv","",1,1]
    }"#;
    match parse(json) {
      Actions::Start {
        id,
        activity,
        fields,
        ..
      } => {
        assert_eq!(id, 1234);
        assert_eq!(activity, Activities::Build);
        assert_eq!(fields[0].as_str().unwrap(), "/nix/store/abc-hello.drv");
        assert_eq!(fields[1].as_str().unwrap(), "");
        assert_eq!(fields[2].as_u64().unwrap(), 1); // round
        assert_eq!(fields[3].as_u64().unwrap(), 1); // nrRounds
      },
      _ => panic!("expected Start"),
    }
  }

  #[test]
  fn start_substitute() {
    let json = r#"{
      "action":"start","id":42,"level":0,"parent":0,"text":"",
      "type":108,
      "fields":["/nix/store/abc-hello","/nix/store/abc-hello"]
    }"#;
    match parse(json) {
      Actions::Start { activity, .. } => {
        assert_eq!(activity, Activities::Substitute);
      },
      _ => panic!("expected Start"),
    }
  }

  #[test]
  fn start_no_fields_defaults_to_empty() {
    let json = r#"{"action":"start","id":1,"level":4,"parent":0,"text":"evaluating","type":0}"#;
    match parse(json) {
      Actions::Start { fields, .. } => assert!(fields.is_empty()),
      _ => panic!("expected Start"),
    }
  }

  #[test]
  fn stop() {
    match parse(r#"{"action":"stop","id":1234}"#) {
      Actions::Stop { id } => assert_eq!(id, 1234),
      _ => panic!("expected Stop"),
    }
  }

  #[test]
  fn message_nix() {
    let json = r#"{"action":"msg","level":0,"msg":"error: build failed"}"#;
    match parse(json) {
      Actions::Message {
        level,
        msg,
        raw_msg,
        file,
        line,
        column,
      } => {
        assert_eq!(level, Verbosity::Error);
        assert_eq!(msg, "error: build failed");
        assert!(raw_msg.is_none());
        assert!(file.is_none());
        assert!(line.is_none());
        assert!(column.is_none());
      },
      _ => panic!("expected Message"),
    }
  }

  #[test]
  fn message_nix_trace() {
    match parse(r#"{"action":"msg","level":0,"msg":"trace: hello from nix"}"#) {
      Actions::Message { msg, raw_msg, .. } => {
        assert_eq!(msg, "trace: hello from nix");
        assert!(raw_msg.is_none());
      },
      _ => panic!("expected Message"),
    }
  }

  #[test]
  fn message_lix_with_source_location() {
    let json = r#"{
      "action":"msg",
      "level":0,
      "msg":"\u001b[31;1merror:\u001b[0m undefined variable 'foo'",
      "raw_msg":"error: undefined variable 'foo'",
      "file":"/home/user/flake.nix",
      "line":12,
      "column":5
    }"#;
    match parse(json) {
      Actions::Message {
        msg,
        raw_msg,
        file,
        line,
        column,
        ..
      } => {
        assert!(msg.contains("error:"));
        assert_eq!(raw_msg.as_deref(), Some("error: undefined variable 'foo'"));
        assert_eq!(file.as_deref(), Some("/home/user/flake.nix"));
        assert_eq!(line, Some(12));
        assert_eq!(column, Some(5));
      },
      _ => panic!("expected Message"),
    }
  }

  #[test]
  fn message_lix_raw_msg_only() {
    let json = r#"{
      "action":"msg","level":1,
      "msg":"\u001b[33mwarning:\u001b[0m something",
      "raw_msg":"warning: something"
    }"#;
    match parse(json) {
      Actions::Message {
        raw_msg,
        file,
        line,
        ..
      } => {
        assert_eq!(raw_msg.as_deref(), Some("warning: something"));
        assert!(file.is_none());
        assert!(line.is_none());
      },
      _ => panic!("expected Message"),
    }
  }

  #[test]
  fn result_build_log_line() {
    let json = r#"{"action":"result","fields":["checking for gcc... gcc"],"id":99,"type":101}"#;
    match parse(json) {
      Actions::Result {
        result_type,
        fields,
        id,
      } => {
        assert_eq!(result_type, ResultType::BuildLogLine);
        assert_eq!(id, 99);
        assert_eq!(fields[0].as_str().unwrap(), "checking for gcc... gcc");
      },
      _ => panic!("expected Result"),
    }
  }

  #[test]
  fn result_set_phase() {
    match parse(
      r#"{"action":"result","fields":["configurePhase"],"id":5,"type":104}"#,
    ) {
      Actions::Result {
        result_type,
        fields,
        ..
      } => {
        assert_eq!(result_type, ResultType::SetPhase);
        assert_eq!(fields[0].as_str().unwrap(), "configurePhase");
      },
      _ => panic!("expected Result"),
    }
  }

  #[test]
  fn result_progress() {
    match parse(r#"{"action":"result","fields":[3,10,2,0],"id":7,"type":105}"#)
    {
      Actions::Result {
        result_type,
        fields,
        ..
      } => {
        assert_eq!(result_type, ResultType::Progress);
        assert_eq!(fields[0].as_u64(), Some(3)); // done
        assert_eq!(fields[1].as_u64(), Some(10)); // expected
        assert_eq!(fields[2].as_u64(), Some(2)); // running
        assert_eq!(fields[3].as_u64(), Some(0)); // failed
      },
      _ => panic!("expected Result"),
    }
  }

  #[test]
  fn result_set_expected() {
    match parse(r#"{"action":"result","fields":[105,8],"id":3,"type":106}"#) {
      Actions::Result {
        result_type,
        fields,
        ..
      } => {
        assert_eq!(result_type, ResultType::SetExpected);
        assert_eq!(fields[0].as_u64(), Some(105)); // activity_type = Build
        assert_eq!(fields[1].as_u64(), Some(8));
      },
      _ => panic!("expected Result"),
    }
  }

  #[test]
  fn result_post_build_log_line() {
    match parse(
      r#"{"action":"result","fields":["hook output"],"id":1,"type":107}"#,
    ) {
      Actions::Result { result_type, .. } => {
        assert_eq!(result_type, ResultType::PostBuildLogLine);
      },
      _ => panic!("expected Result"),
    }
  }

  #[test]
  fn parse_line_prefix() {
    let line = r#"@nix {"action":"stop","id":42}"#;
    match parse_line(line).unwrap() {
      Actions::Stop { id } => assert_eq!(id, 42),
      _ => panic!("expected Stop"),
    }
  }

  #[test]
  fn decode_distinguishes_unsupported_protocol_from_malformed_records() {
    match decode_action(br#"{"action":"start","type":114}"#).unwrap() {
      DecodedAction::Unsupported(record) => {
        assert_eq!(record.action, "start");
        assert_eq!(record.kind, Some(114));
      },
      DecodedAction::Known(_) => panic!("unknown activity was accepted"),
    }
    assert!(decode_action(br#"{"action":"stop","id":"bad"}"#).is_err());
    assert!(decode_action(b"{not json").is_err());
  }

  #[test]
  fn parse_line_non_nix() {
    assert!(parse_line("some other output").is_none());
    assert!(parse_line("").is_none());
  }
}
