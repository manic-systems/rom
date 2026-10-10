//! Icon sets for ROM display output.
//!
//! Two sets are available: Unicode (standard, widely supported) and Nerd Fonts
//! (requires a patched font or a terminal that bundles the glyphs).

use std::env;

/// A complete set of display icons.
pub struct Icons {
  /// Marks a running build.
  pub running:  &'static str,
  /// Marks a finished build.
  pub done:     &'static str,
  /// Marks a planned build.
  pub planned:  &'static str,
  /// Marks a failed build.
  pub failed:   &'static str,
  /// Marks a download.
  pub download: &'static str,
  /// Marks an upload.
  pub upload:   &'static str,
  /// Precedes elapsed time.
  pub clock:    &'static str,
  /// Precedes an estimated duration.
  pub estimate: &'static str,
  /// Marks the summary.
  pub summary:  &'static str,
  /// Separator printed after an icon.
  ///
  /// A no-break space rather than a space: Ghostty draws symbol glyphs up to
  /// two cells wide when U+0020 or U+2002 follows them, which would eat the
  /// gap for wide icons only. Every other terminal shows an ordinary space.
  pub gap:      &'static str,
}

/// Standard Unicode icons, which need no special font.
pub static UNICODE: Icons = Icons {
  running:  "\u{23f5}", // ⏵
  done:     "\u{2714}", // ✔
  planned:  "\u{23f8}", // ⏸
  failed:   "\u{2717}", // ✗
  download: "\u{2193}", // ↓
  upload:   "\u{2191}", // ↑
  clock:    "\u{23f1}", // ⏱
  estimate: "\u{2205}", // ∅
  summary:  "\u{2211}", // ∑
  gap:      "\u{a0}",
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
  gap:      "\u{a0}",
};

/// Detects the best icon set for the current terminal session.
///
/// Checks `NERD_FONTS` env override first (`1` forces Nerd, `0` forces
/// Unicode), then checks for terminals that bundle Nerd Font glyphs.
#[must_use]
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

/// Resolves a configured icon choice.
#[must_use]
pub fn select(mode: crate::types::IconMode) -> &'static Icons {
  match mode {
    crate::types::IconMode::Auto => detect(),
    crate::types::IconMode::Unicode => &UNICODE,
    crate::types::IconMode::Nerd => &NERD,
  }
}
