//! Visual regression snapshots replayed from timed Nix logs.
//!
//! Each `fixtures/<log>.log` line is `<ms> <raw line>`, written to the stream
//! followed by a newline, or `<ms> = <name>`, which snapshots the frame. The
//! finished frame and the decoded log are always snapshotted last.

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

/// Replays `fixtures/<log>.log` into `fixtures/<snapshot>.snap` with a 79x23
/// Unicode tree view, adjusted by `configure`.
fn replay(snapshot: &str, log: &str, configure: impl FnOnce(&mut View)) {
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
  let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
  let log_path = fixtures.join(format!("{log}.log"));
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
  let derivations = fixtures.join(format!("{log}.drvs.json"));
  if derivations.exists() {
    stream
      .engine_mut()
      .set_resolver(FixtureResolver::load(&derivations).unwrap());
  }
  let history = fixtures.join(format!("{log}.history.csv"));
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
    assert_eq!(unstyled.join("\n"), frame.text(), "{snapshot}: {name}");
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
    assert!(time >= at_ms, "{log}: timestamps must be monotonic");
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

  assert_or_bless(&fixtures.join(format!("{snapshot}.snap")), &actual);
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
  replay("big-graph", "big-graph", |_| {});
}

#[test]
fn deep_chain() {
  replay("deep-chain", "deep-chain", |_| {});
}

#[test]
fn wide_fanout() {
  replay("wide-fanout", "wide-fanout", |_| {});
}

#[test]
fn grow_shrink() {
  replay("grow-shrink", "grow-shrink", |_| {});
}

#[test]
fn shared_names() {
  replay("shared-names", "shared-names", |_| {});
}

#[test]
fn planned_lifecycle() {
  replay("planned-lifecycle", "planned-lifecycle", |_| {});
}

#[test]
fn sub_second_builds() {
  replay("sub-second-builds", "sub-second-builds", |_| {});
}

#[test]
fn phases_and_estimates() {
  replay("phases-and-estimates", "phases-and-estimates", |_| {});
}

#[test]
fn failed_build() {
  replay("failed-build", "failed-build", |_| {});
}

#[test]
fn dependency_failure() {
  replay("dependency-failure", "dependency-failure", |_| {});
}

#[test]
fn multiple_failures() {
  replay("multiple-failures", "multiple-failures", |_| {});
}

#[test]
fn packages_and_units() {
  replay("packages-and-units", "packages-and-units", |_| {});
}

#[test]
fn ca_and_dynamic() {
  replay("ca-and-dynamic", "ca-and-dynamic", |_| {});
}

#[test]
fn downloads() {
  replay("downloads", "downloads", |_| {});
}

#[test]
fn unsized_download() {
  replay("unsized-download", "unsized-download", |_| {});
}

#[test]
fn mixed_transfers() {
  replay("mixed-transfers", "mixed-transfers", |_| {});
}

#[test]
fn substitution_heavy() {
  replay("substitution-heavy", "substitution-heavy", |_| {});
}

#[test]
fn source_fetches() {
  replay("source-fetches", "source-fetches", |_| {});
}

#[test]
fn fetch_to_store() {
  replay("fetch-to-store", "fetch-to-store", |_| {});
}

#[test]
fn remote_build() {
  replay("remote-build", "remote-build", |_| {});
}

#[test]
fn remote_mix() {
  replay("remote-mix", "remote-mix", |_| {});
}

#[test]
fn chatty_build() {
  replay("chatty-build", "chatty-build", |_| {});
}

#[test]
fn hostile_logs() {
  replay("hostile-logs", "hostile-logs", |_| {});
}

#[test]
fn passthrough() {
  replay("passthrough", "passthrough", |_| {});
}

#[test]
fn unsupported_record() {
  replay("unsupported-record", "unsupported-record", |_| {});
}

#[test]
fn nix_error() {
  replay("nix-error", "nix-error", |_| {});
}

#[test]
fn long_names() {
  replay("long-names", "long-names", |_| {});
}

#[test]
fn odd_names() {
  replay("odd-names", "odd-names", |_| {});
}

#[test]
fn overflow() {
  replay("overflow", "overflow", |_| {});
}

#[test]
fn empty_run() {
  replay("empty-run", "empty-run", |_| {});
}

#[test]
fn instant_finish() {
  replay("instant-finish", "instant-finish", |_| {});
}

#[test]
fn interrupted() {
  replay("interrupted", "interrupted", |_| {});
}

#[test]
fn long_run() {
  replay("long-run", "long-run", |_| {});
}

// Variants replay a fixture above under another view.

#[test]
fn log_line_limit() {
  replay("log-line-limit", "log-line-limit", |view| {
    view.engine.log_line_limit = Some(2);
  });
}

#[test]
fn fetch_to_store_plain() {
  replay("fetch-to-store-plain", "fetch-to-store", |view| {
    view.render.format = DisplayFormat::Plain;
  });
}

#[test]
fn big_graph_narrow() {
  replay("big-graph-narrow", "big-graph", |view| {
    (view.width, view.height) = (40, 12);
  });
}

#[test]
fn big_graph_tall() {
  replay("big-graph-tall", "big-graph", |view| {
    view.height = 60;
  });
}

#[test]
fn deep_chain_narrow() {
  replay("deep-chain-narrow", "deep-chain", |view| {
    (view.width, view.height) = (40, 12);
  });
}

#[test]
fn overflow_narrow() {
  replay("overflow-narrow", "overflow", |view| {
    (view.width, view.height) = (40, 12);
  });
}

#[test]
fn format_dashboard() {
  replay("format-dashboard", "big-graph", |view| {
    view.render.format = DisplayFormat::Dashboard;
  });
}

#[test]
fn format_plain() {
  replay("format-plain", "big-graph", |view| {
    view.render.format = DisplayFormat::Plain;
  });
}

#[test]
fn legend_compact() {
  replay("legend-compact", "dependency-failure", |view| {
    view.render.legend_style = LegendStyle::Compact;
  });
}

#[test]
fn legend_verbose() {
  replay("legend-verbose", "dependency-failure", |view| {
    view.render.legend_style = LegendStyle::Verbose;
  });
}

#[test]
fn summary_table() {
  replay("summary-table", "dependency-failure", |view| {
    view.render.summary_style = SummaryStyle::Table;
  });
}

#[test]
fn summary_full() {
  replay("summary-full", "dependency-failure", |view| {
    view.render.summary_style = SummaryStyle::Full;
  });
}

#[test]
fn icons_nerd() {
  replay("icons-nerd", "mixed-transfers", |view| {
    view.render.icons = IconMode::Nerd;
  });
}

#[test]
fn silent() {
  replay("silent", "chatty-build", |view| {
    view.engine.silent = true;
  });
}

#[test]
fn log_prefix_none() {
  replay("log-prefix-none", "chatty-build", |view| {
    view.engine.log_prefix_style = LogPrefixStyle::None;
  });
}
