//! Public configuration and presentation types.

use cognos::Verbosity;
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
  /// Parses a format name: `tree`, `plain`, or `dashboard`.
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
  /// Status counts on a single line.
  Compact,
  /// Status counts in a boxed table.
  #[default]
  Table,
  /// A table with every status spelled out.
  Verbose,
}

impl LegendStyle {
  /// Parses a legend style name: `compact`, `table`, or `verbose`.
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
  /// A single line of totals.
  #[default]
  Concise,
  /// Totals in a table.
  Table,
  /// Totals and every finished build.
  Full,
}

impl SummaryStyle {
  /// Parses a summary style name: `concise`, `table`, or `full`.
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
  /// The derivation's short name.
  #[default]
  Short,
  /// The full derivation path.
  Full,
  /// No prefix.
  None,
}

impl LogPrefixStyle {
  /// Parses a log prefix style name: `short`, `full`, or `none`.
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
  /// Tree connectors and box borders.
  pub connector:      Color,
  /// Ordinary text.
  pub text:           Color,
  /// Secondary text, such as timers.
  pub muted:          Color,
  /// Planned work.
  pub planned:        Color,
  /// Running work.
  pub running:        Color,
  /// Completed work.
  pub completed:      Color,
  /// Failed work.
  pub failed:         Color,
  /// Builder log prefixes.
  pub log_prefix:     Color,
  /// Host names.
  pub host:           Color,
  /// Downloads.
  pub download:       Color,
  /// Uploads.
  pub upload:         Color,
  /// The unfilled part of progress bars.
  pub progress_track: Color,
}

/// Icon selection.
///
/// `Auto` also honors the existing `NERD_FONTS` override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconMode {
  /// Nerd Fonts if `NERD_FONTS` asks for them, otherwise Unicode.
  #[default]
  Auto,
  /// Plain Unicode symbols.
  Unicode,
  /// Nerd Fonts glyphs.
  Nerd,
}

impl Default for Theme {
  fn default() -> Self {
    Self {
      connector:      Color::DarkGray,
      // The terminal's own foreground, readable on light and dark themes.
      text:           Color::Reset,
      muted:          Color::DarkGray,
      planned:        Color::Blue,
      running:        Color::Yellow,
      completed:      Color::Green,
      failed:         Color::Red,
      log_prefix:     Color::DarkGray,
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
  /// Suppress decoded logs and presentations.
  pub silent:           bool,
  /// Most verbose message level that is shown.
  pub verbosity:        Verbosity,
  /// How input records are recognized.
  pub input_mode:       InputMode,
  /// Prefix used for builder log lines.
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
  ///
  /// Exact non-protocol passthrough is never changed by this setting.
  pub ansi:          bool,
  /// Show elapsed and estimated times.
  pub show_timers:   bool,
  /// Width override; defaults to the terminal width.
  pub width:         Option<u16>,
  /// Height override; defaults to the terminal height.
  pub height:        Option<u16>,
  /// Primary presentation.
  pub format:        DisplayFormat,
  /// Legend detail.
  pub legend_style:  LegendStyle,
  /// Final summary detail.
  pub summary_style: SummaryStyle,
  /// Colors.
  pub theme:         Theme,
  /// Icon selection.
  pub icons:         IconMode,
}

impl Default for EngineConfig {
  fn default() -> Self {
    Self {
      silent:           false,
      verbosity:        Verbosity::Info,
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

/// Complete adapter configuration.
///
/// The engine and renderer retain only their respective halves.
#[derive(Debug, Clone, Default)]
pub struct Config {
  /// Engine half.
  pub engine: EngineConfig,
  /// Renderer half.
  pub render: RenderConfig,
}

/// A decoded logical log line.
///
/// Presentation adapters decide whether to retain producer styling and how to
/// color the optional activity prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
  /// Activity prefix, such as the derivation name.
  pub prefix: String,
  /// Message with the producer's ANSI styling.
  pub styled: String,
  /// Message without styling.
  pub plain:  String,
}
