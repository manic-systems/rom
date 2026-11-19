//! The config file and the flags that must work regardless of it.

#![cfg(feature = "cli")]
#![expect(
  clippy::tests_outside_test_module,
  reason = "integration tests are their own crate"
)]

use std::{
  fs,
  io::Write,
  process::{Command, Stdio},
};

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

#[test]
fn config_file_supports_nom_style_and_toggle_options() {
  let directory = tempfile::tempdir().unwrap();
  fs::create_dir_all(directory.path().join("rom")).unwrap();
  fs::write(
    directory.path().join("rom").join("config.toml"),
    "\
tree-direction = \"up\"
legend = \"nom\"
summary = \"nom\"
icons = \"unicode\"
host-sort = \"download-size\"
max-hosts = 2
curl-progress = true
progress-bars = false
show-percentages = false
decimal-bytes = true
",
  )
  .unwrap();

  let mut child = Command::new(env!("CARGO_BIN_EXE_rom"))
    .env("XDG_CONFIG_HOME", directory.path())
    .env("XDG_STATE_HOME", directory.path())
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
  let mut stdin = child.stdin.take().unwrap();
  writeln!(
    stdin,
    r#"@nix {{"action":"start","id":1,"level":3,"parent":0,"text":"building","type":105,"fields":["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-archive.tar.gz.drv","",1,1]}}"#
  )
  .unwrap();
  writeln!(
    stdin,
    r#"@nix {{"action":"result","id":1,"type":101,"fields":["trying https://cdn.example.com/archive.tar.gz"]}}"#
  )
  .unwrap();
  writeln!(
    stdin,
    r#"@nix {{"action":"result","id":1,"type":101,"fields":["100 60.0M  100 60.0M    0     0  10.0M      0  0:00:06  0:00:06 --:--:-- 10.0M"]}}"#
  )
  .unwrap();
  writeln!(stdin, r#"@nix {{"action":"stop","id":1}}"#).unwrap();
  drop(stdin);
  let output = child.wait_with_output().unwrap();
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(output.status.success(), "{stderr}");
  assert!(stderr.contains("cdn.example.com (https)"), "{stderr}");
  assert!(stderr.contains("60.0 MiB/60.0 MiB"), "{stderr}");
  assert!(stderr.contains("┗━ ∑"), "{stderr}");
}

#[test]
fn config_file_parses_theme_colors_and_rejects_invalid_values() {
  use rom::{
    cli::ThemeOptions,
    types::{Color, Theme},
  };

  let mut theme = Theme::default();
  let parsed: ThemeOptions = toml::from_str(
    "\
connector = \"white\"
log-prefix = \"blue\"
host = \"#ff8800\"
muted = \"240\"
",
  )
  .unwrap();
  parsed.apply(&mut theme);
  assert_eq!(theme.connector, Color::White);
  assert_eq!(theme.log_prefix, Color::Blue);
  assert_eq!(theme.host, Color::Rgb(0xFF, 0x88, 0x00));
  assert_eq!(theme.muted, Color::Indexed(240));
  assert_eq!(theme.running, Theme::default().running);

  let directory = tempfile::tempdir().unwrap();
  fs::create_dir_all(directory.path().join("rom")).unwrap();
  fs::write(
    directory.path().join("rom").join("config.toml"),
    "[theme]\nconnector = \"not-a-color\"\n",
  )
  .unwrap();
  let run = Command::new(env!("CARGO_BIN_EXE_rom"))
    .env("XDG_CONFIG_HOME", directory.path())
    .env("XDG_STATE_HOME", directory.path())
    .output()
    .unwrap();
  let stderr = String::from_utf8_lossy(&run.stderr);
  assert!(!run.status.success(), "{stderr}");
  assert!(stderr.contains("not-a-color"), "{stderr}");

  fs::write(
    directory.path().join("rom").join("config.toml"),
    "[theme]\nunknown-slot = \"blue\"\n",
  )
  .unwrap();
  let run = Command::new(env!("CARGO_BIN_EXE_rom"))
    .env("XDG_CONFIG_HOME", directory.path())
    .env("XDG_STATE_HOME", directory.path())
    .output()
    .unwrap();
  let stderr = String::from_utf8_lossy(&run.stderr);
  assert!(!run.status.success(), "{stderr}");
  assert!(stderr.contains("unknown field `unknown-slot`"), "{stderr}");
}
