use rom::cli::{parse_args_with_separator, replace_command_with_exit};

#[test]
fn test_replace_command_with_exit() {
  let args = vec![
    "nixpkgs#hello".to_string(),
    "--command".to_string(),
    "bash".to_string(),
  ];

  let result = replace_command_with_exit(&args);
  assert_eq!(result[0], "nixpkgs#hello");
  assert!(result.contains(&"--command".to_string()));
  assert!(result.contains(&"exit".to_string()));
  assert!(!result.contains(&"bash".to_string()));
}

#[test]
fn test_replace_command_short_form() {
  let args = vec![
    "nixpkgs#hello".to_string(),
    "-c".to_string(),
    "echo test".to_string(),
  ];

  let result = replace_command_with_exit(&args);
  assert_eq!(result[0], "nixpkgs#hello");
  assert!(result.contains(&"exit".to_string()));
  assert!(!result.contains(&"echo test".to_string()));
}

#[test]
fn test_parse_args_with_separator() {
  // Test with separator
  let args = vec![
    "nixpkgs#hello".to_string(),
    "--".to_string(),
    "--help".to_string(),
  ];
  let (before, after) = parse_args_with_separator(&args);
  assert_eq!(before, vec!["nixpkgs#hello".to_string()]);
  assert_eq!(after, vec!["--help".to_string()]);

  // Test without separator
  let args = vec!["nixpkgs#hello".to_string(), "--help".to_string()];
  let (before, after) = parse_args_with_separator(&args);
  assert_eq!(before, vec![
    "nixpkgs#hello".to_string(),
    "--help".to_string()
  ]);
  assert_eq!(after, Vec::<String>::new());

  // Test with multiple nix args after separator
  let args = vec![
    "nixpkgs#hello".to_string(),
    "--".to_string(),
    "--option".to_string(),
    "foo".to_string(),
    "bar".to_string(),
  ];
  let (before, after) = parse_args_with_separator(&args);
  assert_eq!(before, vec!["nixpkgs#hello".to_string()]);
  assert_eq!(after, vec![
    "--option".to_string(),
    "foo".to_string(),
    "bar".to_string()
  ]);

  // Test with only separator
  let args = vec!["--".to_string(), "--help".to_string()];
  let (before, after) = parse_args_with_separator(&args);
  assert_eq!(before, Vec::<String>::new());
  assert_eq!(after, vec!["--help".to_string()]);
}
