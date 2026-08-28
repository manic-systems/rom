use std::{hint::black_box, time::Instant};

use rom::{Engine, EngineConfig, RenderConfig, display::render_frame};

fn main() {
  const DERIVATIONS: usize = 10_000;
  const RECORDS: usize = 100_000;
  let mut engine = Engine::new(EngineConfig::default());
  let started = Instant::now();
  for id in 1..=DERIVATIONS {
    let record = format!(
      "@nix {{\"action\":\"start\",\"id\":{id},\"level\":3,\"parent\":0,\"\
       text\":\"\",\"type\":105,\"fields\":[\"/nix/store/\
       aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bench-{id}.drv\",\"\",1,1]}}"
    );
    engine
      .process_record_at(record.as_bytes(), id as f64)
      .unwrap();
  }
  for index in DERIVATIONS..RECORDS {
    let id = index % DERIVATIONS + 1;
    let record = format!(
      "@nix {{\"action\":\"result\",\"id\":{id},\"type\":105,\"fields\":\
       [{index},{RECORDS},1,0]}}"
    );
    engine
      .process_record_at(record.as_bytes(), index as f64)
      .unwrap();
  }
  let elapsed = started.elapsed();
  println!(
    "reduced {RECORDS} records and retained {} derivations in {elapsed:?}",
    engine.state().derivations().len(),
  );
  assert_eq!(engine.state().derivations().len(), DERIVATIONS);

  const FRAMES: usize = 120;
  let render = RenderConfig::default();
  let started = Instant::now();
  let rendered_rows: usize = (0..FRAMES)
    .map(|frame| {
      usize::from(
        black_box(render_frame(
          engine.state(),
          &render,
          RECORDS as f64 + frame as f64 / 20.0,
          159,
          47,
          false,
        ))
        .height,
      )
    })
    .sum();
  let elapsed = started.elapsed();
  println!(
    "rendered {FRAMES} frames over {DERIVATIONS} visible derivations in \
     {elapsed:?}",
  );
  assert!(rendered_rows > 0);
}
