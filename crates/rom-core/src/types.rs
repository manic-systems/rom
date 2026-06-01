//! Core types for ROM
use std::{convert::Infallible, str::FromStr};

/// Legend display style
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegendStyle {
  /// Compact single-line legend
  Compact,
  /// Table with host columns
  Table,
  /// Verbose full legend
  Verbose,
}

impl FromStr for LegendStyle {
  type Err = Infallible;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    Ok(match s.to_lowercase().as_str() {
      "compact" => Self::Compact,
      "verbose" => Self::Verbose,
      _ => Self::Table,
    })
  }
}

/// Display format for output
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayFormat {
  /// Show dependency tree graph
  Tree,
  /// Plain text output
  Plain,
  /// Dashboard summary view
  Dashboard,
}

/// Log prefix style for build logs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogPrefixStyle {
  /// Just package name (pname)
  Short,
  /// Full derivation name with version
  Full,
  /// No prefix
  None,
}

/// Summary display style
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryStyle {
  /// Concise single-line summary
  Concise,
  /// Table with host breakdown
  Table,
  /// Full detailed summary
  Full,
}

impl FromStr for SummaryStyle {
  type Err = Infallible;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    Ok(match s.to_lowercase().as_str() {
      "concise" => Self::Concise,
      "table" => Self::Table,
      "full" => Self::Full,
      _ => Self::Concise,
    })
  }
}

impl FromStr for LogPrefixStyle {
  type Err = Infallible;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    Ok(match s.to_lowercase().as_str() {
      "short" => Self::Short,
      "full" => Self::Full,
      "none" => Self::None,
      _ => Self::Short,
    })
  }
}

impl FromStr for DisplayFormat {
  type Err = Infallible;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    Ok(match s.to_lowercase().as_str() {
      "tree" => Self::Tree,
      "plain" => Self::Plain,
      "dashboard" => Self::Dashboard,
      _ => Self::Tree,
    })
  }
}

/// Configuration for the monitor
#[derive(Debug, Clone)]
pub struct Config {
  /// Whether we're piping output through
  pub piping:           bool,
  /// Silent mode - minimal output
  pub silent:           bool,
  /// Input parsing mode
  pub input_mode:       InputMode,
  /// Show completion times
  pub show_timers:      bool,
  /// Terminal width override
  pub width:            Option<usize>,
  /// Display format
  pub format:           DisplayFormat,
  /// Legend display style
  pub legend_style:     LegendStyle,
  /// Summary display style
  pub summary_style:    SummaryStyle,
  /// Log prefix style for build logs
  pub log_prefix_style: LogPrefixStyle,
  /// Maximum number of log lines to display (None = unlimited)
  pub log_line_limit:   Option<usize>,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      piping:           false,
      silent:           false,
      input_mode:       InputMode::Human,
      show_timers:      true,
      width:            None,
      format:           DisplayFormat::Tree,
      legend_style:     LegendStyle::Table,
      summary_style:    SummaryStyle::Concise,
      log_prefix_style: LogPrefixStyle::Short,
      log_line_limit:   None,
    }
  }
}

/// Input parsing mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
  /// Parse JSON output from nix --log-format=internal-json
  Json,
  /// Parse human-readable nix output
  Human,
}
