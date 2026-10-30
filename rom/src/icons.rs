//! Icon sets for ROM display output.
//!
//! Two sets are available: Unicode (standard, widely supported) and Nerd Fonts
//! (requires a patched font or a terminal that bundles the glyphs).

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
  /// Precedes the cores a build is using.
  pub cpu:      &'static str,
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
  running:  "⏵",
  done:     "✔",
  planned:  "⏸",
  failed:   "✗",
  download: "↓",
  upload:   "↑",
  clock:    "⏱",
  estimate: "∅",
  cpu:      "⚙",
  summary:  "∑",
  gap:      "\u{a0}",
};

/// Nerd Fonts icons.
///
/// Requires a Nerd Font–patched terminal font or bundled glyphs.
pub static NERD: Icons = Icons {
  running:  "",
  done:     "",
  planned:  "",
  failed:   "",
  download: "",
  upload:   "",
  clock:    "",
  estimate: "",
  cpu:      "",
  summary:  "󰒠",
  gap:      "\u{a0}",
};

/// Resolves a configured icon choice.
#[must_use]
pub fn select(mode: crate::types::IconMode) -> &'static Icons {
  match mode {
    crate::types::IconMode::Unicode => &UNICODE,
    crate::types::IconMode::Nerd => &NERD,
  }
}
