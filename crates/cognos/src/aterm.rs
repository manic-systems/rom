//! Nix `.drv` parsing backed by the `nix-derivation` crate.

use std::{fs, path::Path};

use nix_derivation::Derivation;

/// Parsed derivation information used by ROM's graph model.
#[derive(Debug, Clone)]
pub struct ParsedDerivation {
  pub outputs:    Vec<(String, String)>,
  pub input_drvs: Vec<(String, Vec<String>)>,
  pub input_srcs: Vec<String>,
  pub platform:   String,
  pub builder:    String,
  pub args:       Vec<String>,
  pub env:        Vec<(String, String)>,
}

/// Parse a `.drv` file and extract its dependency information.
pub fn parse_drv_file<P: AsRef<Path>>(
  path: P,
) -> Result<ParsedDerivation, String> {
  let path = path.as_ref();
  let content =
    fs::read(path).map_err(|error| format!("Failed to read file: {error}"))?;
  let name = path
    .file_stem()
    .and_then(|name| name.to_str())
    .unwrap_or("unknown");
  parse_drv_bytes(&content, name)
}

/// Parse textual `.drv` content.
pub fn parse_drv_content(content: &str) -> Result<ParsedDerivation, String> {
  parse_drv_bytes(content.as_bytes(), "unknown")
}

fn parse_drv_bytes(
  content: &[u8],
  name: &str,
) -> Result<ParsedDerivation, String> {
  let derivation = Derivation::from_aterm_bytes(content, name)
    .map_err(|error| error.to_string())?;
  let store_dir = derivation.store_dir();
  let outputs = derivation
    .resolved_outputs()
    .map_err(|error| error.to_string())?
    .into_iter()
    .map(|(name, output)| {
      let path = output
        .path
        .map_or_else(String::new, |path| path.to_absolute_path_in(store_dir));
      (name, path)
    })
    .collect();
  let input_drvs = derivation
    .input_derivations()
    .iter()
    .map(|(path, input)| {
      (
        path.to_absolute_path_in(store_dir),
        input.outputs().iter().cloned().collect(),
      )
    })
    .collect();
  let input_srcs = derivation
    .input_sources()
    .iter()
    .map(|path| path.to_absolute_path_in(store_dir))
    .collect();
  let env = derivation
    .environment()
    .iter()
    .map(|(key, value)| {
      (key.clone(), String::from_utf8_lossy(value).into_owned())
    })
    .collect();

  Ok(ParsedDerivation {
    outputs,
    input_drvs,
    input_srcs,
    platform: derivation.system().to_string(),
    builder: derivation.builder().to_string(),
    args: derivation.arguments().to_vec(),
    env,
  })
}

/// Extract all input derivation paths from a `.drv` file.
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

/// Extract `pname` from environment variables.
#[must_use]
pub fn extract_pname(env: &[(String, String)]) -> Option<String> {
  extract_env(env, "pname")
}

/// Extract `version` from environment variables.
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

  const OUT: &str = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-output";
  const DEP1: &str = "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-dep1.drv";
  const DEP2: &str = "/nix/store/cccccccccccccccccccccccccccccccc-dep2.drv";

  #[test]
  fn parses_derivation_metadata() {
    let drv = format!(
      r#"Derive([("out","{OUT}","","")],[("{DEP1}",["out"]),("{DEP2}",["dev","out"])],[],"x86_64-linux","/bin/sh",["-e"],[("pname","hello"),("version","1.0")])"#
    );
    let result = parse_drv_content(&drv).unwrap();

    assert_eq!(result.outputs, vec![("out".to_string(), OUT.to_string())]);
    assert_eq!(result.input_drvs.len(), 2);
    assert_eq!(result.input_drvs[0].0, DEP1);
    assert_eq!(result.input_drvs[0].1, vec!["out"]);
    assert_eq!(result.input_drvs[1].0, DEP2);
    assert_eq!(result.input_drvs[1].1, vec!["dev", "out"]);
    assert_eq!(result.platform, "x86_64-linux");
    assert_eq!(result.builder, "/bin/sh");
    assert_eq!(result.args, vec!["-e"]);
    assert_eq!(extract_pname(&result.env).as_deref(), Some("hello"));
    assert_eq!(extract_version(&result.env).as_deref(), Some("1.0"));
  }

  #[test]
  fn parses_nix_string_escapes() {
    let drv = format!(
      r#"Derive([("out","{OUT}","","")],[],[],"x86_64-linux","/bin/sh",[],[("value","hello\nworld")])"#
    );
    let result = parse_drv_content(&drv).unwrap();
    assert_eq!(
      extract_env(&result.env, "value").as_deref(),
      Some("hello\nworld")
    );
  }
}
