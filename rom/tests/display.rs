//! Rendering of the tree, plain, and dashboard presentations.

#![expect(
  clippy::tests_outside_test_module,
  reason = "integration tests are their own crate"
)]

use std::{collections::BTreeMap, ffi::OsStr, io::ErrorKind, path::Path};

use cognos::{ParsedDerivation, aterm::Error};
use rom::{
  EngineConfig,
  display::{format_bytes, format_bytes_decimal, render_frame},
  monitor::{DerivationResolver, Engine, Output},
  state::State,
  types::{
    DisplayFormat,
    HostSort,
    IconMode,
    LegendStyle,
    RenderConfig,
    SummaryStyle,
  },
};

const CHAIN_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn chain_path(index: usize) -> String {
  format!("/nix/store/{CHAIN_HASH}-chain-{index}.drv")
}

struct ChainResolver {
  nodes: usize,
}

impl DerivationResolver for ChainResolver {
  fn resolve(&self, path: &Path) -> Result<ParsedDerivation, Error> {
    let index = path
      .file_name()
      .and_then(OsStr::to_str)
      .and_then(|name| name.strip_suffix(".drv"))
      .and_then(|name| name.rsplit_once("-chain-"))
      .and_then(|(_, index)| index.parse::<usize>().ok())
      .ok_or_else(|| {
        Error::Read {
          path:   path.to_owned(),
          source: ErrorKind::NotFound.into(),
        }
      })?;
    let input_drvs = if index + 1 < self.nodes {
      vec![(chain_path(index + 1), vec!["out".to_owned()])]
    } else {
      Vec::new()
    };
    Ok(ParsedDerivation {
      outputs: Vec::new(),
      input_drvs,
      dynamic_drvs: Vec::new(),
      input_srcs: Vec::new(),
      platform: "x86_64-linux".to_owned(),
      builder: "/bin/sh".to_owned(),
      args: Vec::new(),
      env: BTreeMap::new(),
    })
  }
}

#[derive(Clone, Copy)]
enum TestBuildStatus {
  Planned,
  Building,
  Built,
}

fn engine_with_build(status: TestBuildStatus) -> Engine {
  const START: &[u8] = br#"@nix {"action":"start","id":1,"level":3,"parent":0,"text":"building","type":105,"fields":["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-demo.drv","",1,1]}"#;
  let mut engine = Engine::new(EngineConfig::default());
  match status {
    TestBuildStatus::Planned => {
      engine
        .process_record_at(
          br#"@nix {"action":"msg","level":3,"msg":"  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-demo.drv"}"#,
          0.0,
        )
        .unwrap();
    },
    TestBuildStatus::Building => {
      engine.process_record_at(START, 0.0).unwrap();
    },
    TestBuildStatus::Built => {
      engine.process_record_at(START, 0.0).unwrap();
      engine
        .process_record_at(br#"@nix {"action":"stop","id":1}"#, 1.0)
        .unwrap();
    },
  }
  engine
}

#[test]
fn tree_has_continuous_header_and_root_connector() {
  let engine = engine_with_build(TestBuildStatus::Building);
  let frame =
    render_frame(engine.state(), &RenderConfig::default(), 2.0, 79, 23, false);
  let text = frame.text();
  assert!(text.starts_with("╭─ Builds\n├─"), "{text}");
  assert!(text.lines().last().unwrap().starts_with("┗━"), "{text}");
  assert!(text.contains("demo"));
}

#[test]
fn running_build_displays_its_current_phase() {
  let mut engine = engine_with_build(TestBuildStatus::Building);
  let processed = engine
    .process_record_at(
      br#"@nix {"action":"result","id":1,"type":104,"fields":["checkPhase"]}"#,
      1.0,
    )
    .unwrap();
  assert!(processed.changed);
  assert_eq!(processed.output, Vec::<Output>::new());

  for format in [DisplayFormat::Tree, DisplayFormat::Plain] {
    let frame = render_frame(
      engine.state(),
      &RenderConfig {
        format,
        ..RenderConfig::default()
      },
      2.0,
      79,
      23,
      false,
    );
    assert!(frame.text().contains("(checkPhase)"), "{}", frame.text());
  }
}

#[test]
fn final_eof_does_not_claim_an_active_build_succeeded() {
  let engine = engine_with_build(TestBuildStatus::Building);
  let text =
    render_frame(engine.state(), &RenderConfig::default(), 2.0, 79, 23, true)
      .text();
  assert!(text.contains("unfinished"), "{text}");
  assert!(!text.contains("Finished after"), "{text}");
}

#[test]
fn renderer_reserves_requested_width() {
  let engine = engine_with_build(TestBuildStatus::Planned);
  let frame =
    render_frame(engine.state(), &RenderConfig::default(), 0.0, 19, 5, false);
  assert_eq!(frame.buffer.area.width, 19);
  assert!(frame.text().lines().all(|line| line.chars().count() <= 19));
}

#[test]
fn live_tree_is_capped_at_two_thirds_and_keeps_its_legend() {
  let mut engine = Engine::new(EngineConfig::default());
  for id in 1..=30 {
    let record = format!(
      "@nix {{\"action\":\"start\",\"id\":{id},\"level\":3,\"parent\":0,\"\
       text\":\"building\",\"type\":105,\"fields\":[\"/nix/store/{id:\
       032}-demo-{id}.drv\",\"\",1,1]}}"
    );
    engine.process_record_at(record.as_bytes(), 0.0).unwrap();
  }

  let text =
    render_frame(engine.state(), &RenderConfig::default(), 2.0, 79, 23, false)
      .text();
  let lines = text.lines().collect::<Vec<&str>>();
  let legend = lines
    .iter()
    .position(|line| line.starts_with("┢━ Status"))
    .expect("table legend is missing");
  assert_eq!(
    legend, 15,
    "graph exceeded two thirds of the frame:\n{text}"
  );
  assert!(lines[legend - 1].starts_with("├─ …"), "{text}");
  assert!(lines.last().unwrap().starts_with("┗━ Elapsed"), "{text}");
}

#[test]
fn deeply_nested_live_graph_is_bounded_before_rendering() {
  const NODES: usize = 2_000;
  let mut engine = Engine::new(EngineConfig::default());
  engine.set_resolver(ChainResolver { nodes: NODES });
  for path in (0..NODES).map(chain_path) {
    let record =
      format!("@nix {{\"action\":\"msg\",\"level\":3,\"msg\":\"  {path}\"}}");
    engine.process_record_at(record.as_bytes(), 0.0).unwrap();
  }

  let text =
    render_frame(engine.state(), &RenderConfig::default(), 2.0, 79, 23, false)
      .text();
  let lines = text.lines().collect::<Vec<&str>>();
  assert_eq!(lines.len(), 18, "{text}");
  assert!(lines[14].contains("… 1987 more"), "{text}");
  assert!(lines[15].starts_with("┢━ Status"), "{text}");
  assert!(lines.last().unwrap().starts_with("┗━ Elapsed"), "{text}");
}

#[test]
fn waiting_roots_fill_rows_left_after_active_work() {
  let mut engine = Engine::new(EngineConfig::default());
  for id in 1..=20 {
    let record = format!(
      "@nix {{\"action\":\"msg\",\"level\":3,\"msg\":\"  \
       /nix/store/{id:032}-waiting-{id}.drv\"}}"
    );
    engine.process_record_at(record.as_bytes(), 0.0).unwrap();
  }
  engine
    .process_record_at(
      br#"@nix {"action":"start","id":100,"level":3,"parent":0,"text":"building","type":105,"fields":["/nix/store/ffffffffffffffffffffffffffffffff-active.drv","",1,1]}"#,
      1.0,
    )
    .unwrap();

  let text =
    render_frame(engine.state(), &RenderConfig::default(), 2.0, 79, 23, false)
      .text();
  let lines = text.lines().collect::<Vec<&str>>();
  assert!(lines[1].contains("active"), "{text}");
  assert!(text.contains("waiting-1"), "{text}");
  assert!(text.contains("… 8 more"), "{text}");
}

#[test]
fn minimum_live_height_keeps_every_table_legend_row() {
  let mut engine = engine_with_build(TestBuildStatus::Building);
  for record in [
    br#"@nix {"action":"start","id":2,"level":3,"parent":0,"text":"copying","type":108,"fields":["/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-download","https://cache.nixos.org",""]}"#.as_slice(),
    br#"@nix {"action":"start","id":3,"level":3,"parent":0,"text":"copying","type":100,"fields":["/nix/store/cccccccccccccccccccccccccccccccc-upload","","ssh://builder"]}"#.as_slice(),
  ] {
    engine.process_record_at(record, 0.0).unwrap();
  }

  let text =
    render_frame(engine.state(), &RenderConfig::default(), 2.0, 79, 7, false)
      .text();
  let lines = text.lines().collect::<Vec<&str>>();
  assert_eq!(lines.len(), 7, "{text}");
  assert!(lines[1].starts_with("├─ …"), "{text}");
  assert!(lines[2].starts_with("┢━ Status"), "{text}");
  assert!(lines[3].contains("Builds"), "{text}");
  assert!(lines[4].contains("Downloads"), "{text}");
  assert!(lines[5].contains("Uploads"), "{text}");
  assert!(lines[6].starts_with("┗━ Elapsed"), "{text}");
}

#[test]
fn every_format_has_a_useful_initial_frame() {
  for (format, expected) in [
    (DisplayFormat::Tree, "Builds"),
    (DisplayFormat::Plain, "─ Builds"),
    (DisplayFormat::Dashboard, "Build Dashboard"),
  ] {
    let config = RenderConfig {
      format,
      icons: IconMode::Unicode,
      ..RenderConfig::default()
    };
    let text = render_frame(&State::new(), &config, 0.0, 79, 23, false).text();
    assert!(text.contains(expected), "{format:?}: {text}");
  }
}

#[test]
fn plain_and_dashboard_keep_their_own_final_layouts() {
  let engine = engine_with_build(TestBuildStatus::Built);
  let plain = render_frame(
    engine.state(),
    &RenderConfig {
      format: DisplayFormat::Plain,
      icons: IconMode::Unicode,
      ..RenderConfig::default()
    },
    2.0,
    79,
    23,
    true,
  )
  .text();
  assert!(plain.contains("─ Builds"), "{plain}");
  assert!(plain.contains("✔\u{a0}1 completed"), "{plain}");
  assert!(plain.contains("Finished after"), "{plain}");
  assert!(!plain.contains("╰─"), "{plain}");

  let dashboard = render_frame(
    engine.state(),
    &RenderConfig {
      format: DisplayFormat::Dashboard,
      icons: IconMode::Unicode,
      ..RenderConfig::default()
    },
    2.0,
    79,
    23,
    true,
  )
  .text();
  assert!(dashboard.starts_with("╭─ Build Dashboard"), "{dashboard}");
  assert!(dashboard.lines().last().unwrap().starts_with("╰─ Summary"));
  assert!(!dashboard.contains("Finished after"), "{dashboard}");
}

#[test]
fn tree_legends_are_distinct_and_other_formats_ignore_them() {
  let engine = engine_with_build(TestBuildStatus::Building);
  let render = |format, legend| {
    render_frame(
      engine.state(),
      &RenderConfig {
        format,
        legend_style: legend,
        icons: IconMode::Unicode,
        ..RenderConfig::default()
      },
      2.0,
      79,
      23,
      false,
    )
    .text()
  };

  let compact = render(DisplayFormat::Tree, LegendStyle::Compact);
  let table = render(DisplayFormat::Tree, LegendStyle::Table);
  let verbose = render(DisplayFormat::Tree, LegendStyle::Verbose);
  let nom = render(DisplayFormat::Tree, LegendStyle::Nom);
  assert!(!compact.contains("Status"), "{compact}");
  assert!(table.contains("┢━ Status"), "{table}");
  assert!(verbose.contains("┢━ Build Summary"), "{verbose}");
  assert!(nom.contains("┢━━━ Builds"), "{nom}");
  assert!(nom.contains("┗━ ∑"), "{nom}");
  assert_ne!(compact, table);
  assert_ne!(table, verbose);
  assert_ne!(table, nom);

  for format in [DisplayFormat::Plain, DisplayFormat::Dashboard] {
    assert_eq!(
      render(format, LegendStyle::Compact),
      render(format, LegendStyle::Verbose),
      "{format:?} should not render a tree legend",
    );
  }
}

#[test]
fn curl_progress_is_opt_in_and_preserves_logs() {
  let feed_curl = |curl_progress: bool| {
    let mut engine = Engine::new(EngineConfig {
      curl_progress,
      ..EngineConfig::default()
    });
    let mut logs = Vec::new();
    for (time, record) in [
      (
        0.0,
        br#"@nix {"action":"start","id":1,"level":3,"parent":0,"text":"building","type":105,"fields":["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-Geekbench-6.7.1-Linux.tar.gz.drv","",1,1]}"#.as_slice(),
      ),
      (
        0.5,
        br#"@nix {"action":"result","id":1,"type":101,"fields":["trying https://cdn.geekbench.com/Geekbench-6.7.1-Linux.tar.gz"]}"#,
      ),
      (
        1.0,
        br#"@nix {"action":"result","id":1,"type":101,"fields":["  % Total    % Received % Xferd  Average Speed   Time    Time     Time  Current"]}"#,
      ),
      (
        1.5,
        br#"@nix {"action":"result","id":1,"type":101,"fields":[" 24 217.5M   24 52.34M    0     0  7.77M      0  0:00:27  0:00:06  0:00:21  9.89M"]}"#,
      ),
    ] {
      let processed = engine.process_record_at(record, time).unwrap();
      logs.extend(processed.output);
    }
    (engine, logs)
  };

  let (disabled_engine, disabled_logs) = feed_curl(false);
  let (enabled_engine, enabled_logs) = feed_curl(true);

  // Raw curl log lines are never filtered out.
  assert_eq!(disabled_logs.len(), 3);
  assert_eq!(enabled_logs.len(), 3);

  let config = RenderConfig {
    legend_style: LegendStyle::Nom,
    icons: IconMode::Unicode,
    progress_bars: false,
    show_percentages: false,
    decimal_bytes: true,
    ..RenderConfig::default()
  };

  let disabled_text =
    render_frame(disabled_engine.state(), &config, 2.0, 110, 23, false).text();
  assert!(
    !disabled_text.contains("cdn.geekbench.com"),
    "{disabled_text}"
  );

  let enabled_text =
    render_frame(enabled_engine.state(), &config, 2.0, 110, 23, false).text();
  assert!(
    enabled_text.contains("from cdn.geekbench.com"),
    "{enabled_text}"
  );
  assert!(
    enabled_text.contains("52.3 MiB/217.5 MiB"),
    "{enabled_text}"
  );
  assert!(
    enabled_text.contains("cdn.geekbench.com (https)"),
    "{enabled_text}"
  );
}

#[test]
fn nom_legend_sorts_and_caps_hosts_into_other() {
  let mut engine = Engine::new(EngineConfig {
    curl_progress: true,
    ..EngineConfig::default()
  });

  // 1 local build + 3 remote download hosts of different sizes and counts:
  // - small.example (2 downloads, 2 MiB total)
  // - medium.example (1 download, 10 MiB total)
  // - large.example (1 download, 100 MiB total)
  for record in [
    r#"@nix {"action":"start","id":1,"level":3,"parent":0,"text":"building","type":105,"fields":["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-app.drv","",1,1]}"#,
    r#"@nix {"action":"start","id":10,"level":3,"parent":0,"text":"copying","type":108,"fields":["/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-s1","https://small.example"]}"#,
    r#"@nix {"action":"result","id":10,"type":105,"fields":[524288,1048576,1,0]}"#,
    r#"@nix {"action":"start","id":11,"level":3,"parent":0,"text":"copying","type":108,"fields":["/nix/store/cccccccccccccccccccccccccccccccc-s2","https://small.example"]}"#,
    r#"@nix {"action":"result","id":11,"type":105,"fields":[524288,1048576,1,0]}"#,
    r#"@nix {"action":"start","id":12,"level":3,"parent":0,"text":"copying","type":108,"fields":["/nix/store/dddddddddddddddddddddddddddddddd-m1","https://medium.example"]}"#,
    r#"@nix {"action":"result","id":12,"type":105,"fields":[2097152,10485760,1,0]}"#,
    r#"@nix {"action":"start","id":13,"level":3,"parent":0,"text":"copying","type":108,"fields":["/nix/store/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-l1","https://large.example"]}"#,
    r#"@nix {"action":"result","id":13,"type":105,"fields":[10485760,104857600,1,0]}"#,
  ] {
    engine.process_record_at(record.as_bytes(), 0.0).unwrap();
  }

  let render_with = |host_sort, max_hosts, final_render| {
    render_frame(
      engine.state(),
      &RenderConfig {
        legend_style: LegendStyle::Nom,
        summary_style: SummaryStyle::Nom,
        icons: IconMode::Unicode,
        host_sort,
        max_hosts,
        decimal_bytes: true,
        ..RenderConfig::default()
      },
      2.0,
      120,
      25,
      final_render,
    )
    .text()
  };

  let by_size = render_with(HostSort::DownloadSize, None, false);
  let pos_small = by_size.find("small.example (https)").unwrap();
  let pos_medium = by_size.find("medium.example (https)").unwrap();
  let pos_large = by_size.find("large.example (https)").unwrap();
  assert!(
    pos_small < pos_medium && pos_medium < pos_large,
    "expected ascending size order:\n{by_size}"
  );

  let by_count = render_with(HostSort::DownloadCount, None, false);
  let pos_medium = by_count.find("medium.example (https)").unwrap();
  let pos_large = by_count.find("large.example (https)").unwrap();
  let pos_small = by_count.find("small.example (https)").unwrap();
  assert!(
    pos_medium < pos_large && pos_large < pos_small,
    "expected ascending download-count order:\n{by_count}"
  );

  let capped = render_with(HostSort::DownloadSize, Some(1), false);
  assert!(capped.contains("(other)"), "{capped}");
  assert!(capped.contains("large.example (https)"), "{capped}");
  assert!(!capped.contains("small.example"), "{capped}");
  assert!(!capped.contains("medium.example"), "{capped}");
  // (other) aggregates small (1.0/2.0 MiB) + medium (2.0/10.0 MiB) = 3.0/12.0
  // MiB
  assert!(capped.contains("3.0 MiB/12.0 MiB"), "{capped}");

  let final_nom = render_with(HostSort::DownloadSize, Some(1), true);
  assert!(final_nom.contains("large.example (https)"), "{final_nom}");
  assert!(final_nom.contains("┗━ ∑"), "{final_nom}");
}

#[test]
fn progress_bars_percentages_and_decimal_bytes_toggle_independently() {
  assert_eq!(format_bytes(62_914_560), "60 MiB");
  assert_eq!(format_bytes_decimal(62_914_560), "60.0 MiB");

  let mut engine = Engine::new(EngineConfig::default());
  for record in [
    r#"@nix {"action":"start","id":10,"level":3,"parent":0,"text":"copying","type":108,"fields":["/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-pkg","https://cache.nixos.org"]}"#,
    r#"@nix {"action":"result","id":10,"type":105,"fields":[15728640,62914560,1,0]}"#,
  ] {
    engine.process_record_at(record.as_bytes(), 0.0).unwrap();
  }

  let default_text = render_frame(
    engine.state(),
    &RenderConfig {
      icons: IconMode::Unicode,
      ..RenderConfig::default()
    },
    2.0,
    100,
    20,
    false,
  )
  .text();
  assert!(default_text.contains('━'), "{default_text}");
  assert!(default_text.contains("25%"), "{default_text}");
  assert!(default_text.contains("15 MiB/60 MiB"), "{default_text}");

  let numbers_only = render_frame(
    engine.state(),
    &RenderConfig {
      icons: IconMode::Unicode,
      progress_bars: false,
      show_percentages: false,
      decimal_bytes: true,
      ..RenderConfig::default()
    },
    2.0,
    100,
    20,
    false,
  )
  .text();
  assert!(
    !numbers_only.contains('╸') && !numbers_only.contains("──"),
    "{numbers_only}"
  );
  assert!(!numbers_only.contains('%'), "{numbers_only}");
  assert!(numbers_only.contains("15.0 MiB/60.0 MiB"), "{numbers_only}");

  // Plain format respects the same toggles.
  let plain_numbers = render_frame(
    engine.state(),
    &RenderConfig {
      format: DisplayFormat::Plain,
      icons: IconMode::Unicode,
      progress_bars: false,
      show_percentages: false,
      decimal_bytes: true,
      ..RenderConfig::default()
    },
    2.0,
    100,
    20,
    false,
  )
  .text();
  assert!(!plain_numbers.contains('━'), "{plain_numbers}");
  assert!(!plain_numbers.contains('%'), "{plain_numbers}");
  assert!(
    plain_numbers.contains("15.0 MiB/60.0 MiB"),
    "{plain_numbers}"
  );
}
