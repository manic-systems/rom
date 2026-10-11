//! Small domain types shared by the parsers.

/// Progress of a monitored run.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProgressState {
  /// No input has been received yet.
  JustStarted,
  /// At least one input record has been received.
  InputReceived,
  /// The input has ended.
  Finished,
}

/// Name of a derivation output.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OutputName {
  /// The default `out` output.
  Out,
  /// The `doc` output.
  Doc,
  /// The `dev` output.
  Dev,
  /// The `bin` output.
  Bin,
  /// The `info` output.
  Info,
  /// The `lib` output.
  Lib,
  /// The `man` output.
  Man,
  /// The `dist` output.
  Dist,
  /// Any other output name.
  Other(String),
}

impl OutputName {
  /// Parses an output name, ignoring ASCII case for the well-known names.
  #[must_use]
  pub fn parse(name: &str) -> Self {
    match name {
      _ if name.eq_ignore_ascii_case("out") => Self::Out,
      _ if name.eq_ignore_ascii_case("doc") => Self::Doc,
      _ if name.eq_ignore_ascii_case("dev") => Self::Dev,
      _ if name.eq_ignore_ascii_case("bin") => Self::Bin,
      _ if name.eq_ignore_ascii_case("info") => Self::Info,
      _ if name.eq_ignore_ascii_case("lib") => Self::Lib,
      _ if name.eq_ignore_ascii_case("man") => Self::Man,
      _ if name.eq_ignore_ascii_case("dist") => Self::Dist,
      _ => Self::Other(name.to_owned()),
    }
  }
}

/// Machine that a build or transfer runs on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Host {
  /// The local machine.
  Localhost,
  /// A remote builder or store, identified by its name.
  Remote(String),
}

impl Host {
  /// Returns the name shown for this host.
  #[must_use]
  pub fn name(&self) -> &str {
    match self {
      Self::Localhost => "localhost",
      Self::Remote(name) => name,
    }
  }
}
