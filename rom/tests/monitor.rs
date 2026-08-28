use rom::{
  display::{format_log, render_frame},
  monitor::{DerivationResolver, Engine, Output, StreamEngine},
  types::{EngineConfig, IconMode, LogLine, RenderConfig},
};

fn log_line(value: &str) -> Output {
  Output::Log(LogLine {
    prefix: String::new(),
    styled: value.to_string(),
    plain:  value.to_string(),
  })
}

struct Resolver;

impl DerivationResolver for Resolver {
  fn resolve(
    &self,
    _path: &std::path::Path,
  ) -> Result<cognos::ParsedDerivation, String> {
    Ok(cognos::ParsedDerivation {
      outputs:    Vec::new(),
      input_drvs: vec![(
        "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-child.drv".to_string(),
        vec!["out".to_string()],
      )],
      input_srcs: Vec::new(),
      platform:   "x86_64-linux".to_string(),
      builder:    "/bin/sh".to_string(),
      args:       Vec::new(),
      env:        vec![("pname".to_string(), "demo".to_string())],
    })
  }
}

struct NestedResolver;

impl DerivationResolver for NestedResolver {
  fn resolve(
    &self,
    path: &std::path::Path,
  ) -> Result<cognos::ParsedDerivation, String> {
    let dependency = match path.to_string_lossy().as_ref() {
      "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-root.drv" => {
        Some("/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-child.drv")
      },
      "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-child.drv" => {
        Some("/nix/store/cccccccccccccccccccccccccccccccc-leaf.drv")
      },
      "/nix/store/cccccccccccccccccccccccccccccccc-leaf.drv" => None,
      other => return Err(format!("unexpected derivation: {other}")),
    };
    Ok(cognos::ParsedDerivation {
      outputs:    Vec::new(),
      input_drvs: dependency
        .map(|path| vec![(path.to_string(), vec!["out".to_string()])])
        .unwrap_or_default(),
      input_srcs: Vec::new(),
      platform:   "x86_64-linux".to_string(),
      builder:    "/bin/sh".to_string(),
      args:       Vec::new(),
      env:        Vec::new(),
    })
  }
}

#[test]
fn engine_is_created_without_io_or_runtime() {
  let engine = Engine::new(EngineConfig::default());
  assert!(engine.state().derivations().is_empty());
}

#[test]
fn prefixed_json_and_passthrough_share_one_stream() {
  let mut stream = StreamEngine::new(EngineConfig::default());
  let processed = stream
    .push_at(
      b"hello\n@nix {\"action\":\"msg\",\"level\":3,\"msg\":\"world\"}\n",
      0.0,
    )
    .unwrap();
  assert_eq!(processed.output, vec![
    Output::Passthrough(b"hello\n".to_vec()),
    log_line("world"),
  ]);
}

#[test]
fn decoded_ansi_is_explicit_but_passthrough_remains_exact() {
  let input = b"raw \x1b[36mcyan\x1b[0m\n@nix {\"action\":\"msg\",\"level\":3,\"msg\":\"\\u001b[1;31mbold red\\u001b[0m\"}\n";
  let mut stream = StreamEngine::new(EngineConfig::default());
  let output = stream.push_at(input, 0.0).unwrap().output;
  assert_eq!(output, vec![
    Output::Passthrough(b"raw \x1b[36mcyan\x1b[0m\n".to_vec()),
    log_line("\x1b[1;31mbold red\x1b[0m"),
  ]);
  let Output::Log(log) = &output[1] else {
    panic!("expected decoded log");
  };
  assert_eq!(format_log(log, &RenderConfig::default()), "bold red");
  assert_eq!(
    format_log(log, &RenderConfig {
      ansi: true,
      ..RenderConfig::default()
    }),
    "\x1b[1;31mbold red\x1b[0m\x1b[0m"
  );
}

#[test]
fn eof_preserves_an_unterminated_passthrough_record() {
  let mut stream = StreamEngine::new(EngineConfig::default());
  let first = stream.push_at(b"tail", 0.0).unwrap();
  assert_eq!(first.output, vec![Output::Passthrough(b"tail".to_vec())]);
  assert!(stream.finish_at(1.0).unwrap().output.is_empty());
}

#[test]
fn library_can_inject_derivation_resolution() {
  let mut engine = Engine::new(EngineConfig::default());
  engine.set_resolver(Resolver);
  engine.process_record_at(
    br#"@nix {"action":"start","id":1,"level":3,"parent":0,"text":"","type":105,"fields":["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-demo.drv","",1,1]}"#,
    0.0,
  ).unwrap();
  let root = engine.state().roots()[0];
  let info = engine.state().get_derivation_info(root).unwrap();
  assert_eq!(info.platform.as_deref(), Some("x86_64-linux"));
  assert_eq!(info.input_derivations.len(), 1);
}

#[test]
fn planned_records_build_a_recursive_dependency_tree() {
  let mut engine = Engine::new(EngineConfig::default());
  let render = RenderConfig {
    icons: IconMode::Unicode,
    ..RenderConfig::default()
  };
  engine.set_resolver(NestedResolver);
  for (index, path) in [
    "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-root.drv",
    "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-child.drv",
    "/nix/store/cccccccccccccccccccccccccccccccc-leaf.drv",
  ]
  .into_iter()
  .enumerate()
  {
    let record =
      format!("@nix {{\"action\":\"msg\",\"level\":3,\"msg\":\"  {path}\"}}");
    engine
      .process_record_at(record.as_bytes(), index as f64)
      .unwrap();
  }

  assert_eq!(engine.state().summary().planned_builds.len(), 3);
  assert_eq!(engine.state().roots().len(), 1);
  let text = render_frame(engine.state(), &render, 3.0, 100, 30, false).text();
  assert!(text.contains("┣━ ⏸ root"), "{text}");
  assert!(text.contains("┃  ┗━ ⏸ child"), "{text}");
  assert!(text.contains("┃     ┗━ ⏸ leaf"), "{text}");
  let builds = text
    .lines()
    .find(|line| line.contains("Builds       ⏵"))
    .expect("missing build status row");
  assert!(builds.contains("⏸ 3"), "{builds}");
  assert!(builds.ends_with('3'), "{builds}");
}

#[test]
fn planned_download_moves_through_running_and_completed_counts() {
  const PATH: &str = "/nix/store/dddddddddddddddddddddddddddddddd-download";
  let render = RenderConfig {
    icons: IconMode::Unicode,
    ..RenderConfig::default()
  };
  let mut engine = Engine::new(EngineConfig::default());
  engine
    .process_record_at(
      format!("@nix {{\"action\":\"msg\",\"level\":3,\"msg\":\"  {PATH}\"}}")
        .as_bytes(),
      0.0,
    )
    .unwrap();
  assert_eq!(engine.state().summary().planned_downloads.len(), 1);

  engine
    .process_record_at(
      format!(
        "@nix {{\"action\":\"start\",\"id\":10,\"level\":3,\"parent\":0,\"text\":\"\",\"type\":108,\"fields\":[\"{PATH}\",\"https://cache.nixos.org\"]}}"
      )
      .as_bytes(),
      1.0,
    )
    .unwrap();
  assert!(engine.state().summary().planned_downloads.is_empty());
  assert_eq!(engine.state().summary().running_downloads.len(), 1);
  let running =
    render_frame(engine.state(), &render, 1.0, 100, 30, false).text();
  let downloads = running
    .lines()
    .find(|line| line.contains("Downloads"))
    .expect("missing download status row");
  assert!(downloads.contains("↓ 1"), "{downloads}");
  assert!(downloads.ends_with('1'), "{downloads}");

  engine
    .process_record_at(b"@nix {\"action\":\"stop\",\"id\":10}", 2.0)
    .unwrap();
  assert!(engine.state().summary().running_downloads.is_empty());
  assert_eq!(engine.state().summary().completed_downloads.len(), 1);
  let completed =
    render_frame(engine.state(), &render, 2.0, 100, 30, false).text();
  let downloads = completed
    .lines()
    .find(|line| line.contains("Downloads"))
    .expect("missing download status row");
  assert!(downloads.contains("↓ 0"), "{downloads}");
  assert!(downloads.contains("↓ 1"), "{downloads}");
  assert!(downloads.ends_with('1'), "{downloads}");
}

#[test]
fn descendant_progress_does_not_overwrite_its_transfer_parent() {
  const PATH: &str = "/nix/store/dddddddddddddddddddddddddddddddd-download";
  let mut engine = Engine::new(EngineConfig::default());
  for record in [
    format!(
      "@nix {{\"action\":\"start\",\"id\":20,\"level\":3,\"parent\":0,\
       \"text\":\"copying\",\"type\":108,\"fields\":[\"{PATH}\",\
       \"https://cache.nixos.org\"]}}"
    ),
    r#"@nix {"action":"start","id":21,"level":3,"parent":20,"text":"downloading","type":101,"fields":[]}"#.to_string(),
    r#"@nix {"action":"result","id":20,"type":105,"fields":[197132288,1073741824,1,0]}"#.to_string(),
  ] {
    engine.process_record_at(record.as_bytes(), 0.0).unwrap();
  }
  let transfer = engine
    .state()
    .summary()
    .running_downloads
    .values()
    .next()
    .unwrap();
  assert_eq!(transfer.total_bytes, Some(1_073_741_824));

  engine
    .process_record_at(
      br#"@nix {"action":"result","id":21,"type":105,"fields":[58720256,327155712,1,0]}"#,
      1.0,
    )
    .unwrap();
  let transfer = engine
    .state()
    .summary()
    .running_downloads
    .values()
    .next()
    .unwrap();
  assert_eq!(transfer.bytes_transferred, 197_132_288);
  assert_eq!(transfer.total_bytes, Some(1_073_741_824));
}

#[test]
fn silent_mode_keeps_errors_but_drops_informational_logs() {
  let mut stream = StreamEngine::new(EngineConfig {
    silent: true,
    ..EngineConfig::default()
  });
  let processed = stream.push_at(
    b"@nix {\"action\":\"msg\",\"level\":3,\"msg\":\"info\"}\n@nix {\"action\":\"msg\",\"level\":0,\"msg\":\"error: broken\"}\n",
    0.0,
  ).unwrap();
  assert_eq!(processed.output, vec![log_line("error: broken")]);
}
