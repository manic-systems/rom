//! Command-line interface and Nix/Lix process adapters.

use std::{
  io::{self, IsTerminal, Read, Write},
  path::PathBuf,
  process::{Command, Stdio},
  sync::mpsc::{self, Receiver, RecvTimeoutError},
  thread,
  time::{Duration, Instant},
};

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::{
  cache::BuildReportCache,
  display::{format_log, render_frame, write_final},
  error::RomError,
  monitor::{FilesystemResolver, Output, Processed, StreamEngine},
  state::current_time,
  terminal::{Admission, LiveTerminal},
  types::{
    Config,
    DisplayFormat,
    EngineConfig,
    InputMode,
    LegendStyle,
    LogPrefixStyle,
    RenderConfig,
    SummaryStyle,
  },
};

#[derive(Debug, Parser)]
#[command(name = "rom", version, about = "Pretty build graphs for Nix and Lix")]
pub struct Cli {
  #[command(subcommand)]
  pub command: Option<Commands>,

  /// Treat every input record as unprefixed internal JSON.
  #[arg(long, global = true)]
  pub json: bool,

  /// Suppress decoded logs and presentations, but never passthrough or errors.
  #[arg(long, global = true)]
  pub silent: bool,

  /// Output format: tree, plain, dashboard.
  #[arg(long, global = true, default_value = "tree")]
  pub format: String,

  /// Legend style: compact, table, verbose.
  #[arg(long, global = true, default_value = "table")]
  pub legend: String,

  /// Final summary style: concise, table, full.
  #[arg(long, global = true, default_value = "concise")]
  pub summary: String,

  /// Builder-log prefix: short, full, none.
  #[arg(long, global = true, default_value = "short")]
  pub log_prefix: String,

  /// Maximum decoded builder log lines per activity.
  #[arg(long, global = true)]
  pub log_lines: Option<usize>,

  /// Nix-family evaluator to use. Auto-detected by default.
  #[arg(long, global = true)]
  pub platform: Option<String>,

  /// Increase Nix and ROM diagnostic verbosity.
  #[arg(short = 'v', action = clap::ArgAction::Count, global = true)]
  pub verbose: u8,
}

#[derive(Debug, clap::Subcommand)]
pub enum Commands {
  /// Run nix/lix build with monitoring.
  Build {
    packages:  Vec<String>,
    #[arg(last = true)]
    nix_flags: Vec<String>,
  },
  /// Realize inputs, then enter nix/lix shell.
  Shell {
    packages:  Vec<String>,
    #[arg(last = true)]
    nix_flags: Vec<String>,
  },
  /// Realize inputs, then enter nix/lix develop.
  Develop {
    packages:  Vec<String>,
    #[arg(last = true)]
    nix_flags: Vec<String>,
  },
}

struct WrapperConfig {
  platform: cognos::Platform,
  verbose:  u8,
  monitor:  Config,
}

pub fn run() -> eyre::Result<()> {
  let cli = Cli::parse();
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
    .with_writer(io::stderr)
    .init();

  let monitor = Config {
    engine: EngineConfig {
      silent: cli.silent,
      input_mode: if cli.json {
        InputMode::Json
      } else {
        InputMode::Auto
      },
      log_prefix_style: LogPrefixStyle::parse(&cli.log_prefix)
        .ok_or_else(|| eyre::eyre!("unknown log prefix: {}", cli.log_prefix))?,
      log_line_limit: cli.log_lines,
      ..EngineConfig::default()
    },
    render: RenderConfig {
      ansi: io::stderr().is_terminal(),
      format: DisplayFormat::parse(&cli.format)
        .ok_or_else(|| eyre::eyre!("unknown format: {}", cli.format))?,
      legend_style: LegendStyle::parse(&cli.legend)
        .ok_or_else(|| eyre::eyre!("unknown legend style: {}", cli.legend))?,
      summary_style: SummaryStyle::parse(&cli.summary)
        .ok_or_else(|| eyre::eyre!("unknown summary style: {}", cli.summary))?,
      ..RenderConfig::default()
    },
  };
  let config = WrapperConfig {
    platform: cli
      .platform
      .as_deref()
      .and_then(|value| value.parse().ok())
      .unwrap_or_else(cognos::Platform::detect),
    verbose: cli.verbose,
    monitor,
  };

  let program = std::env::args()
    .next()
    .and_then(|path| {
      PathBuf::from(path).file_name()?.to_str().map(str::to_owned)
    })
    .unwrap_or_else(|| "rom".to_string());

  match (program.as_str(), cli.command) {
    ("rom-build", _) => {
      let args: Vec<_> = std::env::args().skip(1).collect();
      let (packages, flags) = parse_args_with_separator(&args);
      build(packages, flags, &config)
    },
    ("rom-shell", _) => {
      let args: Vec<_> = std::env::args().skip(1).collect();
      let (packages, flags) = parse_args_with_separator(&args);
      shell(packages, flags, &config)
    },
    ("rom-develop", _) => {
      let args: Vec<_> = std::env::args().skip(1).collect();
      let (packages, flags) = parse_args_with_separator(&args);
      develop(packages, flags, &config)
    },
    (
      _,
      Some(Commands::Build {
        packages,
        nix_flags,
      }),
    ) => {
      if packages.is_empty()
        && config.monitor.engine.input_mode == InputMode::Json
      {
        run_input(io::stdin(), config.monitor.clone())
      } else {
        build(packages, nix_flags, &config)
      }
    },
    (
      _,
      Some(Commands::Shell {
        packages,
        nix_flags,
      }),
    ) => {
      if packages.is_empty()
        && config.monitor.engine.input_mode == InputMode::Json
      {
        run_input(io::stdin(), config.monitor.clone())
      } else {
        shell(packages, nix_flags, &config)
      }
    },
    (
      _,
      Some(Commands::Develop {
        packages,
        nix_flags,
      }),
    ) => {
      if packages.is_empty()
        && config.monitor.engine.input_mode == InputMode::Json
      {
        run_input(io::stdin(), config.monitor.clone())
      } else {
        develop(packages, nix_flags, &config)
      }
    },
    (_, None) => run_input(io::stdin(), config.monitor.clone()),
  }
}

#[must_use]
pub fn parse_args_with_separator(
  args: &[String],
) -> (Vec<String>, Vec<String>) {
  args
    .iter()
    .position(|argument| argument == "--")
    .map_or_else(
      || (args.to_vec(), Vec::new()),
      |separator| (args[..separator].to_vec(), args[separator + 1..].to_vec()),
    )
}

fn nix_verbosity_flag(verbose: u8) -> String {
  format!("-{}", "v".repeat(verbose.max(1) as usize))
}

fn require_packages(kind: &str, packages: &[String]) -> eyre::Result<()> {
  if packages.is_empty() {
    eyre::bail!("No package or flake specified for {kind}");
  }
  Ok(())
}

fn build(
  packages: Vec<String>,
  nix_flags: Vec<String>,
  config: &WrapperConfig,
) -> eyre::Result<()> {
  require_packages("build", &packages)?;
  let mut arguments = vec![
    "build".to_string(),
    nix_verbosity_flag(config.verbose),
    "--log-format".to_string(),
    "internal-json".to_string(),
  ];
  arguments.extend(packages);
  arguments.extend(nix_flags);
  exit_with(run_monitored_command(
    config.platform.binary(),
    arguments,
    config,
  )?)
}

fn shell(
  packages: Vec<String>,
  nix_flags: Vec<String>,
  config: &WrapperConfig,
) -> eyre::Result<()> {
  require_packages("shell", &packages)?;
  let original: Vec<_> = packages.iter().chain(&nix_flags).cloned().collect();
  let mut monitored = vec![
    "shell".to_string(),
    nix_verbosity_flag(config.verbose),
    "--log-format".to_string(),
    "internal-json".to_string(),
  ];
  monitored.extend(replace_command_with_exit(&original));
  let code =
    run_monitored_command(config.platform.binary(), monitored, config)?;
  if code != 0 {
    return exit_with(code);
  }
  let mut arguments = vec!["shell".to_string()];
  arguments.extend(packages);
  arguments.extend(nix_flags);
  exit_with(run_inherited(config.platform.binary(), &arguments)?)
}

fn develop(
  packages: Vec<String>,
  nix_flags: Vec<String>,
  config: &WrapperConfig,
) -> eyre::Result<()> {
  require_packages("develop", &packages)?;
  let mut monitored = vec![
    "develop".to_string(),
    nix_verbosity_flag(config.verbose),
    "--log-format".to_string(),
    "internal-json".to_string(),
    "--command".to_string(),
    "true".to_string(),
  ];
  monitored.extend(packages.clone());
  monitored.extend(nix_flags.clone());
  let code =
    run_monitored_command(config.platform.binary(), monitored, config)?;
  if code != 0 {
    return exit_with(code);
  }
  let mut arguments = vec!["develop".to_string()];
  arguments.extend(packages);
  arguments.extend(nix_flags);
  exit_with(run_inherited(config.platform.binary(), &arguments)?)
}

fn run_inherited(command: &str, arguments: &[String]) -> io::Result<i32> {
  let status = Command::new(command).args(arguments).status()?;
  Ok(exit_status_code(status))
}

fn exit_with(code: i32) -> eyre::Result<()> {
  if code != 0 {
    std::process::exit(code);
  }
  Ok(())
}

fn run_monitored_command(
  command: &str,
  arguments: Vec<String>,
  config: &WrapperConfig,
) -> eyre::Result<i32> {
  #[cfg(unix)] use std::os::unix::process::CommandExt;

  let mut command = Command::new(command);
  command
    .args(arguments)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  #[cfg(unix)]
  command.process_group(0);
  let mut child = command.spawn().map_err(RomError::Io)?;
  #[cfg(unix)]
  let signal_forwarder = SignalForwarder::start(child.id())?;
  let stdout = child
    .stdout
    .take()
    .ok_or_else(|| RomError::process("missing child stdout"))?;
  let stderr = child
    .stderr
    .take()
    .ok_or_else(|| RomError::process("missing child stderr"))?;

  let stdout_thread = thread::spawn(move || -> io::Result<()> {
    let mut stdout = stdout;
    let mut destination = io::stdout().lock();
    io::copy(&mut stdout, &mut destination)?;
    destination.flush()
  });
  let (receiver, reader_thread) = byte_reader(stderr);
  let mut monitor_config = config.monitor.clone();
  monitor_config.engine.input_mode = InputMode::Auto;
  let result = drive(receiver, monitor_config, false);
  if result.is_err() {
    terminate_process(&mut child);
  }
  let status = child.wait().map_err(RomError::Io)?;
  #[cfg(unix)]
  signal_forwarder.stop();
  reader_thread
    .join()
    .map_err(|_| RomError::process("stderr reader panicked"))??;
  stdout_thread
    .join()
    .map_err(|_| RomError::process("stdout relay panicked"))??;
  result?;
  Ok(exit_status_code(status))
}

fn terminate_process(child: &mut std::process::Child) {
  #[cfg(unix)]
  {
    let process_group = -(child.id() as i32);
    // SAFETY: a negative pid addresses the child-created process group.
    unsafe {
      libc::kill(process_group, libc::SIGKILL);
    }
  }
  #[cfg(not(unix))]
  let _ = child.kill();
}

#[cfg(unix)]
struct SignalForwarder {
  handle:  signal_hook::iterator::Handle,
  _thread: thread::JoinHandle<()>,
}

#[cfg(unix)]
impl SignalForwarder {
  fn start(process_group: u32) -> io::Result<Self> {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let mut signals =
      signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP])?;
    let handle = signals.handle();
    let thread = thread::spawn(move || {
      for signal in signals.forever() {
        // SAFETY: the child was spawned as the leader of this process group.
        unsafe {
          libc::kill(-(process_group as i32), signal);
        }
      }
    });
    Ok(Self {
      handle,
      _thread: thread,
    })
  }

  fn stop(self) {
    self.handle.close();
  }
}

fn run_input<R: Read + Send + 'static>(
  reader: R,
  config: Config,
) -> eyre::Result<()> {
  let (receiver, reader_thread) = byte_reader(reader);
  let result = drive(receiver, config, true);
  reader_thread
    .join()
    .map_err(|_| RomError::process("input reader panicked"))??;
  result
}

fn byte_reader<R: Read + Send + 'static>(
  mut reader: R,
) -> (
  Receiver<io::Result<Vec<u8>>>,
  thread::JoinHandle<io::Result<()>>,
) {
  let (sender, receiver) = mpsc::sync_channel(128);
  let thread = thread::spawn(move || {
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
      match reader.read(&mut buffer) {
        Ok(0) => break,
        Ok(count) => {
          if sender.send(Ok(buffer[..count].to_vec())).is_err() {
            break;
          }
        },
        Err(error) => {
          let _ = sender.send(Err(error));
          break;
        },
      }
    }
    Ok(())
  });
  (receiver, thread)
}

enum Presenter {
  Live(LiveTerminal<io::Stderr>),
  Fallback {
    writer:            io::Stderr,
    presented_initial: bool,
  },
  Append(io::Stderr),
}

impl Presenter {
  fn new(silent: bool) -> Self {
    let admission = crate::terminal::admission();
    tracing::debug!(?admission, silent, "selected terminal presenter");
    if !silent && matches!(admission, Admission::Live) {
      Self::Live(LiveTerminal::new(io::stderr()))
    } else if !silent && !matches!(admission, Admission::NotATerminal) {
      Self::Fallback {
        writer:            io::stderr(),
        presented_initial: false,
      }
    } else {
      Self::Append(io::stderr())
    }
  }

  fn output(
    &mut self,
    output: Vec<Output>,
    render: &RenderConfig,
  ) -> io::Result<()> {
    for item in output {
      match item {
        Output::Passthrough(bytes) => self.write(&bytes)?,
        Output::Log(line) => {
          let line = format_log(&line, render);
          self.write(line.as_bytes())?;
          self.write(b"\n")?;
        },
      }
    }
    Ok(())
  }

  fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
    match self {
      Self::Live(terminal) => terminal.write_passthrough(bytes),
      Self::Fallback { writer, .. } | Self::Append(writer) => {
        writer.write_all(bytes)?;
        writer.flush()
      },
    }
  }

  fn render(
    &mut self,
    stream: &StreamEngine,
    config: &RenderConfig,
    now: f64,
    final_render: bool,
  ) -> io::Result<bool> {
    match self {
      Self::Live(terminal) => {
        terminal.render(stream.engine().state(), config, now, final_render)
      },
      Self::Fallback {
        writer,
        presented_initial,
      } => {
        if final_render || *presented_initial {
          return Ok(false);
        }
        let width = config.width.unwrap_or(100).max(2) - 1;
        let height = config.height.unwrap_or(100).max(1);
        let frame = render_frame(
          stream.engine().state(),
          config,
          now,
          width,
          height,
          false,
        );
        if frame.text().trim().is_empty() {
          return Ok(false);
        }
        if config.ansi {
          writeln!(writer, "{}", frame.ansi_text())?;
        } else {
          writeln!(writer, "{}", frame.text())?;
        }
        writer.flush()?;
        *presented_initial = true;
        Ok(true)
      },
      Self::Append(_) => Ok(false),
    }
  }

  fn render_initial(
    &mut self,
    stream: &StreamEngine,
    config: &RenderConfig,
    now: f64,
  ) -> io::Result<bool> {
    match self {
      Self::Live(terminal) => {
        terminal.render(stream.engine().state(), config, now, false)
      },
      Self::Fallback { .. } | Self::Append(_) => Ok(false),
    }
  }

  fn final_append(
    &mut self,
    stream: &StreamEngine,
    config: &RenderConfig,
    silent: bool,
    now: f64,
  ) -> io::Result<()> {
    if silent {
      return Ok(());
    }
    match self {
      Self::Live(terminal) if !terminal.is_retired() => terminal.finish(),
      Self::Live(terminal) => {
        terminal.append_final(stream.engine().state(), config, now)
      },
      Self::Fallback { writer, .. } | Self::Append(writer) => {
        write_final(writer, stream.engine().state(), config, now)
      },
    }
  }
}

fn drive(
  receiver: Receiver<io::Result<Vec<u8>>>,
  config: Config,
  semantic_failure_is_error: bool,
) -> eyre::Result<()> {
  let Config { engine, render } = config;
  let silent = engine.silent;
  let history = BuildReportCache::new(BuildReportCache::default_cache_path());
  let mut stream = StreamEngine::new(engine);
  stream.engine_mut().set_resolver(FilesystemResolver);
  stream.engine_mut().load_history(&history);
  let mut presenter = Presenter::new(silent);
  let mut dirty = false;
  let mut last_frame = Instant::now() - Duration::from_secs(1);
  let mut last_timer = Instant::now();
  if !silent && presenter.render_initial(&stream, &render, current_time())? {
    last_frame = Instant::now();
  }

  loop {
    match receiver.recv_timeout(Duration::from_millis(25)) {
      Ok(Ok(bytes)) => {
        let Processed { changed, output } =
          stream.push_at(&bytes, current_time())?;
        let has_output = !output.is_empty();
        presenter.output(output, &render)?;
        dirty |= changed || has_output;
      },
      Ok(Err(error)) => return Err(error.into()),
      Err(RecvTimeoutError::Disconnected) => break,
      Err(RecvTimeoutError::Timeout) => {},
    }
    let timer_due = last_timer.elapsed() >= Duration::from_secs(1);
    if !silent
      && (dirty || timer_due)
      && last_frame.elapsed() >= Duration::from_millis(50)
    {
      presenter.render(&stream, &render, current_time(), false)?;
      dirty = false;
      last_frame = Instant::now();
      if timer_due {
        last_timer = Instant::now();
      }
    }
  }

  let final_output = stream.finish_at(current_time())?;
  presenter.output(final_output.output, &render)?;
  if !silent {
    let _ = presenter.render(&stream, &render, current_time(), true)?;
    presenter.final_append(&stream, &render, silent, current_time())?;
  }
  if let Err(error) = stream.engine().save_history(&history) {
    tracing::debug!("failed to save build history: {error}");
  }
  if semantic_failure_is_error && stream.engine().state().has_errors() {
    return Err(RomError::BuildFailed.into());
  }
  if semantic_failure_is_error && stream.engine().state().has_unfinished() {
    return Err(RomError::Incomplete.into());
  }
  Ok(())
}

#[must_use]
pub fn replace_command_with_exit(arguments: &[String]) -> Vec<String> {
  let mut result = Vec::new();
  let mut skip = false;
  for argument in arguments {
    if skip {
      skip = false;
    } else if argument == "--command" || argument == "-c" {
      skip = true;
    } else {
      result.push(argument.clone());
    }
  }
  result.extend(["--command", "sh", "-c", "exit"].map(str::to_string));
  result
}

fn exit_status_code(status: std::process::ExitStatus) -> i32 {
  if let Some(code) = status.code() {
    return code;
  }
  #[cfg(unix)]
  {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map_or(1, |signal| 128 + signal)
  }
  #[cfg(not(unix))]
  1
}
