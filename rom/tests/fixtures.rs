//! Visual regression snapshots replayed from timed Nix logs.
//!
//! Each fixture is a directory under `fixtures/`. Every `build.log` line is
//! `<ms> <raw line>`, written to the stream followed by a newline, or
//! `<ms> = <name>`, which snapshots the frame. The finished frame and the
//! decoded log are always snapshotted last, into one `<variant>.snap` per view.

#![expect(
  clippy::tests_outside_test_module,
  reason = "integration tests are their own crate"
)]
#![expect(clippy::panic, reason = "tests fail by panicking")]

use std::{fmt::Write as _, fs, path::Path, time::Duration};

use rom::{
  cache::BuildReportCache,
  display::{format_log, render_frame},
  fixture::FixtureResolver,
  monitor::{Output, StreamEngine},
  types::{
    DisplayFormat,
    EngineConfig,
    IconMode,
    LegendStyle,
    LogPrefixStyle,
    RenderConfig,
    SummaryStyle,
  },
};

struct View {
  width:  u16,
  height: u16,
  render: RenderConfig,
  engine: EngineConfig,
}

/// Replays `fixtures/<fixture>/build.log` into `<variant>.snap` beside it with
/// a 79x23 Unicode tree view, adjusted by `configure`.
fn replay(fixture: &str, variant: &str, configure: impl FnOnce(&mut View)) {
  let mut view = View {
    width:  79,
    height: 23,
    render: RenderConfig {
      icons: IconMode::Unicode,
      ..RenderConfig::default()
    },
    engine: EngineConfig::default(),
  };
  configure(&mut view);
  let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("tests/fixtures")
    .join(fixture);
  let log_path = directory.join("build.log");
  let input = fs::read_to_string(&log_path).unwrap();
  let ansi = RenderConfig {
    ansi: true,
    ..view.render
  };
  let plain = RenderConfig {
    ansi: false,
    ..ansi
  };

  let mut stream = StreamEngine::new(view.engine);
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
      view.width,
      view.height,
      done,
    );
    let styled = frame.ansi_text();
    let unstyled = strip_ansi(&styled);
    let unstyled = unstyled.lines().map(str::trim_end).collect::<Vec<&str>>();
    assert_eq!(
      unstyled.join("\n"),
      frame.text(),
      "{fixture}/{variant}: {name}"
    );
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

  assert_or_bless(&directory.join(format!("{variant}.snap")), &actual);
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

fn assert_or_bless(path: &Path, actual: &str) {
  if std::env::var_os("ROM_BLESS").is_some() {
    fs::write(path, actual).unwrap();
    return;
  }
  let expected = fs::read_to_string(path).unwrap_or_else(|_| {
    panic!(
      "missing snapshot {}; run ROM_BLESS=1 cargo test --test fixtures",
      path.display()
    )
  });
  assert!(
    expected == actual,
    "snapshot mismatch for {}\n{}",
    path.display(),
    simple_diff(&expected, actual),
  );
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

#[test]
fn big_graph() {
  replay("big-graph", "default", |_| {});
}

#[test]
fn deep_chain() {
  replay("deep-chain", "default", |_| {});
}

#[test]
fn wide_fanout() {
  replay("wide-fanout", "default", |_| {});
}

#[test]
fn grow_shrink() {
  replay("grow-shrink", "default", |_| {});
}

#[test]
fn shared_names() {
  replay("shared-names", "default", |_| {});
}

#[test]
fn planned_lifecycle() {
  replay("planned-lifecycle", "default", |_| {});
}

#[test]
fn sub_second_builds() {
  replay("sub-second-builds", "default", |_| {});
}

#[test]
fn phases_and_estimates() {
  replay("phases-and-estimates", "default", |_| {});
}

#[test]
fn failed_build() {
  replay("failed-build", "default", |_| {});
}

#[test]
fn dependency_failure() {
  replay("dependency-failure", "default", |_| {});
}

#[test]
fn multiple_failures() {
  replay("multiple-failures", "default", |_| {});
}

#[test]
fn packages_and_units() {
  replay("packages-and-units", "default", |_| {});
}

#[test]
fn ca_and_dynamic() {
  replay("ca-and-dynamic", "default", |_| {});
}

#[test]
fn downloads() {
  replay("downloads", "default", |_| {});
}

#[test]
fn unsized_download() {
  replay("unsized-download", "default", |_| {});
}

#[test]
fn mixed_transfers() {
  replay("mixed-transfers", "default", |_| {});
}

#[test]
fn substitution_heavy() {
  replay("substitution-heavy", "default", |_| {});
}

#[test]
fn source_fetches() {
  replay("source-fetches", "default", |_| {});
}

#[test]
fn fetch_to_store() {
  replay("fetch-to-store", "default", |_| {});
}

#[test]
fn remote_build() {
  replay("remote-build", "default", |_| {});
}

#[test]
fn remote_mix() {
  replay("remote-mix", "default", |_| {});
}

#[test]
fn chatty_build() {
  replay("chatty-build", "default", |_| {});
}

#[test]
fn hostile_logs() {
  replay("hostile-logs", "default", |_| {});
}

#[test]
fn passthrough() {
  replay("passthrough", "default", |_| {});
}

#[test]
fn unsupported_record() {
  replay("unsupported-record", "default", |_| {});
}

#[test]
fn nix_error() {
  replay("nix-error", "default", |_| {});
}

#[test]
fn long_names() {
  replay("long-names", "default", |_| {});
}

#[test]
fn odd_names() {
  replay("odd-names", "default", |_| {});
}

#[test]
fn overflow() {
  replay("overflow", "default", |_| {});
}

#[test]
fn empty_run() {
  replay("empty-run", "default", |_| {});
}

#[test]
fn instant_finish() {
  replay("instant-finish", "default", |_| {});
}

#[test]
fn interrupted() {
  replay("interrupted", "default", |_| {});
}

#[test]
fn long_run() {
  replay("long-run", "default", |_| {});
}

// Variants replay a fixture above under another view.

#[test]
fn log_line_limit() {
  replay("log-line-limit", "default", |view| {
    view.engine.log_line_limit = Some(2);
  });
}

#[test]
fn fetch_to_store_plain() {
  replay("fetch-to-store", "plain", |view| {
    view.render.format = DisplayFormat::Plain;
  });
}

#[test]
fn big_graph_narrow() {
  replay("big-graph", "narrow", |view| {
    (view.width, view.height) = (40, 12);
  });
}

#[test]
fn big_graph_tall() {
  replay("big-graph", "tall", |view| {
    view.height = 60;
  });
}

#[test]
fn deep_chain_narrow() {
  replay("deep-chain", "narrow", |view| {
    (view.width, view.height) = (40, 12);
  });
}

#[test]
fn overflow_narrow() {
  replay("overflow", "narrow", |view| {
    (view.width, view.height) = (40, 12);
  });
}

#[test]
fn format_dashboard() {
  replay("big-graph", "dashboard", |view| {
    view.render.format = DisplayFormat::Dashboard;
  });
}

#[test]
fn format_plain() {
  replay("big-graph", "plain", |view| {
    view.render.format = DisplayFormat::Plain;
  });
}

#[test]
fn legend_compact() {
  replay("dependency-failure", "legend-compact", |view| {
    view.render.legend_style = LegendStyle::Compact;
  });
}

#[test]
fn legend_verbose() {
  replay("dependency-failure", "legend-verbose", |view| {
    view.render.legend_style = LegendStyle::Verbose;
  });
}

#[test]
fn summary_table() {
  replay("dependency-failure", "summary-table", |view| {
    view.render.summary_style = SummaryStyle::Table;
  });
}

#[test]
fn summary_full() {
  replay("dependency-failure", "summary-full", |view| {
    view.render.summary_style = SummaryStyle::Full;
  });
}

#[test]
fn icons_nerd() {
  replay("mixed-transfers", "nerd-icons", |view| {
    view.render.icons = IconMode::Nerd;
  });
}

#[test]
fn silent() {
  replay("chatty-build", "silent", |view| {
    view.engine.silent = true;
  });
}

#[test]
fn log_prefix_none() {
  replay("chatty-build", "no-log-prefix", |view| {
    view.engine.log_prefix_style = LogPrefixStyle::None;
  });
}
