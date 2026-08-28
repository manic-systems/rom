#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn wrapped_stdout_is_exact_and_child_status_is_authoritative() {
  let directory = tempfile::tempdir().unwrap();
  let nix = directory.path().join("nix");
  fs::write(
    &nix,
    concat!(
      "#!/bin/sh\n",
      "printf '/nix/store/exact-without-newline'\n",
      "printf '%s\\n' '@nix {\"action\":\"msg\",\"level\":3,\"msg\":\"from \
       child\"}' >&2\n",
      "exit 7\n",
    ),
  )
  .unwrap();
  fs::set_permissions(&nix, fs::Permissions::from_mode(0o755)).unwrap();

  let output = Command::new(env!("CARGO_BIN_EXE_rom"))
    .args(["--platform", "nix", "build", "example"])
    .env("PATH", directory.path())
    .env("XDG_STATE_HOME", directory.path())
    .output()
    .unwrap();

  assert_eq!(output.stdout, b"/nix/store/exact-without-newline");
  assert_eq!(output.status.code(), Some(7));
  assert!(String::from_utf8_lossy(&output.stderr).contains("from child"));
}

#[test]
fn silent_shell_still_runs_the_real_second_pass() {
  let directory = tempfile::tempdir().unwrap();
  let nix = directory.path().join("nix");
  let calls = directory.path().join("calls");
  fs::write(
    &nix,
    concat!(
      "#!/bin/sh\n",
      "printf '%s\\n' \"$*\" >> \"$ROM_TEST_CALLS\"\n",
      "exit 0\n",
    ),
  )
  .unwrap();
  fs::set_permissions(&nix, fs::Permissions::from_mode(0o755)).unwrap();

  let output = Command::new(env!("CARGO_BIN_EXE_rom"))
    .args(["--platform", "nix", "--silent", "shell", "example"])
    .env("PATH", directory.path())
    .env("ROM_TEST_CALLS", &calls)
    .env("XDG_STATE_HOME", directory.path())
    .output()
    .unwrap();
  assert!(output.status.success());
  let calls = fs::read_to_string(calls).unwrap();
  assert_eq!(calls.lines().count(), 2, "{calls}");
  assert!(
    calls
      .lines()
      .next()
      .unwrap()
      .contains("--command sh -c exit")
  );
  assert_eq!(calls.lines().nth(1).unwrap(), "shell example");
}
