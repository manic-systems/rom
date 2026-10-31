//! Visual regression snapshots replayed from timed Nix logs.
//!
//! Each fixture is a directory under `fixtures/`. Every `build.log` line is
//! `<ms> <raw line>`, written to the stream followed by a newline, or
//! `<ms> = <name>`, which snapshots the frame. The finished frame and the
//! decoded log are always snapshotted last.
//!
//! A fixture replays once into `default.snap`, plus once into `<name>.snap`
//! for every table in its `variants.toml`. A table holds `config.toml`
//! settings, along with `terminal = { width, height }` and `silent`, which
//! are not settings; a `[default]` table changes the plain run.

#![expect(
  clippy::tests_outside_test_module,
  reason = "integration tests are their own crate"
)]
#![expect(clippy::panic, reason = "tests fail by panicking")]

use std::{fmt::Write as _, fs, path::Path, time::Duration};

use rom::{
  cache::BuildReportCache,
  cli::DisplayOptions,
  display::{format_log, render_frame},
  fixture::FixtureResolver,
  monitor::{Output, StreamEngine},
  types::{Config, IconMode, RenderConfig},
};
use serde::Deserialize;

/// A fixture variant's settings that are not ROM's to read.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Terminal {
  width:  Option<u16>,
  height: Option<u16>,
}

#[test]
fn fixtures() {
  let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
  let mut fixtures = fs::read_dir(&root)
    .unwrap()
    .map(|entry| entry.unwrap().path())
    .filter(|path| path.is_dir())
    .collect::<Vec<_>>();
  fixtures.sort();
  let failures = fixtures
    .iter()
    .flat_map(|directory| replay_variants(directory))
    .collect::<Vec<String>>();
  assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Replays every variant of the fixture in `directory`, returning failures.
fn replay_variants(directory: &Path) -> Vec<String> {
  let variants_path = directory.join("variants.toml");
  let mut variants = if variants_path.exists() {
    toml::from_str::<toml::Table>(&fs::read_to_string(&variants_path).unwrap())
      .unwrap_or_else(|error| panic!("{}: {error}", variants_path.display()))
  } else {
    toml::Table::new()
  };
  variants
    .entry("default")
    .or_insert_with(|| toml::Value::Table(toml::Table::new()));

  let mut failures = Vec::new();
  for (name, settings) in &variants {
    let toml::Value::Table(settings) = settings else {
      panic!("{}: [{name}] must be a table", variants_path.display());
    };
    let actual = replay(directory, name, settings.clone());
    let path = directory.join(format!("{name}.snap"));
    if blessing() {
      fs::write(&path, actual).unwrap();
    } else if let Err(failure) = compare(&path, &actual) {
      failures.push(failure);
    }
  }
  // Snapshots of removed variants would otherwise linger unchecked.
  for entry in fs::read_dir(directory).unwrap() {
    let path = entry.unwrap().path();
    let stale = path
      .extension()
      .is_some_and(|extension| extension == "snap")
      && path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_none_or(|stem| !variants.contains_key(stem));
    if stale {
      if blessing() {
        fs::remove_file(&path).unwrap();
      } else {
        failures.push(format!(
          "stale snapshot {}; run ROM_BLESS=1 cargo test --test fixtures",
          path.display()
        ));
      }
    }
  }
  failures
}

/// Replays a fixture's `build.log` into a snapshot, with a 79x23 Unicode tree
/// view changed by `settings`.
fn replay(
  directory: &Path,
  variant: &str,
  mut settings: toml::Table,
) -> String {
  let fixture = directory.file_name().unwrap().to_string_lossy();
  let fixture = format!("{fixture}/{variant}");
  let terminal =
    settings
      .remove("terminal")
      .map_or_else(Terminal::default, |terminal| {
        terminal
          .try_into()
          .unwrap_or_else(|error| panic!("{fixture}: terminal: {error}"))
      });
  let silent = settings.remove("silent").is_some_and(|silent| {
    silent
      .as_bool()
      .unwrap_or_else(|| panic!("{fixture}: silent must be a boolean"))
  });
  let mut config = Config {
    render: RenderConfig {
      icons: IconMode::Unicode,
      ..RenderConfig::default()
    },
    ..Config::default()
  };
  config.engine.silent = silent;
  toml::Value::Table(settings)
    .try_into::<DisplayOptions>()
    .unwrap_or_else(|error| panic!("{fixture}: {error}"))
    .apply(&mut config);
  let (width, height) =
    (terminal.width.unwrap_or(79), terminal.height.unwrap_or(23));

  let log_path = directory.join("build.log");
  let input = fs::read_to_string(&log_path).unwrap();
  let ansi = RenderConfig {
    ansi: true,
    ..config.render
  };
  let plain = RenderConfig {
    ansi: false,
    ..ansi
  };

  let mut stream = StreamEngine::new(config.engine);
  let derivations = directory.join("derivations.json");
  if derivations.exists() {
    stream
      .engine_mut()
      .set_resolver(FixtureResolver::load(&derivations).unwrap());
  }
  let history = directory.join("history.csv");
  if history.exists() {
    stream
      .engine_mut()
      .load_history(&BuildReportCache::new(history));
  }
  let mut output = Vec::new();
  let mut actual = String::new();
  let mut snap = |stream: &StreamEngine, name: &str, at_ms: u64, done: bool| {
    let frame = render_frame(
      stream.engine().state(),
      &ansi,
      Duration::from_millis(at_ms).as_secs_f64(),
      width,
      height,
      done,
    );
    let styled = frame.ansi_text();
    let unstyled = strip_ansi(&styled);
    let unstyled = unstyled.lines().map(str::trim_end).collect::<Vec<&str>>();
    assert_eq!(unstyled.join("\n"), frame.text(), "{fixture}: {name}");
    writeln!(actual, "=== {name} @ {at_ms}ms ===").unwrap();
    actual.push_str(&visible_escapes(&styled));
    actual.push('\n');
  };

  let mut at_ms = 0;
  for (index, line) in input.lines().enumerate() {
    let (time, rest) = line.split_once(' ').unwrap_or_else(|| {
      panic!(
        "{}:{}: expected `<ms> <line>`",
        log_path.display(),
        index + 1
      )
    });
    let time = time.parse::<u64>().unwrap();
    assert!(time >= at_ms, "{fixture}: timestamps must be monotonic");
    at_ms = time;
    let now = Duration::from_millis(at_ms).as_secs_f64();
    if let Some(name) = rest.strip_prefix("= ") {
      // The CLI polls while idle, so answers to lookups asked during the
      // last poll have arrived by the time a frame is drawn.
      while stream.engine_mut().poll_lookups(now) {}
      stream.engine_mut().poll_lookups(now);
      snap(&stream, name, at_ms, false);
    } else {
      let processed =
        stream.push_at(format!("{rest}\n").as_bytes(), now).unwrap();
      output.extend(processed.output);
      stream.engine_mut().poll_lookups(now);
    }
  }
  output.extend(
    stream
      .finish_at(Duration::from_millis(at_ms).as_secs_f64())
      .unwrap()
      .output,
  );
  snap(&stream, "finished", at_ms, true);

  let styled_log = render_output(&output, &ansi);
  assert_eq!(
    strip_ansi(&styled_log),
    render_output(&output, &plain),
    "styled and plain logs differ only by ANSI styling"
  );
  actual.push_str("=== log ===\n");
  actual.push_str(&visible_escapes(&styled_log));

  actual
}

fn render_output(output: &[Output], render: &RenderConfig) -> String {
  let mut bytes = Vec::new();
  for output in output {
    match output {
      Output::Passthrough(passthrough) => bytes.extend(passthrough),
      Output::Log(line) => {
        bytes.extend(format_log(line, render).as_bytes());
        bytes.push(b'\n');
      },
    }
  }
  String::from_utf8(bytes).unwrap()
}

fn strip_ansi(value: &str) -> String {
  let mut result = String::new();
  let mut chars = value.chars();
  while let Some(ch) = chars.next() {
    if ch == '\x1b' {
      chars.by_ref().find(|&ch| ch == 'm');
    } else {
      result.push(ch);
    }
  }
  result
}

fn visible_escapes(value: &str) -> String {
  value.replace('\x1b', "\\e")
}

fn blessing() -> bool {
  std::env::var_os("ROM_BLESS").is_some()
}

fn compare(path: &Path, actual: &str) -> Result<(), String> {
  let Ok(expected) = fs::read_to_string(path) else {
    return Err(format!(
      "missing snapshot {}; run ROM_BLESS=1 cargo test --test fixtures",
      path.display()
    ));
  };
  if expected == actual {
    return Ok(());
  }
  Err(format!(
    "snapshot mismatch for {}\n{}",
    path.display(),
    simple_diff(&expected, actual),
  ))
}

fn simple_diff(expected: &str, actual: &str) -> String {
  let expected = expected.lines().collect::<Vec<&str>>();
  let actual = actual.lines().collect::<Vec<&str>>();
  let mut diff = String::from("--- expected\n+++ actual\n");
  for index in 0..expected.len().max(actual.len()) {
    match (expected.get(index), actual.get(index)) {
      (Some(left), Some(right)) if left == right => {
        writeln!(diff, " {left}").unwrap();
      },
      (left, right) => {
        if let Some(left) = left {
          writeln!(diff, "-{left}").unwrap();
        }
        if let Some(right) = right {
          writeln!(diff, "+{right}").unwrap();
        }
      },
    }
  }
  diff
}
