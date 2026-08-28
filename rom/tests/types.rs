use rom::types::{Config, DisplayFormat, InputMode, LogPrefixStyle};

#[test]
fn test_config_default() {
  let config = Config::default();
  assert!(!config.engine.silent);
  assert_eq!(config.engine.input_mode, InputMode::Auto);
  assert!(config.render.show_timers);
  assert_eq!(config.render.format, DisplayFormat::Tree);
  assert_eq!(config.engine.log_prefix_style, LogPrefixStyle::Short);
  assert_eq!(config.engine.log_line_limit, None);
}

#[test]
fn test_input_mode_comparison() {
  assert_eq!(InputMode::Json, InputMode::Json);
  assert_ne!(InputMode::Json, InputMode::Auto);
}
