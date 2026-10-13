//! Argument rewriting helpers used by the CLI wrappers.

#![cfg(feature = "cli")]
#![expect(
  clippy::tests_outside_test_module,
  reason = "integration tests are their own crate"
)]

use rom::cli::replace_command_with_exit;

#[test]
fn replaces_command_with_exit() {
  let args = vec![
    "nixpkgs#hello".to_owned(),
    "--command".to_owned(),
    "bash".to_owned(),
  ];

  let result = replace_command_with_exit(&args);
  assert_eq!(result[0], "nixpkgs#hello");
  assert!(result.contains(&"--command".to_owned()));
  assert!(result.contains(&"exit".to_owned()));
  assert!(!result.contains(&"bash".to_owned()));
}

#[test]
fn replace_command_short_form() {
  let args = vec![
    "nixpkgs#hello".to_owned(),
    "-c".to_owned(),
    "echo test".to_owned(),
  ];

  let result = replace_command_with_exit(&args);
  assert_eq!(result[0], "nixpkgs#hello");
  assert!(result.contains(&"exit".to_owned()));
  assert!(!result.contains(&"echo test".to_owned()));
}
