//! `ATerm` and Nix .drv file parser
//!
//! Parses Nix .drv files in `ATerm` format to extract dependency information.
use std::{fs, path::Path};

/// Parsed derivation information from a .drv file
#[derive(Debug, Clone)]
pub struct ParsedDerivation {
  /// Output names paired with their store paths.
  pub outputs:    Vec<(String, String)>,
  /// Input derivation paths paired with the outputs used from each.
  pub input_drvs: Vec<(String, Vec<String>)>,
  /// Source store paths used as inputs.
  pub input_srcs: Vec<String>,
  /// System the derivation builds for, such as `x86_64-linux`.
  pub platform:   String,
  /// Path of the builder executable.
  pub builder:    String,
  /// Arguments passed to the builder.
  pub args:       Vec<String>,
  /// Environment variables set for the builder.
  pub env:        Vec<(String, String)>,
}

/// Parse a .drv file and extract its dependency information
///
/// # Errors
///
/// Returns a description of the failure if the file cannot be read or is not a
/// valid derivation.
pub fn parse_drv_file<P: AsRef<Path>>(
  path: P,
) -> Result<ParsedDerivation, String> {
  let content = fs::read_to_string(path)
    .map_err(|e| format!("Failed to read file: {e}"))?;
  parse_drv_content(&content)
}

/// Parse the content of a .drv file
///
/// # Errors
///
/// Returns a description of the failure if `content` is not a valid derivation.
pub fn parse_drv_content(content: &str) -> Result<ParsedDerivation, String> {
  let content = content.trim();

  if !content.starts_with("Derive(") {
    return Err(
      "Invalid derivation format: must start with 'Derive('".to_string(),
    );
  }

  let inner = content
    .strip_prefix("Derive(")
    .and_then(|s| s.strip_suffix(")"))
    .ok_or("Invalid derivation format: missing closing parenthesis")?;

  // XXX: The derivation has this structure:
  // Derive(outputs, inputDrvs, inputSrcs, platform, builder, args, env)
  let parts = parse_top_level_list(inner);

  let [
    outputs,
    input_drvs,
    input_srcs,
    platform,
    builder,
    args,
    env,
    ..,
  ] = parts.as_slice()
  else {
    return Err(format!(
      "Invalid derivation format: expected 7 parts, got {}",
      parts.len()
    ));
  };

  let outputs = parse_outputs(outputs)?;
  let input_drvs = parse_input_drvs(input_drvs)?;
  let input_srcs = parse_string_list(input_srcs)?;
  let platform = parse_string(platform)?;
  let builder = parse_string(builder)?;
  let args = parse_string_list(args)?;
  let env = parse_env(env)?;

  Ok(ParsedDerivation {
    outputs,
    input_drvs,
    input_srcs,
    platform,
    builder,
    args,
    env,
  })
}

/// Parse the top-level comma-separated list, respecting nested brackets
fn parse_top_level_list(s: &str) -> Vec<String> {
  let mut parts = Vec::new();
  let mut current = String::new();
  let mut depth = 0;
  let mut in_string = false;
  let mut escape = false;

  for ch in s.chars() {
    if escape {
      current.push(ch);
      escape = false;
      continue;
    }

    match ch {
      '\\' if in_string => {
        escape = true;
        current.push(ch);
      },
      '"' => {
        in_string = !in_string;
        current.push(ch);
      },
      '[' | '(' if !in_string => {
        depth += 1;
        current.push(ch);
      },
      ']' | ')' if !in_string => {
        depth -= 1;
        current.push(ch);
      },
      ',' if depth == 0 && !in_string => {
        parts.push(current.trim().to_string());
        current.clear();
      },
      _ => {
        current.push(ch);
      },
    }
  }

  if !current.trim().is_empty() {
    parts.push(current.trim().to_string());
  }

  parts
}

fn parse_list(s: &str, error: &'static str) -> Result<Vec<String>, String> {
  let inner = s
    .trim()
    .strip_prefix('[')
    .and_then(|s| s.strip_suffix(']'))
    .ok_or(error)?;

  Ok(parse_top_level_list(inner))
}

fn parse_tuple(s: &str, error: &'static str) -> Result<Vec<String>, String> {
  let inner = s
    .trim()
    .strip_prefix('(')
    .and_then(|s| s.strip_suffix(')'))
    .ok_or(error)?;

  Ok(parse_top_level_list(inner))
}

fn parse_tuple_list<T>(
  s: &str,
  list_error: &'static str,
  tuple_error: &'static str,
  mut parse_item: impl FnMut(&[String]) -> Result<Option<T>, String>,
) -> Result<Vec<T>, String> {
  let mut items = Vec::new();

  for tuple in parse_list(s, list_error)? {
    if let Some(item) = parse_item(&parse_tuple(&tuple, tuple_error)?)? {
      items.push(item);
    }
  }

  Ok(items)
}

/// Parse outputs: [("out","/nix/store/...","",""),...]
fn parse_outputs(s: &str) -> Result<Vec<(String, String)>, String> {
  parse_tuple_list(
    s,
    "Invalid outputs format",
    "Invalid output tuple format",
    parse_string_pair,
  )
}

/// Parse input derivations: [("/nix/store/foo.drv",["out"]),...]
fn parse_input_drvs(s: &str) -> Result<Vec<(String, Vec<String>)>, String> {
  parse_tuple_list(
    s,
    "Invalid input drvs format",
    "Invalid input drv tuple format",
    |parts| {
      if parts.len() < 2 {
        return Ok(None);
      }

      Ok(Some((
        parse_string(&parts[0])?,
        parse_string_list(&parts[1])?,
      )))
    },
  )
}

/// Parse environment variables: [("name","value"),...]
fn parse_env(s: &str) -> Result<Vec<(String, String)>, String> {
  parse_tuple_list(
    s,
    "Invalid env format",
    "Invalid env tuple format",
    parse_string_pair,
  )
}

fn parse_string_pair(
  parts: &[String],
) -> Result<Option<(String, String)>, String> {
  if parts.len() < 2 {
    return Ok(None);
  }

  Ok(Some((parse_string(&parts[0])?, parse_string(&parts[1])?)))
}

/// Parse a list of strings: ["foo","bar",...]
fn parse_string_list(s: &str) -> Result<Vec<String>, String> {
  parse_list(s, "Invalid string list format")?
    .into_iter()
    .map(|item| parse_string(&item))
    .collect()
}

/// Parse a quoted string: "foo" -> foo
fn parse_string(s: &str) -> Result<String, String> {
  let s = s.trim();
  let inner = s
    .strip_prefix('"')
    .and_then(|s| s.strip_suffix('"'))
    .ok_or_else(|| format!("Invalid string format: {s}"))?;

  // Unescape the string
  Ok(unescape_string(inner))
}

/// Unescape a string (handle \n, \t, \\, \", etc.)
fn unescape_string(s: &str) -> String {
  let mut result = String::new();
  let mut chars = s.chars();

  while let Some(ch) = chars.next() {
    if ch == '\\' {
      match chars.next() {
        Some('n') => result.push('\n'),
        Some('t') => result.push('\t'),
        Some('r') => result.push('\r'),
        Some('\\') => result.push('\\'),
        Some('"') => result.push('"'),
        Some(c) => {
          result.push('\\');
          result.push(c);
        },
        None => result.push('\\'),
      }
    } else {
      result.push(ch);
    }
  }

  result
}

/// Extract all input derivation paths from a .drv file
///
/// # Errors
///
/// Returns a description of the failure if the file cannot be read or is not a
/// valid derivation.
pub fn get_input_derivations<P: AsRef<Path>>(
  path: P,
) -> Result<Vec<String>, String> {
  let parsed = parse_drv_file(path)?;
  Ok(
    parsed
      .input_drvs
      .into_iter()
      .map(|(path, _)| path)
      .collect(),
  )
}

/// Extract pname from environment variables
#[must_use]
pub fn extract_pname(env: &[(String, String)]) -> Option<String> {
  extract_env(env, "pname")
}

/// Extract version from environment variables
#[must_use]
pub fn extract_version(env: &[(String, String)]) -> Option<String> {
  extract_env(env, "version")
}

fn extract_env(env: &[(String, String)], key: &str) -> Option<String> {
  env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_parse_string() {
    assert_eq!(parse_string(r#""hello""#).unwrap(), "hello");
    assert_eq!(parse_string(r#""hello world""#).unwrap(), "hello world");
    assert_eq!(parse_string(r#""hello\nworld""#).unwrap(), "hello\nworld");
  }

  #[test]
  fn test_parse_string_list() {
    let list = r#"["foo","bar","baz"]"#;
    let result = parse_string_list(list).unwrap();
    assert_eq!(result, vec!["foo", "bar", "baz"]);

    let empty = "[]";
    let result = parse_string_list(empty).unwrap();
    assert_eq!(result, Vec::<String>::new());
  }

  #[test]
  fn test_parse_outputs() {
    let outputs = r#"[("out","/nix/store/abc-foo","","")]"#;
    let result = parse_outputs(outputs).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "out");
    assert_eq!(result[0].1, "/nix/store/abc-foo");
  }

  #[test]
  fn test_parse_input_drvs() {
    let input = r#"[("/nix/store/abc-foo.drv",["out"]),("/nix/store/def-bar.drv",["out","dev"])]"#;
    let result = parse_input_drvs(input).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].0, "/nix/store/abc-foo.drv");
    assert_eq!(result[0].1, vec!["out"]);
    assert_eq!(result[1].0, "/nix/store/def-bar.drv");
    assert_eq!(result[1].1, vec!["out", "dev"]);
  }

  #[test]
  fn test_parse_minimal_drv() {
    let drv = r#"Derive([("out","/nix/store/output","","")],[],[],"x86_64-linux","/bin/sh",[],[("name","value")])"#;
    let result = parse_drv_content(drv).unwrap();
    assert_eq!(result.outputs.len(), 1);
    assert_eq!(result.outputs[0].0, "out");
    assert_eq!(result.platform, "x86_64-linux");
    assert_eq!(result.builder, "/bin/sh");
  }

  #[test]
  fn test_parse_with_dependencies() {
    let drv = r#"Derive([("out","/nix/store/abc-foo","","")],[("/nix/store/dep1.drv",["out"]),("/nix/store/dep2.drv",["out","dev"])],[],"x86_64-linux","/bin/sh",[],[("name","foo")])"#;
    let result = parse_drv_content(drv).unwrap();
    assert_eq!(result.input_drvs.len(), 2);
    assert_eq!(result.input_drvs[0].0, "/nix/store/dep1.drv");
    assert_eq!(result.input_drvs[0].1, vec!["out"]);
    assert_eq!(result.input_drvs[1].0, "/nix/store/dep2.drv");
    assert_eq!(result.input_drvs[1].1, vec!["out", "dev"]);
  }

  #[test]
  fn test_parse_real_world_hello_drv() {
    // Stripped down version of a real hello.drv
    let drv = r#"Derive([("out","/nix/store/00000000000000000000000000000001-hello-2.12.2","","")],[("/nix/store/00000000000000000000000000000002-hello-2.12.2.tar.gz.drv",["out"]),("/nix/store/00000000000000000000000000000003-bash-5.3p3.drv",["out"]),("/nix/store/00000000000000000000000000000004-version-check-hook.drv",["out"]),("/nix/store/00000000000000000000000000000005-stdenv-linux.drv",["out"])],["/nix/store/00000000000000000000000000000006-source-stdenv.sh","/nix/store/00000000000000000000000000000007-default-builder.sh"],"x86_64-linux","/nix/store/00000000000000000000000000000008-bash-5.3p3/bin/bash",["-e","/nix/store/00000000000000000000000000000006-source-stdenv.sh"],[("name","hello-2.12.2"),("pname","hello"),("version","2.12.2"),("system","x86_64-linux")])"#;

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
    assert_eq!(extract_pname(&result.env), Some("hello".to_string()));
    assert_eq!(extract_version(&result.env), Some("2.12.2".to_string()));
  }

  #[test]
  fn test_get_input_derivations() {
    let drv = r#"Derive([("out","/nix/store/out","","")],[("/nix/store/dep.drv",["out"])],[],"x86_64-linux","/bin/sh",[],[("pname","hello"),("version","1.0")])"#;
    let result = parse_drv_content(drv).unwrap();
    assert_eq!(result.input_drvs.len(), 1);
    assert_eq!(result.input_drvs[0].0, "/nix/store/dep.drv");
    assert_eq!(extract_pname(&result.env).unwrap(), "hello");
    assert_eq!(extract_version(&result.env).unwrap(), "1.0");
  }
}
