use rom::{
  monitor::{Engine, Output, StreamEngine},
  state::BuildStatus,
  types::{EngineConfig, InputMode},
};

const START: &str = r#"{"action":"start","id":1,"level":3,"parent":0,"text":"building","type":105,"fields":["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-demo.drv","",1,1]}"#;
const STOP: &str = r#"{"action":"stop","id":1}"#;
const UNKNOWN: &str = r#"{"action":"start","id":2,"level":3,"parent":0,"text":"fetching","type":113,"fields":[]}"#;

fn record(json: &str, mode: InputMode) -> String {
  match mode {
    InputMode::Auto => format!("@nix {json}"),
    InputMode::Json => json.to_string(),
  }
}

fn diagnostic(engine: &mut Engine, input: &str) -> String {
  let output = engine
    .process_record_at(input.as_bytes(), 1.0)
    .unwrap()
    .output;
  let [Output::Log(log)] = &output[..] else {
    panic!("missing diagnostic: {output:?}")
  };
  log.plain.clone()
}

#[test]
fn protocol_diagnostics_preserve_state_and_recovery_in_both_input_modes() {
  for mode in [InputMode::Auto, InputMode::Json] {
    let mut engine = Engine::new(EngineConfig {
      input_mode: mode,
      ..EngineConfig::default()
    });
    engine
      .process_record_at(record(START, mode).as_bytes(), 0.0)
      .unwrap();

    assert!(
      diagnostic(&mut engine, &record(UNKNOWN, mode))
        .starts_with("rom: ignored unsupported internal-JSON start type 113")
    );
    assert_eq!(
      diagnostic(
        &mut engine,
        &record(r#"{"action":"result","id":1,"type":109,"fields":[]}"#, mode),
      ),
      "rom: ignored unsupported internal-JSON result type 109"
    );
    assert!(
      engine
        .process_record_at(record("{not json", mode).as_bytes(), 1.0)
        .is_err()
    );
    assert_eq!(engine.state().derivations().len(), 1);
    assert!(!engine.state().has_errors());

    engine
      .process_record_at(record(STOP, mode).as_bytes(), 2.0)
      .unwrap();
    assert!(
      engine
        .state()
        .derivations()
        .values()
        .all(|info| { matches!(info.build_status, BuildStatus::Built { .. }) })
    );
  }
}

#[test]
fn stream_preserves_output_order_around_unsupported_records() {
  let input = format!(
    "before\n@nix {START}\n@nix {UNKNOWN}\n@nix \
     {{\"action\":\"msg\",\"level\":3,\"msg\":\"later-log\"}}\nafter\n@nix \
     {STOP}"
  );
  let mut stream = StreamEngine::new(EngineConfig::default());
  let mut text = String::new();
  for item in stream.push_at(input.as_bytes(), 0.0).unwrap().output {
    match item {
      Output::Passthrough(bytes) => {
        text.push_str(&String::from_utf8(bytes).unwrap())
      },
      Output::Log(log) => {
        text.push_str(&log.plain);
        text.push('\n');
      },
    }
  }
  assert!(text.starts_with(
    "before\nrom: ignored unsupported internal-JSON start type 113\n"
  ));
  assert!(text.ends_with("later-log\nafter\n"));
}

#[test]
fn diagnostics_remain_visible_in_silent_mode_without_terminal_controls() {
  let mut engine = Engine::new(EngineConfig {
    silent: true,
    log_line_limit: Some(0),
    ..EngineConfig::default()
  });
  let message =
    diagnostic(&mut engine, r#"@nix {"action":"future\u001b[2J\n\roops"}"#);
  assert!(message.starts_with("rom: ignored unsupported"));
  assert!(!message.chars().any(char::is_control));
  assert!(!engine.state().has_errors());
}
