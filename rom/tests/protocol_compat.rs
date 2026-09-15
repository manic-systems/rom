use rom::{
  monitor::{Engine, Output},
  types::{EngineConfig, InputMode},
};

const FETCH_START: &str = r#"{"action":"start","id":2,"level":5,"parent":0,"text":"hashing '/tmp/source'","type":113,"fields":["/tmp/source",1]}"#;
const FETCH_RESULT: &str = r#"{"action":"result","id":2,"type":109,"fields":["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-source"]}"#;

fn record(json: &str, mode: InputMode) -> String {
  match mode {
    InputMode::Auto => format!("@nix {json}"),
    InputMode::Json => json.to_string(),
  }
}

fn output(engine: &mut Engine, input: &str) -> Vec<Output> {
  engine
    .process_record_at(input.as_bytes(), 1.0)
    .unwrap()
    .output
}

#[test]
fn fetch_to_store_records_are_supported_in_both_input_modes() {
  for mode in [InputMode::Auto, InputMode::Json] {
    let mut engine = Engine::new(EngineConfig {
      input_mode: mode,
      ..EngineConfig::default()
    });
    assert!(output(&mut engine, &record(FETCH_START, mode)).is_empty());
    assert!(output(&mut engine, &record(FETCH_RESULT, mode)).is_empty());
    assert!(
      output(&mut engine, &record(r#"{"action":"stop","id":2}"#, mode))
        .is_empty()
    );
    assert!(!engine.state().has_errors());
  }
}

#[test]
fn repeated_unsupported_records_report_once() {
  let mut engine = Engine::new(EngineConfig::default());
  let record = r#"@nix {"action":"start","id":2,"level":3,"parent":0,"text":"future","type":114,"fields":[]}"#;
  let first = output(&mut engine, record);
  let second = output(&mut engine, record);
  let [Output::Log(log)] = &first[..] else {
    panic!("missing diagnostic: {first:?}")
  };
  assert_eq!(
    log.plain,
    "rom: ignored unsupported internal-JSON start type 114"
  );
  assert!(second.is_empty());
}

#[test]
fn malformed_records_still_fail() {
  let mut engine = Engine::new(EngineConfig::default());
  assert!(engine.process_record_at(b"@nix {not json", 1.0).is_err());
}
