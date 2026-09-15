//! Visual regression snapshots replayed from timed Nix logs.
//!
//! Each `fixtures/<log>.log` line is `<ms> <raw line>`, written to the stream
//! followed by a newline, or `<ms> = <name>`, which snapshots the frame. The
//! finished frame and the decoded log are always snapshotted last.

use std::{fs, path::Path};

use rom::{
  display::{format_log, render_frame},
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

/// Replay `fixtures/<log>.log` into `fixtures/<snapshot>.snap` with a 79x23
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
    ..ansi.clone()
  };

  let mut stream = StreamEngine::new(view.engine);
  let mut output = Vec::new();
  let mut actual = String::new();
  let mut snap = |stream: &StreamEngine, name: &str, at_ms: u64, done: bool| {
    let frame = render_frame(
      stream.engine().state(),
      &ansi,
      at_ms as f64 / 1000.0,
      view.width,
      view.height,
      done,
    );
    let styled = frame.ansi_text();
    let unstyled = strip_ansi(&styled);
    let unstyled: Vec<_> = unstyled.lines().map(str::trim_end).collect();
    assert_eq!(unstyled.join("\n"), frame.text(), "{snapshot}: {name}");
    actual.push_str(&format!("=== {name} @ {at_ms}ms ===\n"));
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
    let time: u64 = time.parse().unwrap();
    assert!(time >= at_ms, "{log}: timestamps must be monotonic");
    at_ms = time;
    match rest.strip_prefix("= ") {
      Some(name) => snap(&stream, name, at_ms, false),
      None => {
        let processed = stream
          .push_at(format!("{rest}\n").as_bytes(), at_ms as f64 / 1000.0)
          .unwrap();
        output.extend(processed.output);
      },
    }
  }
  output.extend(stream.finish_at(at_ms as f64 / 1000.0).unwrap().output);
  snap(&stream, "finished", at_ms, true);

  let styled_log = render_output(&output, &ansi);
  assert_eq!(strip_ansi(&styled_log), render_output(&output, &plain));
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
  if expected != actual {
    panic!(
      "snapshot mismatch for {}\n{}",
      path.display(),
      simple_diff(&expected, actual),
    );
  }
}

fn simple_diff(expected: &str, actual: &str) -> String {
  let expected: Vec<_> = expected.lines().collect();
  let actual: Vec<_> = actual.lines().collect();
  let mut diff = String::from("--- expected\n+++ actual\n");
  for index in 0..expected.len().max(actual.len()) {
    match (expected.get(index), actual.get(index)) {
      (Some(left), Some(right)) if left == right => {
        diff.push_str(&format!(" {left}\n"));
      },
      (left, right) => {
        if let Some(left) = left {
          diff.push_str(&format!("-{left}\n"));
        }
        if let Some(right) = right {
          diff.push_str(&format!("+{right}\n"));
        }
      },
    }
  }
  diff
}

// Build lifecycles.
#[test]
fn download_progress() {
  replay("download-progress", "download-progress", |_| {});
}

#[test]
fn failed_build() {
  replay("failed-build", "failed-build", |_| {});
}

#[test]
fn planned_builds() {
  replay("planned-builds", "planned-builds", |_| {});
}

#[test]
fn remote_build() {
  replay("remote-build", "remote-build", |_| {});
}

#[test]
fn long_names() {
  replay("long-names", "long-names", |_| {});
}

// How a run ends.
#[test]
fn nix_error() {
  replay("nix-error", "nix-error", |_| {});
}

#[test]
fn unfinished() {
  replay("unfinished", "unfinished", |_| {});
}

#[test]
fn planned_leftover() {
  replay("planned-leftover", "planned-leftover", |_| {});
}

// Transfers.
#[test]
fn concurrent_downloads() {
  replay("concurrent-downloads", "concurrent-downloads", |_| {});
}

#[test]
fn unsized_download() {
  replay("unsized-download", "unsized-download", |_| {});
}

#[test]
fn upload() {
  replay("upload", "upload", |_| {});
}

#[test]
fn fetch_to_store() {
  replay("fetch-to-store", "fetch-to-store", |_| {});
}

#[test]
fn fetch_to_store_plain() {
  replay("fetch-to-store-plain", "fetch-to-store", |view| {
    view.render.format = DisplayFormat::Plain;
  });
}

// Input handling and log policy.
#[test]
fn passthrough() {
  replay("passthrough", "passthrough", |_| {});
}

#[test]
fn unsupported_record() {
  replay("unsupported-record", "unsupported-record", |_| {});
}

#[test]
fn log_line_limit() {
  replay("log-line-limit", "log-line-limit", |view| {
    view.engine.log_line_limit = Some(2)
  });
}

#[test]
fn silent() {
  replay("silent", "failed-build", |view| view.engine.silent = true);
}

#[test]
fn log_prefix_none() {
  replay("log-prefix-none", "download-progress", |view| {
    view.engine.log_prefix_style = LogPrefixStyle::None
  });
}

// Layout and presentation options.
#[test]
fn overflow() {
  replay("overflow", "overflow", |_| {});
}

#[test]
fn overflow_narrow() {
  replay("overflow-narrow", "overflow", |view| {
    (view.width, view.height) = (40, 12)
  });
}

#[test]
fn format_dashboard() {
  replay("format-dashboard", "overflow", |view| {
    view.render.format = DisplayFormat::Dashboard
  });
}

#[test]
fn format_plain() {
  replay("format-plain", "overflow", |view| {
    view.render.format = DisplayFormat::Plain
  });
}

#[test]
fn legend_compact() {
  replay("legend-compact", "failed-build", |view| {
    view.render.legend_style = LegendStyle::Compact
  });
}

#[test]
fn legend_verbose() {
  replay("legend-verbose", "failed-build", |view| {
    view.render.legend_style = LegendStyle::Verbose
  });
}

#[test]
fn summary_table() {
  replay("summary-table", "failed-build", |view| {
    view.render.summary_style = SummaryStyle::Table
  });
}

#[test]
fn summary_full() {
  replay("summary-full", "failed-build", |view| {
    view.render.summary_style = SummaryStyle::Full
  });
}

#[test]
fn icons_nerd() {
  replay("icons-nerd", "concurrent-downloads", |view| {
    view.render.icons = IconMode::Nerd
  });
}
