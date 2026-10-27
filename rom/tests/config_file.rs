//! The config file and the flags that must work regardless of it.

#![cfg(feature = "cli")]
#![expect(
  clippy::tests_outside_test_module,
  reason = "integration tests are their own crate"
)]

use std::{fs, process::Command};

#[test]
fn unknown_config_keys_do_not_block_help_or_version() {
  let directory = tempfile::tempdir().unwrap();
  fs::create_dir_all(directory.path().join("rom")).unwrap();
  fs::write(
    directory.path().join("rom").join("config.toml"),
    "from-a-newer-rom = true\n",
  )
  .unwrap();
  let rom = |arguments: &[&str]| {
    Command::new(env!("CARGO_BIN_EXE_rom"))
      .args(arguments)
      .env("XDG_CONFIG_HOME", directory.path())
      .env("XDG_STATE_HOME", directory.path())
      .output()
      .unwrap()
  };

  assert!(rom(&["--help"]).status.success());
  assert!(rom(&["--version"]).status.success());
  let run = rom(&[]);
  let stderr = String::from_utf8_lossy(&run.stderr);
  assert!(!run.status.success(), "{stderr}");
  assert!(
    stderr.contains("unknown field `from-a-newer-rom`"),
    "{stderr}"
  );
}
