use std::path::Path;

use rom::{
  EngineConfig,
  display::render_frame,
  monitor::{DerivationResolver, Engine},
  types::{DisplayFormat, RenderConfig},
};
use serde_json::json;

const ROOT: &str = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-root.drv";
const CONSUMER: &str =
  "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-consumer.drv";
const OTHER: &str =
  "/nix/store/cccccccccccccccccccccccccccccccc-other-consumer.drv";
const PRODUCER: &str =
  "/nix/store/dddddddddddddddddddddddddddddddd-producer.drv";
const SOURCE: &str = "/nix/store/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-source-tree";
const CACHED: &str =
  "/nix/store/ffffffffffffffffffffffffffffffff-cached-source";

struct Sources {
  shared:   bool,
  producer: bool,
  paths:    Vec<String>,
}

impl Default for Sources {
  fn default() -> Self {
    Self {
      shared:   false,
      producer: false,
      paths:    vec![SOURCE.into(), CACHED.into()],
    }
  }
}

impl DerivationResolver for Sources {
  fn resolve(&self, path: &Path) -> Result<cognos::ParsedDerivation, String> {
    let path = path.to_str().unwrap();
    let inputs = match path {
      ROOT if self.shared => vec![CONSUMER, OTHER],
      ROOT => vec![CONSUMER],
      CONSUMER if self.producer => vec![PRODUCER],
      _ => vec![],
    };
    Ok(cognos::ParsedDerivation {
      outputs:    if path == PRODUCER {
        vec![("out".into(), SOURCE.into())]
      } else {
        vec![]
      },
      input_drvs: inputs
        .into_iter()
        .map(|input| (input.into(), vec!["out".into()]))
        .collect(),
      input_srcs: if matches!(path, CONSUMER | OTHER) {
        self.paths.clone()
      } else {
        vec![]
      },
      platform:   "x86_64-linux".into(),
      builder:    "/bin/sh".into(),
      args:       vec![],
      env:        vec![],
    })
  }
}

fn feed(engine: &mut Engine, event: serde_json::Value, now: f64) {
  engine
    .process_record_at(format!("@nix {event}").as_bytes(), now)
    .unwrap();
}

fn plan(engine: &mut Engine, path: &str) {
  feed(
    engine,
    json!({"action":"msg", "level":3, "msg":format!("  {path}")}),
    0.0,
  );
}

fn download(engine: &mut Engine, path: &str, id: u64, parent: u64) {
  feed(
    engine,
    json!({"action":"start", "id":id, "parent":parent, "level":3,
    "text":"copying", "type":108, "fields":[path, "https://cache.nixos.org"]}),
    1.0,
  );
  feed(
    engine,
    json!({"action":"result", "id":id, "type":105, "fields":[50, 100, 1, 0]}),
    1.1,
  );
}

fn setup(sources: Sources) -> Engine {
  let mut engine = Engine::new(EngineConfig::default());
  engine.set_resolver(sources);
  plan(&mut engine, ROOT);
  engine
}

fn frame(
  engine: &Engine,
  format: DisplayFormat,
  now: f64,
  rows: u16,
) -> String {
  render_frame(
    engine.state(),
    &RenderConfig {
      format,
      ..RenderConfig::default()
    },
    now,
    119,
    rows,
    false,
  )
  .text()
}

#[test]
fn source_edges_survive_resolution_and_downloads_reveal_their_ancestors() {
  let mut engine = setup(Sources::default());
  let (&consumer, _) = engine
    .state()
    .derivations()
    .iter()
    .find(|(_, info)| info.name.path == Path::new(CONSUMER))
    .unwrap();
  let source_consumers: Vec<_> = engine
    .state()
    .store_paths()
    .values()
    .filter(|path| path.input_for.contains(&consumer))
    .collect();
  assert_eq!(source_consumers.len(), 2);
  let before = frame(&engine, DisplayFormat::Tree, 0.0, 23);
  assert!(!before.contains("source-tree"), "{before}");
  download(&mut engine, SOURCE, 10, 0);
  let text = frame(&engine, DisplayFormat::Tree, 1.2, 23);
  assert!(text.contains("root"), "{text}");
  assert!(text.contains("consumer"), "{text}");
  assert!(text.contains("┃     ┗━ source-tree"), "{text}");
  assert!(text.contains("50%"), "{text}");
  assert!(!text.contains("Transfers"), "{text}");
  assert!(!text.contains("cached-source"), "{text}");
  assert!(!engine.state().derivations().values().any(|info| {
    matches!(info.build_status, rom::state::BuildStatus::Building(_))
  }));
  assert_eq!(engine.state().summary().running_downloads.len(), 1);
}

#[test]
fn shared_sources_choose_a_planned_consumer_once_and_keep_global_counts_unique()
{
  let mut engine = setup(Sources {
    shared: true,
    ..Sources::default()
  });
  plan(&mut engine, OTHER);
  download(&mut engine, SOURCE, 10, 0);
  for _ in 0..20 {
    let text = frame(&engine, DisplayFormat::Tree, 1.2, 23);
    assert_eq!(text.matches("source-tree").count(), 1, "{text}");
    assert!(text.contains("used by 2 builds"), "{text}");
    let lines: Vec<_> = text.lines().collect();
    let source = lines
      .iter()
      .position(|line| line.contains("source-tree"))
      .unwrap();
    assert!(lines[source - 1].contains("other-consumer"), "{text}");
  }
  assert_eq!(engine.state().summary().running_downloads.len(), 1);
}

#[test]
fn narrow_shared_source_rows_keep_the_name_and_progress() {
  let mut engine = setup(Sources {
    shared: true,
    ..Sources::default()
  });
  download(&mut engine, SOURCE, 10, 0);
  let text =
    render_frame(engine.state(), &RenderConfig::default(), 1.2, 39, 23, false)
      .text();
  let source = text
    .lines()
    .find(|line| line.contains("source-tree"))
    .unwrap();
  assert!(source.contains("50%"), "{text}");
  assert!(!source.contains("used by"), "{text}");
}

#[test]
fn source_downloads_remain_visible_in_non_tree_formats() {
  let mut engine = setup(Sources::default());
  download(&mut engine, SOURCE, 10, 0);
  let plain = frame(&engine, DisplayFormat::Plain, 1.2, 23);
  assert_eq!(plain.matches("source-tree").count(), 1, "{plain}");
  assert!(plain.contains("50%"), "{plain}");
  let dashboard = frame(&engine, DisplayFormat::Dashboard, 1.2, 23);
  assert!(dashboard.contains("Transfer"), "{dashboard}");
  assert!(dashboard.contains("50%"), "{dashboard}");
}

#[test]
fn known_producers_take_precedence_over_source_consumers() {
  let mut engine = setup(Sources {
    producer: true,
    ..Sources::default()
  });
  download(&mut engine, SOURCE, 10, 0);
  let text = frame(&engine, DisplayFormat::Tree, 1.2, 23);
  assert!(
    text
      .lines()
      .any(|line| line.contains("producer") && line.contains("50%")),
    "{text}"
  );
  assert!(!text.contains("source-tree"), "{text}");
  assert!(!text.contains("Transfers"), "{text}");
}

#[test]
fn late_metadata_overrides_activity_nesting_without_inventing_a_producer() {
  let mut engine = Engine::new(EngineConfig::default());
  feed(
    &mut engine,
    json!({"action":"start", "id":1, "parent":0, "level":3,
    "text":"building", "type":105, "fields":[ROOT, "", 1, 1]}),
    0.0,
  );
  download(&mut engine, SOURCE, 10, 1);
  let before = frame(&engine, DisplayFormat::Tree, 1.2, 23);
  assert!(
    before
      .lines()
      .any(|line| line.contains("root") && line.contains("50%")),
    "{before}"
  );
  engine.set_resolver(Sources::default());
  plan(&mut engine, ROOT);
  let text = frame(&engine, DisplayFormat::Tree, 1.2, 23);
  assert!(text.contains("source-tree"), "{text}");
  assert!(text.contains("consumer"), "{text}");
  let path = engine
    .state()
    .store_paths()
    .values()
    .find(|info| info.name.name == "source-tree")
    .unwrap();
  assert!(path.producer.is_none());
  assert!(
    engine
      .state()
      .summary()
      .running_downloads
      .values()
      .any(|transfer| transfer.parent.is_some())
  );
}

#[test]
fn completed_sources_expire_from_the_graph_but_not_the_summary() {
  let mut engine = setup(Sources::default());
  download(&mut engine, SOURCE, 10, 0);
  feed(&mut engine, json!({"action":"stop", "id":10}), 2.0);
  let recent = frame(&engine, DisplayFormat::Tree, 2.1, 23);
  assert!(recent.contains("source-tree"), "{recent}");
  let later = frame(&engine, DisplayFormat::Tree, 3.1, 23);
  assert!(!later.contains("source-tree"), "{later}");
  assert_eq!(engine.state().summary().completed_downloads.len(), 1);
  assert!(engine.state().summary().running_downloads.is_empty());
}

#[test]
fn unrelated_downloads_and_source_uploads_keep_the_transfer_branch() {
  for upload in [false, true] {
    let mut engine = setup(Sources::default());
    if upload {
      feed(
        &mut engine,
        json!({"action":"start", "id":10, "parent":0, "level":3,
        "text":"copying", "type":100, "fields":[SOURCE, "", "ssh://builder"]}),
        1.0,
      );
    } else {
      download(
        &mut engine,
        "/nix/store/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-unrelated",
        10,
        0,
      );
    }
    let text = frame(&engine, DisplayFormat::Tree, 1.2, 23);
    assert!(text.contains("Transfers"), "{text}");
    assert!(!text.contains("consumer"), "{text}");
  }
}

#[test]
fn later_upload_does_not_inherit_a_completed_download_parent() {
  let mut engine = Engine::new(EngineConfig::default());
  feed(
    &mut engine,
    json!({"action":"start", "id":1, "parent":0, "level":3,
    "text":"building", "type":105, "fields":[ROOT, "", 1, 1]}),
    0.0,
  );
  download(&mut engine, SOURCE, 10, 1);
  feed(&mut engine, json!({"action":"stop", "id":10}), 1.0);
  feed(
    &mut engine,
    json!({"action":"start", "id":11, "parent":0, "level":3,
    "text":"copying", "type":100, "fields":[SOURCE, "", "ssh://builder"]}),
    3.0,
  );

  let text = frame(&engine, DisplayFormat::Tree, 3.2, 23);
  assert!(text.contains("Transfers"), "{text}");
  assert!(text.contains("source-tree"), "{text}");
}

#[test]
fn shared_sources_use_stable_ties_and_prefer_a_running_consumer() {
  let mut engine = setup(Sources {
    shared: true,
    ..Sources::default()
  });
  plan(&mut engine, CONSUMER);
  plan(&mut engine, OTHER);
  download(&mut engine, SOURCE, 10, 0);
  let selected_parent = |text: &str| {
    let lines: Vec<_> = text.lines().collect();
    let index = lines
      .iter()
      .position(|line| line.contains("source-tree"))
      .unwrap();
    lines[index - 1].to_string()
  };
  for _ in 0..20 {
    let text = frame(&engine, DisplayFormat::Tree, 1.2, 23);
    let parent = selected_parent(&text);
    assert!(
      parent.contains("consumer") && !parent.contains("other-consumer"),
      "{text}"
    );
  }
  feed(
    &mut engine,
    json!({"action":"start", "id":1, "parent":0, "level":3,
    "text":"building", "type":105, "fields":[OTHER, "", 1, 1]}),
    1.3,
  );
  let text = frame(&engine, DisplayFormat::Tree, 1.4, 23);
  assert!(selected_parent(&text).contains("other-consumer"), "{text}");
  assert_eq!(text.matches("source-tree").count(), 1, "{text}");
}

#[test]
fn source_rows_obey_the_live_graph_budget() {
  let paths: Vec<_> = (0..30)
    .map(|id| {
      format!("/nix/store/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-source-{id:02}")
    })
    .collect();
  let mut engine = setup(Sources {
    paths: paths.clone(),
    ..Sources::default()
  });
  for (id, path) in paths.iter().enumerate() {
    download(&mut engine, path, id as u64 + 10, 0);
  }
  let text = frame(&engine, DisplayFormat::Tree, 1.2, 23);
  let lines: Vec<_> = text.lines().collect();
  let legend = lines
    .iter()
    .position(|line| line.starts_with("┣━ Status"))
    .unwrap();
  assert_eq!(legend, 15, "{text}");
  assert!(lines[legend - 1].contains("hidden"), "{text}");
  assert!(
    lines.iter().any(|line| line.contains("source-00")),
    "{text}"
  );
  assert_eq!(engine.state().summary().running_downloads.len(), 30);
}

#[test]
fn final_tree_filters_completed_transfers_in_every_placement() {
  for placement in ["source", "producer", "parent", "unmatched"] {
    let mut engine = match placement {
      "source" => setup(Sources::default()),
      "producer" => {
        setup(Sources {
          producer: true,
          ..Sources::default()
        })
      },
      _ => Engine::new(EngineConfig::default()),
    };
    let parent = if placement == "parent" {
      feed(
        &mut engine,
        json!({"action":"start", "id":1, "parent":0,
        "level":3, "text":"building", "type":105, "fields":[ROOT, "", 1, 1]}),
        0.0,
      );
      1
    } else {
      0
    };
    download(&mut engine, SOURCE, 10, parent);
    feed(&mut engine, json!({"action":"stop", "id":10}), 2.0);
    let config = RenderConfig::default();
    let live =
      render_frame(engine.state(), &config, 2.1, 119, 40, false).text();
    assert!(live.contains("100%"), "{placement}: {live}");
    let final_frame =
      render_frame(engine.state(), &config, 2.1, 119, 40, true).text();
    assert!(!final_frame.contains('%'), "{placement}: {final_frame}");
    assert!(
      !final_frame.contains("source-tree"),
      "{placement}: {final_frame}"
    );
    // Presentation filtering never destroys retained totals or live grace.
    assert_eq!(engine.state().summary().completed_downloads.len(), 1);
    assert!(
      frame(&engine, DisplayFormat::Plain, 2.1, 40).contains("source-tree")
    );
  }
}

#[test]
fn final_source_filter_preserves_active_transfers_and_completed_totals() {
  let mut engine = setup(Sources::default());
  download(&mut engine, SOURCE, 10, 0);
  download(&mut engine, CACHED, 11, 0);
  feed(&mut engine, json!({"action":"stop", "id":10}), 2.0);
  for format in [
    DisplayFormat::Tree,
    DisplayFormat::Plain,
    DisplayFormat::Dashboard,
  ] {
    let config = RenderConfig {
      format,
      ..RenderConfig::default()
    };
    let text = render_frame(engine.state(), &config, 2.1, 119, 40, true).text();
    assert!(text.contains("50%"), "{text}");
    assert!(!text.contains("source-tree"), "{text}");
    if format != DisplayFormat::Dashboard {
      assert!(text.contains("cached-source"), "{text}");
    }
  }
  assert_eq!(engine.state().summary().completed_downloads.len(), 1);
  assert_eq!(engine.state().summary().running_downloads.len(), 1);
}

#[test]
fn source_heavy_branch_cannot_displace_another_active_build() {
  let paths: Vec<_> = (0..20)
    .map(|id| {
      format!("/nix/store/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-source-{id:02}")
    })
    .collect();
  let mut engine = setup(Sources {
    shared: true,
    paths: paths.clone(),
    ..Sources::default()
  });
  for (id, path) in [(1, CONSUMER), (2, OTHER)] {
    feed(
      &mut engine,
      json!({"action":"start", "id":id, "parent":0,
      "level":3, "text":"building", "type":105, "fields":[path, "", 1, 1]}),
      0.0,
    );
  }
  for (id, path) in paths.iter().enumerate() {
    download(&mut engine, path, id as u64 + 10, 0);
  }
  for rows in 1..=40 {
    let text = frame(&engine, DisplayFormat::Tree, 1.2, rows);
    assert!(text.lines().count() <= usize::from(rows), "{rows}: {text}");
    if rows >= 12 {
      assert!(text.contains("root"), "{rows}: {text}");
      assert!(
        text.lines().any(|line| {
          line.contains("consumer") && !line.contains("other-consumer")
        }),
        "{rows}: {text}"
      );
      assert!(text.contains("other-consumer"), "{rows}: {text}");
      assert!(text.contains("source-00"), "{rows}: {text}");
      if rows < 30 {
        assert!(text.contains("source downloads hidden"), "{rows}: {text}");
      }
    }
    assert_eq!(text, frame(&engine, DisplayFormat::Tree, 1.2, rows));
    let final_text = render_frame(
      engine.state(),
      &RenderConfig::default(),
      1.2,
      119,
      rows,
      true,
    )
    .text();
    assert!(
      final_text.lines().count() <= usize::from(rows),
      "{rows}: {final_text}"
    );
    if rows >= 12 {
      assert!(final_text.contains("root"), "{rows}: {final_text}");
      assert!(
        final_text.contains("other-consumer"),
        "{rows}: {final_text}"
      );
      assert!(final_text.contains("source-00"), "{rows}: {final_text}");
    }
  }
}

#[test]
fn source_details_share_spare_rows_with_unmatched_transfers() {
  let paths: Vec<_> = (0..20)
    .map(|id| {
      format!("/nix/store/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-source-{id:02}")
    })
    .collect();
  let mut engine = setup(Sources {
    paths: paths.clone(),
    ..Sources::default()
  });
  for (id, path) in paths.iter().enumerate() {
    download(&mut engine, path, id as u64 + 10, 0);
  }
  download(
    &mut engine,
    "/nix/store/ffffffffffffffffffffffffffffffff-unrelated",
    100,
    0,
  );
  let text = frame(&engine, DisplayFormat::Tree, 1.2, 12);
  assert!(text.contains("consumer"), "{text}");
  assert!(text.contains("source-00"), "{text}");
  assert!(text.contains("Transfers"), "{text}");
  assert!(text.contains("unrelated"), "{text}");
  assert!(text.lines().count() <= 12, "{text}");
}

#[test]
fn active_source_details_take_priority_over_completion_grace() {
  let completed = "/nix/store/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-aaa-completed";
  let active = "/nix/store/ffffffffffffffffffffffffffffffff-zzz-active";
  let other_completed =
    "/nix/store/dddddddddddddddddddddddddddddddd-bbb-completed";
  let mut engine = setup(Sources {
    paths: vec![completed.into(), active.into(), other_completed.into()],
    ..Sources::default()
  });
  download(&mut engine, completed, 10, 0);
  download(&mut engine, active, 11, 0);
  download(&mut engine, other_completed, 12, 0);
  feed(&mut engine, json!({"action":"stop", "id":10}), 2.0);
  feed(&mut engine, json!({"action":"stop", "id":12}), 2.0);
  let text = frame(&engine, DisplayFormat::Tree, 2.1, 9);
  assert!(text.contains("zzz-active"), "{text}");
  assert!(!text.contains("aaa-completed"), "{text}");
  assert!(!text.contains("bbb-completed"), "{text}");
  assert!(text.contains("2 source downloads hidden"), "{text}");
  assert!(text.lines().count() <= 9, "{text}");
}
