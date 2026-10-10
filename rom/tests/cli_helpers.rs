//! Argument rewriting helpers used by the CLI wrappers.

#![cfg(feature = "cli")]
#![expect(
  clippy::tests_outside_test_module,
  reason = "integration tests are their own crate"
)]

use rom::cli::{parse_args_with_separator, replace_command_with_exit};

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

#[test]
fn splits_args_at_separator() {
  // Test with separator
  let args = vec![
    "nixpkgs#hello".to_owned(),
    "--".to_owned(),
    "--help".to_owned(),
  ];
  let (before, after) = parse_args_with_separator(&args);
  assert_eq!(before, vec!["nixpkgs#hello".to_owned()]);
  assert_eq!(after, vec!["--help".to_owned()]);

  // Test without separator
  let args = vec!["nixpkgs#hello".to_owned(), "--help".to_owned()];
  let (before, after) = parse_args_with_separator(&args);
  assert_eq!(before, vec![
    "nixpkgs#hello".to_owned(),
    "--help".to_owned()
  ]);
  assert_eq!(after, Vec::<String>::new());

  // Test with multiple nix args after separator
  let args = vec![
    "nixpkgs#hello".to_owned(),
    "--".to_owned(),
    "--option".to_owned(),
    "foo".to_owned(),
    "bar".to_owned(),
  ];
  let (before, after) = parse_args_with_separator(&args);
  assert_eq!(before, vec!["nixpkgs#hello".to_owned()]);
  assert_eq!(after, vec![
    "--option".to_owned(),
    "foo".to_owned(),
    "bar".to_owned()
  ]);

  // Test with only separator
  let args = vec!["--".to_owned(), "--help".to_owned()];
  let (before, after) = parse_args_with_separator(&args);
  assert_eq!(before, Vec::<String>::new());
  assert_eq!(after, vec!["--help".to_owned()]);
}
