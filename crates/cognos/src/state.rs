#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProgressState {
  JustStarted,
  InputReceived,
  Finished,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OutputName {
  Out,
  Doc,
  Dev,
  Bin,
  Info,
  Lib,
  Man,
  Dist,
  Other(String),
}

impl OutputName {
  #[must_use]
  pub fn parse(name: &str) -> Self {
    match name {
      name if name.eq_ignore_ascii_case("out") => Self::Out,
      name if name.eq_ignore_ascii_case("doc") => Self::Doc,
      name if name.eq_ignore_ascii_case("dev") => Self::Dev,
      name if name.eq_ignore_ascii_case("bin") => Self::Bin,
      name if name.eq_ignore_ascii_case("info") => Self::Info,
      name if name.eq_ignore_ascii_case("lib") => Self::Lib,
      name if name.eq_ignore_ascii_case("man") => Self::Man,
      name if name.eq_ignore_ascii_case("dist") => Self::Dist,
      _ => Self::Other(name.to_string()),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Host {
  Localhost,
  Remote(String),
}

impl Host {
  #[must_use]
  pub fn name(&self) -> &str {
    match self {
      Self::Localhost => "localhost",
      Self::Remote(name) => name,
    }
  }
}
