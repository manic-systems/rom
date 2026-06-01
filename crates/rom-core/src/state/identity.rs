use std::path::PathBuf;

/// Store path representation
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorePath {
  pub path: PathBuf,
  pub hash: String,
  pub name: String,
}

impl StorePath {
  #[must_use]
  pub fn parse(path: &str) -> Option<Self> {
    if !path.starts_with("/nix/store/") {
      return None;
    }

    let path_buf = PathBuf::from(path);
    let file_name = path_buf.file_name()?.to_str()?.to_string();

    let parts: Vec<&str> = file_name.splitn(2, '-').collect();
    if parts.len() != 2 {
      return None;
    }

    Some(Self {
      path: path_buf,
      hash: parts[0].to_string(),
      name: parts[1].to_string(),
    })
  }
}

/// Derivation representation
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Derivation {
  pub path: PathBuf,
  pub name: String,
}

impl Derivation {
  #[must_use]
  pub fn parse(path: &str) -> Option<Self> {
    let path_buf = PathBuf::from(path);
    let file_name = path_buf.file_name()?.to_str()?.to_string();

    if !file_name.ends_with(".drv") {
      return None;
    }

    let name = file_name.strip_suffix(".drv")?;
    let parts: Vec<&str> = name.splitn(2, '-').collect();
    let display_name = if parts.len() == 2 {
      parts[1].to_string()
    } else {
      name.to_string()
    };

    Some(Self {
      path: path_buf,
      name: display_name,
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn store_path_parse_splits_hash_and_name() {
    let path = "/nix/store/abc123-hello-1.0";
    let store_path = StorePath::parse(path).unwrap();

    assert_eq!(store_path.hash, "abc123");
    assert_eq!(store_path.name, "hello-1.0");
  }

  #[test]
  fn derivation_parse_uses_display_name_without_store_hash() {
    let path = "/nix/store/abc123-hello-1.0.drv";
    let derivation = Derivation::parse(path).unwrap();

    assert_eq!(derivation.name, "hello-1.0");
  }
}
