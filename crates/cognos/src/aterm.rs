//! `ATerm` and Nix `.drv` file parser.
//!
//! Parses Nix .drv files in `ATerm` format to extract dependency information.
use std::{
  collections::{BTreeMap, btree_map::Entry},
  fs,
  io,
  path::{Path, PathBuf},
};

use flub::Error;

/// Parsed derivation information from a `.drv` file.
#[derive(Debug, Clone)]
pub struct ParsedDerivation {
  /// Output names paired with their store paths.
  pub outputs:      Vec<(String, String)>,
  /// Input derivation paths paired with the outputs used from each.
  pub input_drvs:   Vec<(String, Vec<String>)>,
  /// Input derivations whose outputs are themselves derivations whose outputs
  /// this one uses, as `builtins.outputOf` records them.
  pub dynamic_drvs: Vec<String>,
  /// Source store paths used as inputs.
  pub input_srcs:   Vec<String>,
  /// System the derivation builds for, such as `x86_64-linux`.
  pub platform:     String,
  /// Path of the builder executable.
  pub builder:      String,
  /// Arguments passed to the builder.
  pub args:         Vec<String>,
  /// Environment variables set for the builder. Values are byte strings and
  /// need not be UTF-8.
  pub env:          BTreeMap<String, Vec<u8>>,
}

/// Failure to read or parse a `.drv` file.
#[derive(Debug, Error)]
pub enum Error {
  /// The file could not be read.
  #[error("failed to read {path}")]
  Read {
    /// Path of the derivation.
    path:   PathBuf,
    /// Underlying I/O failure.
    #[source]
    source: io::Error,
  },
  /// The file is not a valid derivation.
  #[error("failed to parse {path}")]
  Parse {
    /// Path of the derivation.
    path:   PathBuf,
    /// Underlying parse failure.
    #[source]
    source: ParseError,
  },
}

/// Malformed derivation content.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{kind} at byte {offset}")]
pub struct ParseError {
  /// Byte offset into the content where parsing failed.
  pub offset: usize,
  /// What went wrong at `offset`.
  pub kind:   ParseErrorKind,
}

/// The kind of [`ParseError`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseErrorKind {
  /// A specific token was required.
  #[error("expected `{0}`")]
  Expected(&'static str),
  /// Content continues after the closing parenthesis.
  #[error("unexpected data after the derivation")]
  TrailingData,
  /// A string that must be text is not valid UTF-8.
  #[error("invalid UTF-8 in string")]
  InvalidUtf8,
  /// An environment variable is defined twice.
  #[error("duplicate environment variable `{0}`")]
  DuplicateEnvKey(String),
}

/// Parses a `.drv` file and extracts its dependency information.
///
/// # Errors
///
/// Returns [`Error`] if the file cannot be read or is not a valid derivation.
pub fn parse_drv_file<P: AsRef<Path>>(
  path: P,
) -> Result<ParsedDerivation, Error> {
  let path = path.as_ref();
  let content = fs::read(path).map_err(|source| {
    Error::Read {
      path: path.to_owned(),
      source,
    }
  })?;
  parse_drv_content(&content).map_err(|source| {
    Error::Parse {
      path: path.to_owned(),
      source,
    }
  })
}

/// Parses the content of a `.drv` file.
///
/// # Errors
///
/// Returns [`ParseError`] if `content` is not a valid derivation.
pub fn parse_drv_content(
  content: &[u8],
) -> Result<ParsedDerivation, ParseError> {
  let mut cursor = Cursor::new(content);
  // Nix writes `DrvWithVersion("xp-dyn-drv", ...)` once an input is dynamic.
  if cursor.eat("DrvWithVersion(") {
    cursor.bytes()?;
    cursor.expect(",")?;
  } else {
    cursor.expect("Derive(")?;
  }

  let outputs = cursor.list(|cursor| {
    cursor.expect("(")?;
    let name = cursor.string()?;
    cursor.expect(",")?;
    let path = cursor.string()?;
    cursor.expect(",")?;
    cursor.bytes()?;
    cursor.expect(",")?;
    cursor.bytes()?;
    cursor.expect(")")?;
    Ok((name, path))
  })?;
  cursor.expect(",")?;

  let mut dynamic_drvs = Vec::new();
  let input_drvs = cursor.list(|cursor| {
    cursor.expect("(")?;
    let path = cursor.string()?;
    cursor.expect(",")?;
    // A dynamic input's outputs are `([own],[("out",node),...])`.
    let outputs = if cursor.eat("(") {
      let own = cursor.list(Cursor::string)?;
      cursor.expect(",")?;
      if !cursor.list(Cursor::output_node)?.is_empty() {
        dynamic_drvs.push(path.clone());
      }
      cursor.expect(")")?;
      own
    } else {
      cursor.list(Cursor::string)?
    };
    cursor.expect(")")?;
    Ok((path, outputs))
  })?;
  cursor.expect(",")?;

  let input_srcs = cursor.list(Cursor::string)?;
  cursor.expect(",")?;
  let platform = cursor.string()?;
  cursor.expect(",")?;
  let builder = cursor.string()?;
  cursor.expect(",")?;
  let args = cursor.list(Cursor::string)?;
  cursor.expect(",")?;

  let mut env = BTreeMap::new();
  cursor.list(|cursor| {
    cursor.expect("(")?;
    let offset = cursor.offset;
    let key = cursor.string()?;
    cursor.expect(",")?;
    let value = cursor.bytes()?;
    cursor.expect(")")?;

    match env.entry(key) {
      Entry::Vacant(entry) => {
        entry.insert(value);
        Ok(())
      },
      Entry::Occupied(entry) => {
        Err(ParseError {
          offset,
          kind: ParseErrorKind::DuplicateEnvKey(entry.key().clone()),
        })
      },
    }
  })?;

  cursor.expect(")")?;
  if cursor.offset != content.len() {
    return Err(cursor.error(ParseErrorKind::TrailingData));
  }

  Ok(ParsedDerivation {
    outputs,
    input_drvs,
    dynamic_drvs,
    input_srcs,
    platform,
    builder,
    args,
    env,
  })
}

struct Cursor<'input> {
  input:  &'input [u8],
  offset: usize,
}

impl<'input> Cursor<'input> {
  const fn new(input: &'input [u8]) -> Self {
    Self { input, offset: 0 }
  }

  const fn error(&self, kind: ParseErrorKind) -> ParseError {
    ParseError {
      offset: self.offset,
      kind,
    }
  }

  fn rest(&self) -> &'input [u8] {
    self.input.get(self.offset..).unwrap_or_default()
  }

  fn eat(&mut self, token: &str) -> bool {
    let matched = self.rest().starts_with(token.as_bytes());
    if matched {
      self.offset += token.len();
    }
    matched
  }

  fn expect(&mut self, token: &'static str) -> Result<(), ParseError> {
    if self.eat(token) {
      Ok(())
    } else {
      Err(self.error(ParseErrorKind::Expected(token)))
    }
  }

  fn list<T>(
    &mut self,
    mut item: impl FnMut(&mut Self) -> Result<T, ParseError>,
  ) -> Result<Vec<T>, ParseError> {
    self.expect("[")?;
    let mut items = Vec::new();
    if self.eat("]") {
      return Ok(items);
    }

    loop {
      items.push(item(self)?);
      if !self.eat(",") {
        self.expect("]")?;
        return Ok(items);
      }
    }
  }

  /// Reads a quoted string, unescaping exactly as Nix's `parseString` does.
  fn bytes(&mut self) -> Result<Vec<u8>, ParseError> {
    self.expect("\"")?;
    let mut bytes = Vec::new();
    loop {
      let rest = self.rest();
      let run = rest
        .iter()
        .position(|&byte| byte == b'"' || byte == b'\\')
        .unwrap_or(rest.len());
      bytes.extend_from_slice(&rest[..run]);
      self.offset += run;

      match rest.get(run..) {
        Some(&[b'"', ..]) => {
          self.offset += 1;
          return Ok(bytes);
        },
        Some(&[b'\\', escaped, ..]) => {
          bytes.push(match escaped {
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            other => other,
          });
          self.offset += 2;
        },
        _ => return Err(self.error(ParseErrorKind::Expected("\""))),
      }
    }
  }

  /// Skips one `("name",[outputs])` or `("name",([own],[children]))` entry
  /// of a dynamic input.
  fn output_node(&mut self) -> Result<(), ParseError> {
    self.expect("(")?;
    self.bytes()?;
    self.expect(",")?;
    if self.eat("(") {
      self.list(Self::string)?;
      self.expect(",")?;
      self.list(Self::output_node)?;
      self.expect(")")?;
    } else {
      self.list(Self::string)?;
    }
    self.expect(")")
  }

  fn string(&mut self) -> Result<String, ParseError> {
    let offset = self.offset;
    String::from_utf8(self.bytes()?).map_err(|_| {
      ParseError {
        offset,
        kind: ParseErrorKind::InvalidUtf8,
      }
    })
  }
}

/// Extracts all input derivation paths from a `.drv` file.
///
/// # Errors
///
/// Returns [`Error`] if the file cannot be read or is not a valid derivation.
pub fn get_input_derivations<P: AsRef<Path>>(
  path: P,
) -> Result<Vec<String>, Error> {
  let parsed = parse_drv_file(path)?;
  Ok(
    parsed
      .input_drvs
      .into_iter()
      .map(|(path, _)| path)
      .collect(),
  )
}

/// Extracts `pname` from environment variables, replacing invalid UTF-8.
#[must_use]
pub fn extract_pname(env: &BTreeMap<String, Vec<u8>>) -> Option<String> {
  extract_env(env, "pname")
}

/// Extracts `version` from environment variables, replacing invalid UTF-8.
#[must_use]
pub fn extract_version(env: &BTreeMap<String, Vec<u8>>) -> Option<String> {
  extract_env(env, "version")
}

fn extract_env(env: &BTreeMap<String, Vec<u8>>, key: &str) -> Option<String> {
  env
    .get(key)
    .map(|value| String::from_utf8_lossy(value).into_owned())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_string() {
    let parse = |input: &[u8]| Cursor::new(input).string().unwrap();
    assert_eq!(parse(br#""hello""#), "hello");
    assert_eq!(parse(br#""hello world""#), "hello world");
    assert_eq!(parse(br#""hello\nworld""#), "hello\nworld");
  }

  #[test]
  fn parses_string_list() {
    let parse = |input: &[u8]| Cursor::new(input).list(Cursor::string);
    let result = parse(br#"["foo","bar","baz"]"#).unwrap();
    assert_eq!(result, vec!["foo", "bar", "baz"]);

    let result = parse(b"[]").unwrap();
    assert_eq!(result, Vec::<String>::new());
  }

  #[test]
  fn parse_minimal_drv() {
    let drv = br#"Derive([("out","/nix/store/output","","")],[],[],"x86_64-linux","/bin/sh",[],[("name","value")])"#;
    let result = parse_drv_content(drv).unwrap();
    assert_eq!(result.outputs.len(), 1);
    assert_eq!(result.outputs[0].0, "out");
    assert_eq!(result.platform, "x86_64-linux");
    assert_eq!(result.builder, "/bin/sh");
  }

  #[test]
  fn parse_with_dependencies() {
    let drv = br#"Derive([("out","/nix/store/abc-foo","","")],[("/nix/store/dep1.drv",["out"]),("/nix/store/dep2.drv",["out","dev"])],[],"x86_64-linux","/bin/sh",[],[("name","foo")])"#;
    let result = parse_drv_content(drv).unwrap();
    assert_eq!(result.input_drvs.len(), 2);
    assert_eq!(result.input_drvs[0].0, "/nix/store/dep1.drv");
    assert_eq!(result.input_drvs[0].1, vec!["out"]);
    assert_eq!(result.input_drvs[1].0, "/nix/store/dep2.drv");
    assert_eq!(result.input_drvs[1].1, vec!["out", "dev"]);
  }

  #[test]
  fn parse_real_world_hello_drv() {
    // Stripped down version of a real hello.drv
    let drv = br#"Derive([("out","/nix/store/00000000000000000000000000000001-hello-2.12.2","","")],[("/nix/store/00000000000000000000000000000002-hello-2.12.2.tar.gz.drv",["out"]),("/nix/store/00000000000000000000000000000003-bash-5.3p3.drv",["out"]),("/nix/store/00000000000000000000000000000004-version-check-hook.drv",["out"]),("/nix/store/00000000000000000000000000000005-stdenv-linux.drv",["out"])],["/nix/store/00000000000000000000000000000006-source-stdenv.sh","/nix/store/00000000000000000000000000000007-default-builder.sh"],"x86_64-linux","/nix/store/00000000000000000000000000000008-bash-5.3p3/bin/bash",["-e","/nix/store/00000000000000000000000000000006-source-stdenv.sh"],[("name","hello-2.12.2"),("pname","hello"),("version","2.12.2"),("system","x86_64-linux")])"#;

    let result = parse_drv_content(drv).unwrap();

    // Verify outputs
    assert_eq!(result.outputs.len(), 1);
    assert_eq!(result.outputs[0].0, "out");
    assert!(result.outputs[0].1.contains("hello-2.12.2"));

    // Verify input derivations
    assert_eq!(result.input_drvs.len(), 4);
    assert!(result.input_drvs[0].0.contains("hello-2.12.2.tar.gz.drv"));
    assert!(result.input_drvs[1].0.contains("bash-5.3p3.drv"));
    assert!(result.input_drvs[2].0.contains("version-check-hook.drv"));
    assert!(result.input_drvs[3].0.contains("stdenv-linux.drv"));

    // Verify all inputs have "out" output
    for (_, outputs) in &result.input_drvs {
      assert_eq!(outputs, &vec!["out"]);
    }

    // Verify platform
    assert_eq!(result.platform, "x86_64-linux");

    // Verify builder
    assert!(result.builder.contains("bash"));

    // Verify environment
    assert_eq!(extract_pname(&result.env), Some("hello".to_owned()));
    assert_eq!(extract_version(&result.env), Some("2.12.2".to_owned()));
  }

  #[test]
  fn gets_input_derivations() {
    let drv = br#"Derive([("out","/nix/store/out","","")],[("/nix/store/dep.drv",["out"])],[],"x86_64-linux","/bin/sh",[],[("pname","hello"),("version","1.0")])"#;
    let result = parse_drv_content(drv).unwrap();
    assert_eq!(result.input_drvs.len(), 1);
    assert_eq!(result.input_drvs[0].0, "/nix/store/dep.drv");
    assert_eq!(extract_pname(&result.env).unwrap(), "hello");
    assert_eq!(extract_version(&result.env).unwrap(), "1.0");
  }

  #[test]
  fn keeps_env_bytes() {
    let drv =
      b"Derive([],[],[],\"x\",\"/bin/sh\",[],[(\"pname\",\"\xFF\\n\")])";
    let result = parse_drv_content(drv).unwrap();
    assert_eq!(result.env["pname"], b"\xFF\n");
  }

  #[test]
  fn rejects_duplicate_env_key() {
    let drv = br#"Derive([],[],[],"x","/bin/sh",[],[("a","1"),("a","2")])"#;
    let error = parse_drv_content(drv).unwrap_err();
    assert_eq!(error.kind, ParseErrorKind::DuplicateEnvKey("a".to_owned()));
  }

  #[test]
  fn unescapes_like_nix() {
    let mut cursor = Cursor::new(br#""\q\\\"\t""#);
    assert_eq!(cursor.bytes().unwrap(), b"q\\\"\t");
    let error = Cursor::new(br#""\"#).bytes().unwrap_err();
    assert_eq!(error.kind, ParseErrorKind::Expected("\""));
  }
}
