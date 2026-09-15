//! Icon sets for ROM display output.
//!
//! Two sets are available: Unicode (standard, widely supported) and Nerd Fonts
//! (requires a patched font or a terminal that bundles the glyphs).

use std::env;

/// A complete set of display icons.
pub struct Icons {
  pub running:  &'static str,
  pub done:     &'static str,
  pub planned:  &'static str,
  pub failed:   &'static str,
  pub download: &'static str,
  pub upload:   &'static str,
  pub clock:    &'static str,
  pub estimate: &'static str,
  pub summary:  &'static str,
}

/// Standard Unicode icons are always available, no special font required.
pub static UNICODE: Icons = Icons {
  running:  "⏵",
  done:     "✔",
  planned:  "⏸",
  failed:   "✗",
  download: "↓",
  upload:   "↑",
  clock:    "⏱",
  estimate: "∅",
  summary:  "∑",
};

/// Nerd Fonts icons.
///
/// Requires a Nerd Font–patched terminal font or bundled glyphs. Terminal
/// detection can be overridden with `NERD_FONTS=1` (or
/// disabled with `NERD_FONTS=0`).
pub static NERD: Icons = Icons {
  running:  "\u{f04b}",  // 
  done:     "\u{f00c}",  // 
  planned:  "\u{f04c}",  // 
  failed:   "\u{f071}",  // 
  download: "\u{f063}",  // 
  upload:   "\u{f062}",  // 
  clock:    "\u{f1da}",  // 
  estimate: "\u{f252}",  // 
  summary:  "\u{f04a0}", // 󰒠
};

/// Detect the best icon set for the current terminal session.
///
/// Checks `NERD_FONTS` env override first (`1` forces Nerd, `0` forces
/// Unicode), then checks for terminals that bundle Nerd Font glyphs.
pub fn detect() -> &'static Icons {
  // Manual override takes precedence
  if let Ok(value) = env::var("NERD_FONTS") {
    match value.trim().to_ascii_lowercase().as_str() {
      "1" | "true" | "yes" => return &NERD,
      "0" | "false" | "no" => return &UNICODE,
      _ => {},
    }
  }

  let program = env::var("TERM_PROGRAM").unwrap_or_default();
  let terminal = env::var("TERM").unwrap_or_default();
  if matches!(
    program.trim().to_ascii_lowercase().as_str(),
    "ghostty" | "wezterm" | "kitty" | "superset"
  ) || matches!(
    terminal.to_ascii_lowercase().as_str(),
    "xterm-ghostty" | "xterm-kitty"
  ) {
    &NERD
  } else {
    &UNICODE
  }
}

/// Resolve a configured icon choice.
#[must_use]
pub fn select(mode: crate::types::IconMode) -> &'static Icons {
  match mode {
    crate::types::IconMode::Auto => detect(),
    crate::types::IconMode::Unicode => &UNICODE,
    crate::types::IconMode::Nerd => &NERD,
  }
}
