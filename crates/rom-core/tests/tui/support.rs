pub(super) use std::collections::HashSet;

pub(super) use ratatui::{
  Terminal,
  backend::TestBackend,
  style::{Color, Modifier},
};
pub(super) use rom_core::{
  display::{DisplayConfig, render_state_lines},
  icons,
  state::{
    BuildInfo,
    BuildStatus,
    Derivation,
    DerivationId,
    FailType,
    InputDerivation,
    State,
    StorePath,
    StorePathId,
    TransferInfo,
    current_time,
  },
  tui::{TuiConfig, TuiView, draw},
  types::{DisplayFormat, LegendStyle, SummaryStyle},
};

pub(super) const GRAPH_LINE_COLOR: Color = Color::Rgb(82, 89, 78);
pub(super) const MOSS_GREEN: Color = Color::Rgb(158, 190, 112);
pub(super) const MUTED_RED: Color = Color::Rgb(204, 102, 96);
pub(super) const MUTED_YELLOW: Color = Color::Rgb(224, 190, 96);

pub(super) fn tui_config() -> TuiConfig {
  TuiConfig {
    display:        DisplayConfig {
      use_color: true,
      format: DisplayFormat::Tree,
      legend_style: LegendStyle::Table,
      summary_style: SummaryStyle::Concise,
      icons: &icons::UNICODE,
      ..DisplayConfig::default()
    },
    log_line_limit: Some(8),
  }
}

pub(super) fn running_state() -> State {
  let mut state = State::new();
  let drv_id = add_derivation(&mut state, "hello-1.0");
  state.update_build_status(
    drv_id,
    BuildStatus::Building(BuildInfo {
      start:       current_time(),
      host:        cognos::Host::Localhost,
      estimate:    None,
      activity_id: None,
    }),
  );
  state.forest_roots.push(drv_id);
  state
}

pub(super) fn add_derivation(state: &mut State, name: &str) -> DerivationId {
  let drv = Derivation::parse(&format!(
    "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-{name}.drv"
  ))
  .unwrap();
  state.get_or_create_derivation_id(drv)
}

pub(super) fn add_output_path(
  state: &mut State,
  drv_id: DerivationId,
  name: &str,
) -> StorePathId {
  let path_id = add_store_path(state, name);
  state.get_store_path_info_mut(path_id).unwrap().producer = Some(drv_id);
  state
    .get_derivation_info_mut(drv_id)
    .unwrap()
    .outputs
    .insert(cognos::OutputName::parse("out"), path_id);
  path_id
}

pub(super) fn add_store_path(state: &mut State, name: &str) -> StorePathId {
  let path = StorePath::parse(&format!(
    "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-{name}"
  ))
  .unwrap();
  state.get_or_create_store_path_id(path)
}

pub(super) fn row_text(terminal: &Terminal<TestBackend>, row: u16) -> String {
  let buffer = terminal.backend().buffer();
  (0..buffer.area.width)
    .map(|x| buffer[(x, row)].symbol())
    .collect()
}

pub(super) fn row_containing(
  terminal: &Terminal<TestBackend>,
  needle: &str,
) -> Option<u16> {
  let buffer = terminal.backend().buffer();
  (0..buffer.area.height).find(|row| row_text(terminal, *row).contains(needle))
}
