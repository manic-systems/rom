//! Public configuration and presentation types.

use ratatui_core::style::Color;

/// How ROM interprets structured input records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
  /// Parse only records beginning with `@nix ` and pass all other bytes
  /// through.
  #[default]
  Auto,
  /// Parse every non-empty record as an unprefixed internal-JSON action.
  Json,
}

/// Primary presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayFormat {
  /// A connected dependency forest.
  #[default]
  Tree,
  /// One activity per line without dependency connectors.
  Plain,
  /// A compact aggregate dashboard.
  Dashboard,
}

impl DisplayFormat {
  #[must_use]
  pub fn parse(value: &str) -> Option<Self> {
    match value {
      "tree" => Some(Self::Tree),
      "plain" => Some(Self::Plain),
      "dashboard" => Some(Self::Dashboard),
      _ => None,
    }
  }
}

/// Legend detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LegendStyle {
  Compact,
  #[default]
  Table,
  Verbose,
}

impl LegendStyle {
  #[must_use]
  pub fn parse(value: &str) -> Option<Self> {
    match value {
      "compact" => Some(Self::Compact),
      "table" => Some(Self::Table),
      "verbose" => Some(Self::Verbose),
      _ => None,
    }
  }
}

/// Final-summary detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SummaryStyle {
  #[default]
  Concise,
  Table,
  Full,
}

impl SummaryStyle {
  #[must_use]
  pub fn parse(value: &str) -> Option<Self> {
    match value {
      "concise" => Some(Self::Concise),
      "table" => Some(Self::Table),
      "full" => Some(Self::Full),
      _ => None,
    }
  }
}

/// Prefix used for decoded builder log lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogPrefixStyle {
  #[default]
  Short,
  Full,
  None,
}

impl LogPrefixStyle {
  #[must_use]
  pub fn parse(value: &str) -> Option<Self> {
    match value {
      "short" => Some(Self::Short),
      "full" => Some(Self::Full),
      "none" => Some(Self::None),
      _ => None,
    }
  }
}

/// Semantic colors used by every presentation.
///
/// The fields deliberately describe meaning rather than individual widgets so
/// alternate renderers can share a theme without copying a palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
  pub connector:      Color,
  pub text:           Color,
  pub muted:          Color,
  pub planned:        Color,
  pub running:        Color,
  pub completed:      Color,
  pub failed:         Color,
  pub log_prefix:     Color,
  pub host:           Color,
  pub download:       Color,
  pub upload:         Color,
  pub progress_track: Color,
}

/// Icon selection. `Auto` also honors the existing `NERD_FONTS` override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconMode {
  #[default]
  Auto,
  Unicode,
  Nerd,
}

impl Default for Theme {
  fn default() -> Self {
    Self {
      connector:      Color::Blue,
      text:           Color::White,
      muted:          Color::DarkGray,
      planned:        Color::Blue,
      running:        Color::Yellow,
      completed:      Color::Green,
      failed:         Color::Red,
      log_prefix:     Color::Blue,
      host:           Color::Magenta,
      download:       Color::Cyan,
      upload:         Color::Magenta,
      progress_track: Color::DarkGray,
    }
  }
}

/// Structured-input and log policy owned by the synchronous engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
  pub silent:           bool,
  pub input_mode:       InputMode,
  pub log_prefix_style: LogPrefixStyle,
  /// Maximum decoded builder log lines per activity.
  pub log_line_limit:   Option<usize>,
  /// Maximum size of one claimed internal-JSON record.
  pub max_record_bytes: usize,
}

/// Terminal-independent rendering and serialization policy.
#[derive(Debug, Clone)]
pub struct RenderConfig {
  /// Emit ANSI styling for decoded logs and append-only presentations.
  /// Exact non-protocol passthrough is never changed by this setting.
  pub ansi:          bool,
  pub show_timers:   bool,
  pub width:         Option<u16>,
  pub height:        Option<u16>,
  pub format:        DisplayFormat,
  pub legend_style:  LegendStyle,
  pub summary_style: SummaryStyle,
  pub theme:         Theme,
  pub icons:         IconMode,
}

impl Default for EngineConfig {
  fn default() -> Self {
    Self {
      silent:           false,
      input_mode:       InputMode::Auto,
      log_prefix_style: LogPrefixStyle::Short,
      log_line_limit:   None,
      max_record_bytes: 4 * 1024 * 1024,
    }
  }
}

impl Default for RenderConfig {
  fn default() -> Self {
    Self {
      ansi:          false,
      show_timers:   true,
      width:         None,
      height:        None,
      format:        DisplayFormat::Tree,
      legend_style:  LegendStyle::Table,
      summary_style: SummaryStyle::Concise,
      theme:         Theme::default(),
      icons:         IconMode::Auto,
    }
  }
}

/// Complete adapter configuration. The engine and renderer retain only their
/// respective halves.
#[derive(Debug, Clone, Default)]
pub struct Config {
  pub engine: EngineConfig,
  pub render: RenderConfig,
}

/// A decoded logical log line. Presentation adapters decide whether to retain
/// producer styling and how to color the optional activity prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
  pub prefix: String,
  pub styled: String,
  pub plain:  String,
}
