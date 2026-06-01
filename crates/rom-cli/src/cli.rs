//! CLI interface for ROM
mod tui_runtime;

use std::{
  collections::HashMap,
  io::{self, BufRead, BufReader, IsTerminal, Read, Write},
  path::PathBuf,
  process::{Child, Command, Stdio},
  sync::{
    Arc,
    Mutex,
    atomic::{AtomicBool, Ordering},
  },
  thread,
  time::Duration,
};

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::log_store::{
  DEFAULT_TUI_LOG_LINE_LIMIT,
  LogStore,
  post_tui_failure_error_lines,
};

pub(super) const DEPENDENCY_POPULATE_BUDGET_PER_FRAME: usize = 1;

#[derive(Debug, Parser)]
#[command(name = "rom", version, about = "ROM - A Nix build output monitor")]
pub struct Cli {
  #[command(subcommand)]
  pub command: Option<Commands>,

  /// Parse JSON output from nix --log-format=internal-json
  #[arg(long, global = true)]
  pub json: bool,

  /// Minimal output
  #[arg(long, global = true)]
  pub silent: bool,

  /// Output format: tree, plain, dashboard
  #[arg(long, global = true, default_value = "tree")]
  pub format: String,

  /// Legend display style: compact, table, verbose
  #[arg(long, global = true, default_value = "table")]
  pub legend: String,

  /// Summary display style: concise, table, full
  #[arg(long, global = true, default_value = "concise")]
  pub summary: String,

  /// Log prefix style: short, full, none
  #[arg(long, global = true, default_value = "short")]
  pub log_prefix: String,

  /// Maximum number of log lines to display
  #[arg(long, global = true)]
  pub log_lines: Option<usize>,

  /// Nix-family evaluator to use. Auto-detected by default
  #[arg(long, global = true)]
  pub platform: Option<String>,

  /// Increase verbosity; controls nix log level and rom diagnostic output.
  /// Repeatable: -v (info), -vv (debug), -vvv (trace)
  #[arg(short = 'v', action = clap::ArgAction::Count, global = true)]
  pub verbose: u8,
}

#[derive(Debug, clap::Subcommand)]
pub enum Commands {
  /// Run nix build with monitoring
  Build {
    /// Packages or flake expressions to build
    packages: Vec<String>,

    /// Extra flags to pass directly to nix
    #[arg(last = true)]
    nix_flags: Vec<String>,
  },

  /// Run nix shell with monitoring
  Shell {
    /// Packages or flake expressions
    packages: Vec<String>,

    /// Extra flags to pass directly to nix
    #[arg(last = true)]
    nix_flags: Vec<String>,
  },

  /// Run nix develop with monitoring
  Develop {
    /// Packages or flake expressions
    packages: Vec<String>,

    /// Extra flags to pass directly to nix
    #[arg(last = true)]
    nix_flags: Vec<String>,
  },
}

pub(super) struct WrapperConfig {
  platform:         cognos::Platform,
  silent:           bool,
  verbose:          u8,
  format:           rom_core::types::DisplayFormat,
  legend_style:     rom_core::types::LegendStyle,
  summary_style:    rom_core::types::SummaryStyle,
  log_prefix_style: rom_core::types::LogPrefixStyle,
  log_lines:        Option<usize>,
}

/// Run the CLI application
pub fn run() -> eyre::Result<()> {
  let cli = Cli::parse();

  // Initialize tracing based on verbosity level; RUST_LOG overrides
  let default_filter = match cli.verbose {
    0 => "rom=warn",
    1 => "rom=info",
    2 => "rom=debug",
    _ => "rom=trace",
  };
  tracing_subscriber::fmt()
    .with_env_filter(
      EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_filter)),
    )
    .with_target(false)
    .with_writer(std::io::stderr)
    .init();

  // Pre-parse typed display values before any moves of cli
  let format: rom_core::types::DisplayFormat = cli.format.parse()?;
  let legend_style: rom_core::types::LegendStyle = cli.legend.parse()?;
  let summary_style: rom_core::types::SummaryStyle = cli.summary.parse()?;
  let log_prefix_style: rom_core::types::LogPrefixStyle =
    cli.log_prefix.parse()?;
  let log_lines = cli.log_lines;
  let silent = cli.silent;
  let verbose = cli.verbose;
  let json = cli.json;
  let platform = cli
    .platform
    .as_deref()
    .and_then(|platform| platform.parse().ok())
    .unwrap_or_else(cognos::Platform::detect);

  // Check if we're being called as a symlink (rom-build, rom-shell)
  let program_name = std::env::args()
    .next()
    .and_then(|path| {
      PathBuf::from(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(std::string::ToString::to_string)
    })
    .unwrap_or_else(|| "rom".to_string());

  let make_config = |input_mode: rom_core::types::InputMode| {
    rom_core::types::Config {
      piping: false,
      silent,
      input_mode,
      show_timers: true,
      width: None,
      format,
      legend_style,
      summary_style,
      log_prefix_style,
      log_line_limit: log_lines,
    }
  };

  let cfg = WrapperConfig {
    platform,
    silent,
    verbose,
    format,
    legend_style,
    summary_style,
    log_prefix_style,
    log_lines,
  };

  match (&program_name[..], cli.command) {
    // rom-build symlink
    ("rom-build", _) => {
      let args: Vec<String> = std::env::args().skip(1).collect();
      let (packages, nix_flags) = parse_args_with_separator(&args);
      run_build_wrapper(packages, nix_flags, &cfg)?;
      Ok(())
    },

    // rom-shell symlink
    ("rom-shell", _) => {
      let args: Vec<String> = std::env::args().skip(1).collect();
      let (packages, nix_flags) = parse_args_with_separator(&args);
      run_shell_wrapper(packages, nix_flags, &cfg)?;
      Ok(())
    },

    // rom build command
    (
      _,
      Some(Commands::Build {
        packages,
        nix_flags,
      }),
    ) => {
      if packages.is_empty() && json {
        let stdin = io::stdin();
        let stdout = io::stdout();
        return Ok(rom_core::monitor_stream(
          make_config(rom_core::types::InputMode::Json),
          stdin.lock(),
          stdout.lock(),
        )?);
      }
      if packages.is_empty() {
        eyre::bail!(
          "No package or flake specified for build\nUsage: rom build \
           <package> [-- <flags>]\nExample: rom build nixpkgs#hello -- \
           --rebuild"
        );
      }
      run_build_wrapper(packages, nix_flags, &cfg)?;
      Ok(())
    },

    // rom shell command
    (
      _,
      Some(Commands::Shell {
        packages,
        nix_flags,
      }),
    ) => {
      if packages.is_empty() && json {
        let stdin = io::stdin();
        let stdout = io::stdout();
        return Ok(rom_core::monitor_stream(
          make_config(rom_core::types::InputMode::Json),
          stdin.lock(),
          stdout.lock(),
        )?);
      }
      if packages.is_empty() {
        eyre::bail!(
          "No package or flake specified for shell\nUsage: rom shell \
           <package> [-- <flags>]\nExample: rom shell nixpkgs#python3 -- \
           --pure"
        );
      }
      run_shell_wrapper(packages, nix_flags, &cfg)?;
      Ok(())
    },

    // rom develop command
    (
      _,
      Some(Commands::Develop {
        packages,
        nix_flags,
      }),
    ) => {
      if packages.is_empty() && json {
        let stdin = io::stdin();
        let stdout = io::stdout();
        return Ok(rom_core::monitor_stream(
          make_config(rom_core::types::InputMode::Json),
          stdin.lock(),
          stdout.lock(),
        )?);
      }
      if packages.is_empty() {
        eyre::bail!(
          "No package or flake specified for develop\nUsage: rom develop \
           <package> [-- <flags>]\nExample: rom develop nixpkgs#hello -- \
           --impure"
        );
      }
      run_develop_wrapper(packages, nix_flags, &cfg)?;
      Ok(())
    },

    // Direct piping mode, read from stdin
    (_, None) => {
      let input_mode = if json {
        rom_core::types::InputMode::Json
      } else {
        rom_core::types::InputMode::Human
      };
      let stdin = io::stdin();
      let stdout = io::stdout();
      Ok(rom_core::monitor_stream(
        make_config(input_mode),
        stdin.lock(),
        stdout.lock(),
      )?)
    },
  }
}

/// Parse arguments, separating those before and after `--`
/// Returns (`args_before_separator`, `args_after_separator`)
///
/// Everything before `--` is for the package name and rom arguments.
/// Everything after `--` goes directly to nix.
#[must_use]
pub fn parse_args_with_separator(
  args: &[String],
) -> (Vec<String>, Vec<String>) {
  if let Some(pos) = args.iter().position(|arg| arg == "--") {
    // Arguments before -- are package/rom args
    let before = args[..pos].to_vec();

    // Arguments after -- go to nix
    let after = args[pos + 1..].to_vec();
    (before, after)
  } else {
    // No separator found - all args are package/rom args for backward
    // compatibility
    (args.to_vec(), Vec::new())
  }
}

/// Returns the nix verbosity flag for the given level.
/// Always produces at least `-v` so build events are emitted via
/// `--log-format internal-json`.
fn nix_verbosity_flag(verbose: u8) -> String {
  format!("-{}", "v".repeat(verbose.max(1) as usize))
}

fn run_build_wrapper(
  packages: Vec<String>,
  nix_flags: Vec<String>,
  cfg: &WrapperConfig,
) -> eyre::Result<()> {
  if packages.is_empty() {
    eyre::bail!(
      "No package or flake specified for build\nUsage: rom build <package> \
       [-- <flags>]\nExample: rom build nixpkgs#hello -- --rebuild"
    );
  }

  let mut cmd_args = vec![
    "build".to_string(),
    nix_verbosity_flag(cfg.verbose),
    "--log-format".to_string(),
    "internal-json".to_string(),
  ];
  cmd_args.extend(packages);
  cmd_args.extend(nix_flags);

  let exit_code = run_monitored_command(cfg.platform.binary(), cmd_args, cfg)?;
  if exit_code != 0 {
    std::process::exit(exit_code);
  }
  Ok(())
}

fn run_shell_wrapper(
  packages: Vec<String>,
  nix_flags: Vec<String>,
  cfg: &WrapperConfig,
) -> eyre::Result<()> {
  if packages.is_empty() {
    eyre::bail!(
      "No package or flake specified for shell\nUsage: rom shell <package> \
       [-- <flags>]\nExample: rom shell nixpkgs#python3 -- --pure"
    );
  }

  // First pass: monitor the build phase with --command exit
  let mut monitor_args = vec![
    "shell".to_string(),
    nix_verbosity_flag(cfg.verbose),
    "--log-format".to_string(),
    "internal-json".to_string(),
  ];
  let shell_args: Vec<String> =
    packages.iter().chain(nix_flags.iter()).cloned().collect();
  monitor_args.extend(replace_command_with_exit(&shell_args));

  let exit_code =
    run_monitored_command(cfg.platform.binary(), monitor_args, cfg)?;

  if exit_code != 0 {
    std::process::exit(exit_code);
  }

  // Second pass: enter the actual shell
  if !cfg.silent {
    let mut shell_args = vec!["shell".to_string()];
    shell_args.extend(packages);
    shell_args.extend(nix_flags);

    let status = Command::new(cfg.platform.binary())
      .args(&shell_args)
      .status()
      .map_err(rom_core::error::RomError::Io)?;

    std::process::exit(status.code().unwrap_or(1));
  }

  Ok(())
}

fn run_develop_wrapper(
  packages: Vec<String>,
  nix_flags: Vec<String>,
  cfg: &WrapperConfig,
) -> eyre::Result<()> {
  // First pass: monitor with --command true
  let mut monitor_args = vec![
    "develop".to_string(),
    nix_verbosity_flag(cfg.verbose),
    "--log-format".to_string(),
    "internal-json".to_string(),
    "--command".to_string(),
    "true".to_string(),
  ];
  monitor_args.extend(packages.clone());
  monitor_args.extend(nix_flags.clone());

  let exit_code =
    run_monitored_command(cfg.platform.binary(), monitor_args, cfg)?;

  if exit_code != 0 {
    std::process::exit(exit_code);
  }

  // Second pass: enter the actual dev shell
  if !cfg.silent {
    let mut develop_args = vec!["develop".to_string()];
    develop_args.extend(packages);
    develop_args.extend(nix_flags);

    let status = Command::new(cfg.platform.binary())
      .args(&develop_args)
      .status()
      .map_err(rom_core::error::RomError::Io)?;

    std::process::exit(status.code().unwrap_or(1));
  }

  Ok(())
}

pub(super) struct MonitorShared {
  pub(super) state:        Arc<Mutex<rom_core::state::State>>,
  pub(super) graph:        Arc<Mutex<rom_core::graph::GraphIndexer>>,
  pub(super) log_store:    Arc<Mutex<LogStore>>,
  pub(super) stdout_lines: Arc<Mutex<Vec<String>>>,
  pub(super) stderr_done:  Arc<AtomicBool>,
}

impl MonitorShared {
  fn new(log_line_limit: Option<usize>) -> Self {
    Self {
      state:        Arc::new(Mutex::new(rom_core::state::State::new())),
      graph:        Arc::new(Mutex::new(rom_core::graph::GraphIndexer::new())),
      log_store:    Arc::new(Mutex::new(LogStore::new(log_line_limit))),
      stdout_lines: Arc::new(Mutex::new(Vec::new())),
      stderr_done:  Arc::new(AtomicBool::new(false)),
    }
  }
}

fn run_monitored_command(
  command: &str,
  args: Vec<String>,
  cfg: &WrapperConfig,
) -> eyre::Result<i32> {
  let mut child = Command::new(command)
    .args(&args)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(rom_core::error::RomError::Io)?;

  let stderr = child.stderr.take().expect("Failed to capture stderr");
  let stdout = child.stdout.take().expect("Failed to capture stdout");
  let use_tui = io::stderr().is_terminal();
  let log_line_limit = if use_tui {
    cfg.log_lines.or(Some(DEFAULT_TUI_LOG_LINE_LIMIT))
  } else {
    cfg.log_lines
  };

  let shared = MonitorShared::new(log_line_limit);
  let stderr_thread = spawn_stderr_reader(stderr, &shared, cfg, use_tui);
  let stdout_thread = spawn_stdout_reader(stdout, &shared);

  let exit_code = if use_tui {
    tui_runtime::run_tui_render_loop(&mut child, &shared, cfg)?
  } else {
    run_streaming_render_loop(&mut child, &shared, cfg)?
  };

  let _ = stderr_thread.join();
  let _ = stdout_thread.join();

  let stdout_lines = shared.stdout_lines.lock().unwrap();
  for line in stdout_lines.iter() {
    let _ = writeln!(io::stdout(), "{line}");
  }

  Ok(exit_code)
}

fn spawn_stderr_reader<R: Read + Send + 'static>(
  stderr: R,
  shared: &MonitorShared,
  cfg: &WrapperConfig,
  preserve_log_ansi: bool,
) -> thread::JoinHandle<()> {
  let state = shared.state.clone();
  let graph = shared.graph.clone();
  let log_store = shared.log_store.clone();
  let stderr_done = shared.stderr_done.clone();
  let log_prefix_style = cfg.log_prefix_style;
  let silent = cfg.silent;

  thread::spawn(move || {
    use tracing::debug;
    let reader = BufReader::new(stderr);
    let mut json_count = 0;
    let mut non_json_count = 0;
    let mut log_prefixes = HashMap::new();

    for line in reader.lines().map_while(Result::ok) {
      if let Some(json_line) = line.strip_prefix("@nix ") {
        json_count += 1;
        if let Ok(action) = serde_json::from_str::<cognos::Actions>(json_line) {
          debug!("Parsed JSON message #{}: {:?}", json_count, action);

          if let Some((id, line)) = build_log_line(&action) {
            let prefix = log_prefixes.get(&id).cloned().unwrap_or_default();
            push_log(&log_store, format!("{prefix}{line}"));
            continue;
          }
          if let Some(line) = post_build_log_line(&action) {
            push_log(&log_store, format!("[post-build] {line}"));
            continue;
          }

          if let Some((id, prefix)) =
            build_log_prefix(&action, log_prefix_style, !silent)
          {
            log_prefixes.insert(id, prefix);
          }

          let log_line = message_log_line(&action, preserve_log_ansi);
          if !rom_core::update::action_may_update_state(&action) {
            if let Some(line) = log_line {
              push_log(&log_store, line);
            }
            continue;
          }

          let (derivation_count_before, derivation_count_after) = {
            let mut state = state.lock().unwrap();
            let derivation_count_before = state.derivation_infos.len();
            let mut changed =
              rom_core::update::process_message(&mut state, action.clone());
            changed |= {
              let mut graph = graph.lock().unwrap();
              let mut graph_changed = graph.observe_action(&mut state, &action);
              if let cognos::Actions::Message { msg, raw_msg, .. } = &action {
                graph_changed |= graph.observe_plan_line(
                  &mut state,
                  raw_msg.as_deref().unwrap_or(msg.as_str()),
                );
              }
              graph_changed
            };
            if changed {
              rom_core::update::maintain_state(
                &mut state,
                rom_core::state::current_time(),
              );
            }
            let derivation_count_after = state.derivation_infos.len();

            (derivation_count_before, derivation_count_after)
          };

          if let Some(line) = log_line {
            push_log(&log_store, line);
          }

          if let cognos::Actions::Stop { id } = action {
            log_prefixes.remove(&id);
          }

          if derivation_count_after != derivation_count_before {
            debug!(
              "Derivation count changed: {} -> {}",
              derivation_count_before, derivation_count_after
            );
          }
        } else {
          debug!("Failed to parse JSON: {}", json_line);
        }
      } else {
        non_json_count += 1;
        push_log(&log_store, line);
      }
    }

    debug!(
      "Stderr thread finished: {} JSON messages, {} non-JSON lines",
      json_count, non_json_count
    );
    stderr_done.store(true, Ordering::Release);
  })
}

fn spawn_stdout_reader<R: Read + Send + 'static>(
  stdout: R,
  shared: &MonitorShared,
) -> thread::JoinHandle<()> {
  let stdout_lines = shared.stdout_lines.clone();
  thread::spawn(move || {
    let reader = BufReader::new(stdout);
    for line in reader.lines().map_while(Result::ok) {
      stdout_lines.lock().unwrap().push(line);
    }
  })
}

fn build_log_line(action: &cognos::Actions) -> Option<(cognos::Id, &str)> {
  let cognos::Actions::Result {
    fields,
    id,
    result_type,
  } = action
  else {
    return None;
  };
  if !matches!(result_type, cognos::ResultType::BuildLogLine) {
    return None;
  }

  fields
    .first()
    .and_then(|field| field.as_str())
    .map(|line| (*id, line))
}

fn post_build_log_line(action: &cognos::Actions) -> Option<&str> {
  let cognos::Actions::Result {
    fields,
    result_type,
    ..
  } = action
  else {
    return None;
  };
  if !matches!(result_type, cognos::ResultType::PostBuildLogLine) {
    return None;
  }

  fields.first().and_then(|field| field.as_str())
}

fn message_log_line(
  action: &cognos::Actions,
  preserve_log_ansi: bool,
) -> Option<String> {
  let cognos::Actions::Message { msg, raw_msg, .. } = action else {
    return None;
  };

  let display = if preserve_log_ansi {
    msg.as_str()
  } else {
    raw_msg.as_deref().unwrap_or(msg.as_str())
  };
  Some(display.to_string())
}

fn build_log_prefix(
  action: &cognos::Actions,
  style: rom_core::types::LogPrefixStyle,
  use_color: bool,
) -> Option<(cognos::Id, String)> {
  let cognos::Actions::Start {
    id,
    text,
    activity,
    fields,
    ..
  } = action
  else {
    return None;
  };
  if *activity != cognos::Activities::Build {
    return None;
  }

  let name = fields
    .first()
    .and_then(|value| value.as_str())
    .and_then(rom_core::state::Derivation::parse)
    .or_else(|| {
      text
        .split_whitespace()
        .map(|part| {
          part.trim_matches(|ch| ch == '\'' || ch == '"' || ch == ',')
        })
        .find_map(rom_core::state::Derivation::parse)
    })
    .map(|drv| drv.name)
    .unwrap_or_default();

  Some((*id, format_log_prefix(&name, style, use_color)))
}

fn format_log_prefix(
  name: &str,
  style: rom_core::types::LogPrefixStyle,
  use_color: bool,
) -> String {
  if matches!(style, rom_core::types::LogPrefixStyle::None) || name.is_empty() {
    return String::new();
  }

  let name = if use_color && std::io::stderr().is_terminal() {
    format!("\x1b[34m{name}\x1b[0m")
  } else {
    name.to_string()
  };
  format!("{name}> ")
}

fn push_log(log_store: &Arc<Mutex<LogStore>>, line: String) {
  log_store.lock().unwrap().push(line);
}

pub(super) fn snapshot_logs(
  shared: &MonitorShared,
  silent: bool,
  view: Option<&rom_core::tui::TuiView>,
) -> Vec<String> {
  if silent {
    Vec::new()
  } else {
    shared.log_store.lock().unwrap().snapshot(view)
  }
}

pub(super) fn display_config(
  cfg: &WrapperConfig,
  use_color: bool,
) -> rom_core::display::DisplayConfig {
  rom_core::display::DisplayConfig {
    show_timers: true,
    max_tree_depth: 10,
    max_visible_lines: 100,
    use_color,
    format: cfg.format,
    legend_style: cfg.legend_style,
    summary_style: cfg.summary_style,
    icons: rom_core::icons::detect(),
  }
}

pub(super) fn run_streaming_render_loop(
  child: &mut Child,
  shared: &MonitorShared,
  cfg: &WrapperConfig,
) -> eyre::Result<i32> {
  let render_state = shared.state.clone();
  let render_graph = shared.graph.clone();
  let log_store = shared.log_store.clone();
  let stderr_done = shared.stderr_done.clone();
  let silent = cfg.silent;
  let display_config = display_config(cfg, true);

  let render_thread = thread::spawn(move || {
    use rom_core::display::Display;

    let mut display = Display::new(io::stderr(), display_config).unwrap();

    loop {
      thread::sleep(Duration::from_millis(100));
      let done = stderr_done.load(Ordering::Acquire);
      let mut state = render_state.lock().unwrap();
      let mut graph = render_graph.lock().unwrap();
      if graph
        .populate_pending(&mut state, DEPENDENCY_POPULATE_BUDGET_PER_FRAME)
      {
        let now = rom_core::state::current_time();
        rom_core::update::maintain_state(&mut state, now);
      }
      let logs: Vec<String> = if silent {
        Vec::new()
      } else {
        log_store.lock().unwrap().snapshot(None)
      };
      let _ = display.render(&state, &logs);

      if done {
        break;
      }
    }

    thread::sleep(Duration::from_millis(50));
    let mut state = render_state.lock().unwrap();
    rom_core::update::finish_state(&mut state);
    let _ = display.render_final(&state);
  });

  let status = child.wait().map_err(rom_core::error::RomError::Io)?;
  let _ = render_thread.join();
  Ok(status.code().unwrap_or(1))
}

pub(super) fn render_final_after_tui(
  shared: &MonitorShared,
  cfg: &WrapperConfig,
  exit_code: i32,
) -> eyre::Result<()> {
  use rom_core::display::Display;

  let state = shared.state.lock().unwrap();
  let mut display = Display::new(
    io::stderr(),
    display_config(cfg, io::stderr().is_terminal()),
  )?;
  display
    .render_final(&state)
    .map_err(rom_core::error::RomError::Io)?;
  if exit_code != 0 {
    let logs = shared.log_store.lock().unwrap().snapshot(None);
    write_post_tui_failure_errors(io::stderr(), &state, &logs)
      .map_err(rom_core::error::RomError::Io)?;
  }
  Ok(())
}

fn write_post_tui_failure_errors<W: Write>(
  mut writer: W,
  state: &rom_core::state::State,
  logs: &[String],
) -> io::Result<()> {
  let lines = post_tui_failure_error_lines(state, logs);
  if lines.is_empty() {
    return Ok(());
  }

  writeln!(writer, "Build errors:")?;
  for line in lines {
    writeln!(writer, "{line}")?;
  }
  writeln!(writer)?;
  writer.flush()
}

/// Replace --command/-c arguments with "sh -c exit" for monitoring pass
pub fn replace_command_with_exit(args: &[String]) -> Vec<String> {
  let mut result = Vec::new();
  let mut skip_next = false;

  for arg in args {
    if skip_next {
      skip_next = false;
      continue;
    }

    if arg == "--command" || arg == "-c" {
      // Skip this and the next argument
      skip_next = true;
      continue;
    }

    result.push(arg.clone());
  }

  // Add our exit command
  result.push("--command".to_string());
  result.push("sh".to_string());
  result.push("-c".to_string());
  result.push("exit".to_string());

  result
}
