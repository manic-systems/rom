//! Handling of protocol extensions and malformed records.

#![expect(
  clippy::tests_outside_test_module,
  reason = "integration tests are their own crate"
)]
#![expect(clippy::panic, reason = "tests fail by panicking")]

use rom::{
  monitor::{Engine, Output},
  types::{EngineConfig, InputMode},
};

const FETCH_START: &str = r#"{"action":"start","id":2,"level":5,"parent":0,"text":"hashing '/tmp/source'","type":113,"fields":["/tmp/source",1]}"#;
const FETCH_RESULT: &str = r#"{"action":"result","id":2,"type":109,"fields":["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-source"]}"#;

fn record(json: &str, mode: InputMode) -> String {
  match mode {
    InputMode::Auto => format!("@nix {json}"),
    InputMode::Json => json.to_owned(),
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
    assert_eq!(
      output(&mut engine, &record(FETCH_START, mode)),
      Vec::<Output>::new()
    );
    assert_eq!(
      output(&mut engine, &record(FETCH_RESULT, mode)),
      Vec::<Output>::new()
    );
    assert_eq!(
      output(&mut engine, &record(r#"{"action":"stop","id":2}"#, mode)),
      Vec::<Output>::new()
    );
    assert!(!engine.state().has_errors());
  }
}

#[test]
fn determinate_extension_records_are_silent() {
  let mut engine = Engine::new(EngineConfig::default());
  let start = r#"@nix {"action":"start","id":2,"level":4,"name":"EvaluateFlakeDerivationOutput","parent":0,"payload":{"nix.installable":"flake:nixpkgs#hello"},"sid":"01a1f003-6c5a-7226-80a1-99d35dfea0f0","text":"evaluating derivation 'flake:nixpkgs#hello'","type":10113}"#;
  let result = r#"@nix {"action":"result","id":3,"payload":{"bodySize":21,"httpStatus":200},"sid":"01a1f003-6c5a-7226-80a1-99d35dfea0f0","type":10111}"#;
  assert_eq!(output(&mut engine, start), Vec::<Output>::new());
  assert_eq!(output(&mut engine, result), Vec::<Output>::new());
  assert!(!engine.state().has_errors());
}

#[test]
fn repeated_unsupported_records_report_once() {
  let mut engine = Engine::new(EngineConfig::default());
  let record = r#"@nix {"action":"start","id":2,"level":3,"parent":0,"text":"future","type":114,"fields":[]}"#;
  let first = output(&mut engine, record);
  let second = output(&mut engine, record);
  let [Output::Log(log)] = &*first else {
    panic!("missing diagnostic: {first:?}")
  };
  assert_eq!(
    log.plain,
    "rom: ignored unsupported internal-JSON start type 114"
  );
  assert_eq!(second, Vec::<Output>::new());
}

#[test]
fn malformed_records_still_fail() {
  let mut engine = Engine::new(EngineConfig::default());
  engine
    .process_record_at(b"@nix {not json", 1.0)
    .unwrap_err();
}
