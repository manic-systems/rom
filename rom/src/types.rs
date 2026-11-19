//! Public configuration and presentation types.

use cognos::Verbosity;
pub use ratatui_core::style::Color;

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
#[cfg_attr(feature = "cli", derive(pound::ValueEnum))]
#[cfg_attr(feature = "cli", derive(serde::Deserialize))]
#[cfg_attr(feature = "cli", serde(rename_all = "kebab-case"))]
pub enum DisplayFormat {
  /// A connected dependency forest.
  #[default]
  Tree,
  /// One activity per line without dependency connectors.
  Plain,
  /// A compact aggregate dashboard.
  Dashboard,
}

/// Legend detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "cli", derive(pound::ValueEnum))]
#[cfg_attr(feature = "cli", derive(serde::Deserialize))]
#[cfg_attr(feature = "cli", serde(rename_all = "kebab-case"))]
pub enum LegendStyle {
  /// Status counts on a single line.
  Compact,
  /// Status counts in a boxed table.
  #[default]
  Table,
  /// A table with every status spelled out.
  Verbose,
  /// Per-host counts and transfer totals, in the style of nom.
  Nom,
}

/// Which way the tree grows from its roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "cli", derive(pound::ValueEnum))]
#[cfg_attr(feature = "cli", derive(serde::Deserialize))]
#[cfg_attr(feature = "cli", serde(rename_all = "kebab-case"))]
pub enum TreeDirection {
  /// Roots on top, dependencies below them.
  #[default]
  Down,
  /// Roots just above the legend, dependencies above them.
  Up,
}

/// Final-summary detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "cli", derive(pound::ValueEnum))]
#[cfg_attr(feature = "cli", derive(serde::Deserialize))]
#[cfg_attr(feature = "cli", serde(rename_all = "kebab-case"))]
pub enum SummaryStyle {
  /// A single line of totals.
  #[default]
  Concise,
  /// Totals in a table.
  Table,
  /// Totals and every finished build.
  Full,
  /// Per-host counts and transfer totals, in the style of nom.
  Nom,
}

/// How hosts are ordered in the per-host legend and summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "cli", derive(pound::ValueEnum))]
#[cfg_attr(feature = "cli", derive(serde::Deserialize))]
#[cfg_attr(feature = "cli", serde(rename_all = "kebab-case"))]
pub enum HostSort {
  /// Reverse-domain alphabetical order (or by activity when capped).
  #[default]
  None,
  /// Ascending by total download size (largest at the bottom).
  DownloadSize,
  /// Ascending by total download count (most at the bottom).
  DownloadCount,
  /// Ascending by total build count (most at the bottom).
  Builds,
}

/// Prefix used for decoded builder log lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "cli", derive(pound::ValueEnum))]
#[cfg_attr(feature = "cli", derive(serde::Deserialize))]
#[cfg_attr(feature = "cli", serde(rename_all = "kebab-case"))]
pub enum LogPrefixStyle {
  /// The derivation's short name.
  #[default]
  Short,
  /// The full derivation path.
  Full,
  /// No prefix.
  None,
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
  /// Secondary text, such as build phases.
  pub muted:          Color,
  /// Timers and estimates.
  pub timer:          Color,
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
  /// Byte counts for transfer progress (e.g. `15.0 MiB/60.0 MiB`).
  pub transfer_bytes: Color,
  /// The unfilled part of progress bars.
  pub progress_track: Color,
  /// A build's cores while it runs in parallel.
  pub cpu:            Color,
  /// A build stuck under 1.5 cores.
  pub cpu_serial:     Color,
  /// A build using almost no CPU.
  pub cpu_idle:       Color,
  /// A build taking most of the machine while others run.
  pub cpu_heavy:      Color,
}

/// Icon selection.
///
/// `Auto` selects Nerd Fonts for terminals known to bundle those glyphs and
/// falls back to plain Unicode otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "cli", derive(pound::ValueEnum))]
#[cfg_attr(feature = "cli", derive(serde::Deserialize))]
#[cfg_attr(feature = "cli", serde(rename_all = "kebab-case"))]
pub enum IconMode {
  /// Selects Nerd Fonts when the terminal is known to provide them.
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
      timer:          Color::Gray,
      planned:        Color::Blue,
      running:        Color::Yellow,
      completed:      Color::Green,
      failed:         Color::Red,
      log_prefix:     Color::Indexed(67),
      host:           Color::Magenta,
      download:       Color::Cyan,
      upload:         Color::Magenta,
      transfer_bytes: Color::DarkGray,
      progress_track: Color::DarkGray,
      cpu:            Color::Gray,
      cpu_serial:     Color::Yellow,
      cpu_idle:       Color::Red,
      cpu_heavy:      Color::Magenta,
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
  /// Parse curl download progress from builder log lines.
  pub curl_progress:    bool,
}

/// Terminal-independent rendering and serialization policy.
#[derive(Debug, Clone)]
#[expect(
  clippy::struct_excessive_bools,
  reason = "independent display toggles configured by the user"
)]
pub struct RenderConfig {
  /// Emit ANSI styling for decoded logs and append-only presentations.
  ///
  /// Exact non-protocol passthrough is never changed by this setting.
  pub ansi:             bool,
  /// Show elapsed and estimated times.
  pub show_timers:      bool,
  /// Width override; defaults to the terminal width.
  pub width:            Option<u16>,
  /// Height override; defaults to the terminal height.
  pub height:           Option<u16>,
  /// Primary presentation.
  pub format:           DisplayFormat,
  /// Which way the tree grows.
  pub tree_direction:   TreeDirection,
  /// Legend detail.
  pub legend_style:     LegendStyle,
  /// Final summary detail.
  pub summary_style:    SummaryStyle,
  /// Host ordering in the nom legend and summary.
  pub host_sort:        HostSort,
  /// Maximum number of remote hosts shown before aggregating the rest into
  /// `(other)`.
  pub max_hosts:        Option<usize>,
  /// Show progress bars on running builds and transfers.
  pub progress_bars:    bool,
  /// Show percentage values alongside transfer sizes.
  pub show_percentages: bool,
  /// Always include a decimal digit when formatting KiB and larger sizes.
  pub decimal_bytes:    bool,
  /// Colors.
  pub theme:            Theme,
  /// Icon selection.
  pub icons:            IconMode,
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
      curl_progress:    false,
    }
  }
}

impl Default for RenderConfig {
  fn default() -> Self {
    Self {
      ansi:             false,
      show_timers:      true,
      width:            None,
      height:           None,
      format:           DisplayFormat::Tree,
      tree_direction:   TreeDirection::Down,
      legend_style:     LegendStyle::Table,
      summary_style:    SummaryStyle::Concise,
      host_sort:        HostSort::None,
      max_hosts:        None,
      progress_bars:    true,
      show_percentages: true,
      decimal_bytes:    false,
      theme:            Theme::default(),
      icons:            IconMode::Auto,
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
