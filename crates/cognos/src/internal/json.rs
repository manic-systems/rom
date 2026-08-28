use serde::Deserialize;
use serde_repr::Deserialize_repr;

/// Activity types used in `start` actions.
#[derive(Deserialize_repr, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Activities {
  Unknown       = 0,
  CopyPath      = 100,
  FileTransfer  = 101,
  Realise       = 102,
  CopyPaths     = 103,
  Builds        = 104,
  Build         = 105,
  OptimiseStore = 106,
  VerifyPath    = 107,
  Substitute    = 108,
  QueryPathInfo = 109,
  PostBuildHook = 110,
  BuildWaiting  = 111,
  FetchTree     = 112,
}

/// Result types used in `result` actions. Numerically overlap with
/// `Activities` but carry entirely different semantics; do not conflate.
#[derive(Deserialize_repr, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResultType {
  /// Two ints: (`linked_count`, `total_count`)
  FileLinked       = 100,
  /// One string: a log line emitted by the builder
  BuildLogLine     = 101,
  /// One string: store path that is not trusted
  UntrustedPath    = 102,
  /// One string: store path that is corrupted
  CorruptedPath    = 103,
  /// One string: current build phase name (e.g. "configurePhase")
  SetPhase         = 104,
  /// Four ints: (done, expected, running, failed)
  Progress         = 105,
  /// Two ints: (`activity_type`, `expected_count`)
  SetExpected      = 106,
  /// One string: a log line from a post-build hook
  PostBuildLogLine = 107,
  /// One string: fetch status message
  FetchStatus      = 108,
}

#[derive(
  Deserialize_repr, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord,
)]
#[repr(u8)]
pub enum Verbosity {
  Error     = 0,
  Warning   = 1,
  Notice    = 2,
  Info      = 3,
  Talkative = 4,
  Chatty    = 5,
  Debug     = 6,
  Vomit     = 7,
}

pub type Id = u64;

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "action")]
pub enum Actions {
  #[serde(rename = "start")]
  Start {
    id:       Id,
    level:    Verbosity,
    #[serde(default)]
    parent:   Id,
    text:     String,
    #[serde(rename = "type")]
    activity: Activities,
    #[serde(default)]
    fields:   Vec<serde_json::Value>,
  },

  #[serde(rename = "stop")]
  Stop { id: Id },

  /// A log/diagnostic message.
  ///
  /// Lix extends this with optional source-location fields (`file`, `line`,
  /// `column`) and `raw_msg` (the message text without ANSI escape sequences).
  /// Nix omits these fields entirely; serde defaults them to `None` so the
  /// same struct parses both.
  #[serde(rename = "msg")]
  Message {
    level:   Verbosity,
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

  #[serde(rename = "result")]
  Result {
    #[serde(default)]
    fields:      Vec<serde_json::Value>,
    id:          Id,
    #[serde(rename = "type")]
    result_type: ResultType,
  },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedRecord {
  pub action: String,
  pub kind:   Option<u64>,
}

#[derive(Debug, Clone)]
pub enum DecodedAction {
  Known(Actions),
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
pub fn decode_action(json: &[u8]) -> Result<DecodedAction, serde_json::Error> {
  let value: serde_json::Value = serde_json::from_slice(json)?;
  let envelope: Envelope = serde_json::from_value(value.clone())?;
  let kind = envelope.kind.as_ref().and_then(serde_json::Value::as_u64);
  let level = envelope.level.as_ref().and_then(serde_json::Value::as_u64);
  let unsupported = match envelope.action.as_str() {
    "start" => {
      kind.is_some_and(|kind| !matches!(kind, 0 | 100..=112))
        || level.is_some_and(|level| level > 7)
    },
    "msg" => level.is_some_and(|level| level > 7),
    "result" => kind.is_some_and(|kind| !matches!(kind, 100..=108)),
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
  fn test_start_build_nix() {
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
  fn test_start_substitute() {
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
  fn test_start_no_fields_defaults_to_empty() {
    let json = r#"{"action":"start","id":1,"level":4,"parent":0,"text":"evaluating","type":0}"#;
    match parse(json) {
      Actions::Start { fields, .. } => assert!(fields.is_empty()),
      _ => panic!("expected Start"),
    }
  }

  #[test]
  fn test_stop() {
    match parse(r#"{"action":"stop","id":1234}"#) {
      Actions::Stop { id } => assert_eq!(id, 1234),
      _ => panic!("expected Stop"),
    }
  }

  #[test]
  fn test_message_nix() {
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
  fn test_message_nix_trace() {
    match parse(r#"{"action":"msg","level":0,"msg":"trace: hello from nix"}"#) {
      Actions::Message { msg, raw_msg, .. } => {
        assert_eq!(msg, "trace: hello from nix");
        assert!(raw_msg.is_none());
      },
      _ => panic!("expected Message"),
    }
  }

  #[test]
  fn test_message_lix_with_source_location() {
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
  fn test_message_lix_raw_msg_only() {
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
  fn test_result_build_log_line() {
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
  fn test_result_set_phase() {
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
  fn test_result_progress() {
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
  fn test_result_set_expected() {
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
  fn test_result_post_build_log_line() {
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
  fn test_parse_line_prefix() {
    let line = r#"@nix {"action":"stop","id":42}"#;
    match parse_line(line).unwrap() {
      Actions::Stop { id } => assert_eq!(id, 42),
      _ => panic!("expected Stop"),
    }
  }

  #[test]
  fn decode_distinguishes_unsupported_protocol_from_malformed_records() {
    match decode_action(br#"{"action":"start","type":113}"#).unwrap() {
      DecodedAction::Unsupported(record) => {
        assert_eq!(record.action, "start");
        assert_eq!(record.kind, Some(113));
      },
      DecodedAction::Known(_) => panic!("unknown activity was accepted"),
    }
    assert!(decode_action(br#"{"action":"stop","id":"bad"}"#).is_err());
    assert!(decode_action(b"{not json").is_err());
  }

  #[test]
  fn test_parse_line_non_nix() {
    assert!(parse_line("some other output").is_none());
    assert!(parse_line("").is_none());
  }
}
