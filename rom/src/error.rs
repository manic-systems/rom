//! Error types for ROM.

use std::io;

use flub::Error;

/// Result type for ROM operations.
pub type Result<T> = std::result::Result<T, RomError>;

/// Main error type for ROM.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RomError {
  /// I/O error.
  #[error("I/O failed")]
  Io(#[from] io::Error),

  /// An internal-JSON record that does not decode.
  #[error("invalid internal-JSON record")]
  Json(#[source] serde_json::Error),

  /// An internal-JSON record longer than the configured limit.
  #[error("internal-JSON record exceeds {limit} bytes")]
  RecordTooLarge {
    /// The configured `max_record_bytes`.
    limit: usize,
  },

  /// A record without the `@nix ` prefix reached the decoder.
  #[error("non-protocol record reached the decoder")]
  NotProtocol,

  /// Build failed.
  #[error("Build failed")]
  BuildFailed,

  /// Input ended while structured work was still active.
  #[error("Input ended with unfinished activities")]
  Incomplete,
}
