/// Internal log format parsers.
///
/// Each submodule handles one wire format emitted by a Nix-family evaluator.
/// New formats should be added as sibling modules here.
pub mod json;

use std::{process::Command, str::FromStr};

/// A Nix-family evaluator that rom can monitor.
///
/// Both platforms emit `--log-format internal-json` on stderr with `@nix `
/// prefixed lines. Lix extends the `msg` action with optional source-location
/// fields (`raw_msg`, `file`, `line`, `column`); the shared parser in
/// `json` handles both transparently via serde defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Platform {
  /// Upstream Nix.
  #[default]
  Nix,
  /// Lix (a Nix fork). Adds source-location fields to `msg` actions.
  Lix,
}

impl Platform {
  /// The executable name for this platform.
  ///
  /// Both Nix and Lix ship as `nix`; Lix is a drop-in replacement rather
  /// than a separate command. This method exists so callers do not
  /// hardcode the name and future platforms (e.g. Tvix) can diverge.
  #[must_use]
  pub const fn binary(self) -> &'static str {
    match self {
      Self::Nix | Self::Lix => "nix",
    }
  }

  /// Attempt to detect the active platform by inspecting `nix --version`
  /// output.
  ///
  /// # Returns
  ///
  /// `Nix` if the version string contains neither "lix" nor fails to run.
  #[must_use]
  pub fn detect() -> Self {
    let output = Command::new("nix").arg("--version").output();

    if let Ok(out) = output {
      let version = String::from_utf8_lossy(&out.stdout);
      if version
        .as_bytes()
        .windows(3)
        .any(|s| s.eq_ignore_ascii_case(b"lix"))
      {
        return Self::Lix;
      }
    }

    Self::Nix
  }
}

impl FromStr for Platform {
  type Err = ();

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s {
      s if s.eq_ignore_ascii_case("nix") => Ok(Self::Nix),
      s if s.eq_ignore_ascii_case("lix") => Ok(Self::Lix),
      _ => Err(()),
    }
  }
}
