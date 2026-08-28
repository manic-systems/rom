//! Error types for ROM

use std::io;

use thiserror::Error;

/// Result type for ROM operations
pub type Result<T> = std::result::Result<T, RomError>;

/// Main error type for ROM
#[derive(Debug, Error)]
pub enum RomError {
  /// IO error
  #[error("IO error: {0}")]
  Io(#[from] io::Error),

  /// JSON parsing error
  #[error("JSON parsing error: {0}")]
  Json(#[from] serde_json::Error),

  /// Build failed
  #[error("Build failed")]
  BuildFailed,

  /// Input ended while structured work was still active.
  #[error("Input ended with unfinished activities")]
  Incomplete,

  /// Process execution error
  #[error("Process error: {0}")]
  Process(String),

  /// Configuration error
  #[error("Configuration error: {0}")]
  Config(String),

  /// Parse error
  #[error("Parse error: {0}")]
  Parse(String),

  /// Terminal error
  #[error("Terminal error: {0}")]
  Terminal(String),

  /// Other error
  #[error("{0}")]
  Other(String),
}

impl RomError {
  /// Create a process error
  pub fn process<S: Into<String>>(msg: S) -> Self {
    Self::Process(msg.into())
  }

  /// Create a config error
  pub fn config<S: Into<String>>(msg: S) -> Self {
    Self::Config(msg.into())
  }

  /// Create a parse error
  pub fn parse<S: Into<String>>(msg: S) -> Self {
    Self::Parse(msg.into())
  }

  /// Create a terminal error
  pub fn terminal<S: Into<String>>(msg: S) -> Self {
    Self::Terminal(msg.into())
  }

  /// Create an other error
  pub fn other<S: Into<String>>(msg: S) -> Self {
    Self::Other(msg.into())
  }
}
