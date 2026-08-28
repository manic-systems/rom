use rom::{
  EngineConfig,
  display::render_frame,
  monitor::{DerivationResolver, Engine},
  state::State,
  types::{DisplayFormat, IconMode, LegendStyle, RenderConfig},
};

const CHAIN_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn chain_path(index: usize) -> String {
  format!("/nix/store/{CHAIN_HASH}-chain-{index}.drv")
}

struct ChainResolver {
  nodes: usize,
}

impl DerivationResolver for ChainResolver {
  fn resolve(
    &self,
    path: &std::path::Path,
  ) -> Result<cognos::ParsedDerivation, String> {
    let name = path
      .file_name()
      .and_then(std::ffi::OsStr::to_str)
      .ok_or_else(|| format!("invalid derivation path: {}", path.display()))?;
    let index = name
      .strip_suffix(".drv")
      .and_then(|name| name.rsplit_once("-chain-").map(|(_, index)| index))
      .ok_or_else(|| format!("invalid chain derivation: {name}"))?
      .parse::<usize>()
      .map_err(|error| format!("invalid chain index: {error}"))?;
    let input_drvs = if index + 1 < self.nodes {
      vec![(chain_path(index + 1), vec!["out".to_string()])]
    } else {
      Vec::new()
    };
    Ok(cognos::ParsedDerivation {
      outputs: Vec::new(),
      input_drvs,
      input_srcs: Vec::new(),
      platform: "x86_64-linux".to_string(),
      builder: "/bin/sh".to_string(),
      args: Vec::new(),
      env: Vec::new(),
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
  assert!(text.starts_with("┏━ Builds\n┣━"), "{text}");
  assert!(text.lines().last().unwrap().starts_with("┗━"), "{text}");
  assert!(text.contains("demo"));
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
  let lines: Vec<_> = text.lines().collect();
  let legend = lines
    .iter()
    .position(|line| line.starts_with("┣━ Status"))
    .expect("table legend is missing");
  assert_eq!(
    legend, 15,
    "graph exceeded two thirds of the frame:\n{text}"
  );
  assert!(lines[legend - 1].starts_with("┣━ …"), "{text}");
  assert!(lines.last().unwrap().starts_with("┗━ Elapsed"), "{text}");
}

#[test]
fn deeply_nested_live_graph_is_bounded_before_rendering() {
  const NODES: usize = 2_000;
  let mut engine = Engine::new(EngineConfig::default());
  engine.set_resolver(ChainResolver { nodes: NODES });
  for path in [chain_path(0), chain_path(NODES - 1)] {
    let record =
      format!("@nix {{\"action\":\"msg\",\"level\":3,\"msg\":\"  {path}\"}}");
    engine.process_record_at(record.as_bytes(), 0.0).unwrap();
  }

  let text =
    render_frame(engine.state(), &RenderConfig::default(), 2.0, 79, 23, false)
      .text();
  let lines: Vec<_> = text.lines().collect();
  assert_eq!(lines.len(), 18, "{text}");
  assert!(lines[14].contains("… 1987 hidden"), "{text}");
  assert!(lines[15].starts_with("┣━ Status"), "{text}");
  assert!(lines.last().unwrap().starts_with("┗━ Elapsed"), "{text}");
}

#[test]
fn active_work_does_not_spend_rows_on_unrelated_waiting_roots() {
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
  assert!(text.contains("active"), "{text}");
  assert!(!text.contains("waiting-1"), "{text}");
  assert!(text.contains("… 20 hidden"), "{text}");
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
  let lines: Vec<_> = text.lines().collect();
  assert_eq!(lines.len(), 7, "{text}");
  assert!(lines[1].starts_with("┣━ …"), "{text}");
  assert!(lines[2].starts_with("┣━ Status"), "{text}");
  assert!(lines[3].contains("Builds"), "{text}");
  assert!(lines[4].contains("Downloads"), "{text}");
  assert!(lines[5].contains("Uploads"), "{text}");
  assert!(lines[6].starts_with("┗━ Elapsed"), "{text}");
}

#[test]
fn every_format_has_a_useful_initial_frame() {
  for (format, expected) in [
    (DisplayFormat::Tree, "Builds"),
    (DisplayFormat::Plain, "━ Builds"),
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
  assert!(plain.contains("━ Builds"), "{plain}");
  assert!(plain.contains("✔ 1 completed"), "{plain}");
  assert!(plain.contains("Finished after"), "{plain}");
  assert!(!plain.contains("┗━"), "{plain}");

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
  assert!(dashboard.starts_with("┏━ Build Dashboard"), "{dashboard}");
  assert!(dashboard.lines().last().unwrap().starts_with("┗━ Summary"));
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
  assert!(!compact.contains("Status"), "{compact}");
  assert!(table.contains("┣━ Status"), "{table}");
  assert!(verbose.contains("┣━ Build Summary"), "{verbose}");
  assert_ne!(compact, table);
  assert_ne!(table, verbose);

  for format in [DisplayFormat::Plain, DisplayFormat::Dashboard] {
    assert_eq!(
      render(format, LegendStyle::Compact),
      render(format, LegendStyle::Verbose),
      "{format:?} should not render a tree legend",
    );
  }
}
