//! Command-line interface and Nix/Lix process adapters.

use std::{
  collections::HashSet,
  io::{self, IsTerminal, Read, Write},
  path::PathBuf,
  process::{Command, ExitCode, Stdio},
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError},
  },
  thread,
  time::{Duration, Instant},
};

use cognos::Verbosity;
use misstep::{OptionExt, ResultExt};
use pound::Parse;
use tracing_subscriber::EnvFilter;

use crate::{
  cache::BuildReportCache,
  display::{format_log, render_frame, write_final},
  error::RomError,
  monitor::{FilesystemResolver, Output, Processed, StreamEngine},
  state::{BuildStatus, DerivationId, State, current_time},
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

/// Pretty build graphs for your pretty Nix builds.
#[derive(Debug, Parse)]
#[pound(name = "rom", version)]
pub struct Cli {
  /// Nix command to wrap; without one, `rom` monitors standard input.
  #[pound(subcommand)]
  pub command: Option<Commands>,

  /// Treat every input record as unprefixed internal JSON.
  #[pound(long, global)]
  pub json: bool,

  /// Suppress decoded logs and presentations, but never passthrough or errors.
  #[pound(long, global)]
  pub silent: bool,

  /// Output format.
  #[pound(long, global, default = DisplayFormat::Tree)]
  pub format: DisplayFormat,

  /// Legend style.
  #[pound(long, global, default = LegendStyle::Table)]
  pub legend: LegendStyle,

  /// Final summary style.
  #[pound(long, global, default = SummaryStyle::Concise)]
  pub summary: SummaryStyle,

  /// Builder-log prefix.
  #[pound(long, global, default = LogPrefixStyle::Short)]
  pub log_prefix: LogPrefixStyle,

  /// Maximum decoded builder log lines per activity.
  #[pound(long, global)]
  pub log_lines: Option<usize>,

  /// Nix-family evaluator to use, nix or lix. Auto-detected by default.
  #[pound(
    long,
    global,
    parse = |value: &str| value.parse().map_err(|()| "expected nix or lix")
  )]
  pub platform: Option<cognos::Platform>,

  /// Increase Nix and ROM diagnostic verbosity.
  #[pound(short = 'v', count, global)]
  pub verbose: u8,
}

/// Nix commands that `rom` can wrap.
#[derive(Debug, Parse)]
pub enum Commands {
  /// Run nix/lix build with monitoring.
  Build {
    /// Installables to build.
    packages:  Vec<String>,
    /// Flags passed through to Nix.
    #[pound(last)]
    nix_flags: Vec<String>,
  },
  /// Realize inputs, then enter nix/lix shell.
  Shell {
    /// Installables to put in the shell.
    packages:  Vec<String>,
    /// Flags passed through to Nix.
    #[pound(last)]
    nix_flags: Vec<String>,
  },
  /// Realize inputs, then enter nix/lix develop.
  Develop {
    /// Installable whose development environment to enter.
    packages:  Vec<String>,
    /// Flags passed through to Nix.
    #[pound(last)]
    nix_flags: Vec<String>,
  },
}

struct WrapperConfig {
  platform: cognos::Platform,
  verbose:  u8,
  monitor:  Config,
}

/// Runs the CLI with the process's command-line arguments and returns the
/// exit code of the wrapped Nix command.
///
/// # Errors
///
/// Returns an error if an argument is not valid UTF-8, the Nix process cannot
/// be run, or the build fails or ends with unfinished work.
pub fn run() -> misstep::Result<ExitCode> {
  let mut process_args = std::env::args_os();
  let program = process_args
    .next()
    .and_then(|path| {
      PathBuf::from(path).file_name()?.to_str().map(str::to_owned)
    })
    .unwrap_or_else(|| "rom".to_owned());
  let mut arguments = process_args
    .map(|argument| {
      argument
        .into_string()
        .map_err(|_| misstep::report!("argument is not valid UTF-8"))
    })
    .collect::<misstep::Result<Vec<_>>>()?;
  match program.as_str() {
    "rom-build" => arguments.insert(0, "build".to_owned()),
    "rom-shell" => arguments.insert(0, "shell".to_owned()),
    "rom-develop" => arguments.insert(0, "develop".to_owned()),
    _ => {},
  }
  let cli = Cli::parse_from(arguments.iter().map(String::as_str));
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
      verbosity: match cli.verbose {
        0 => Verbosity::Info,
        1 => Verbosity::Talkative,
        2 => Verbosity::Chatty,
        3 => Verbosity::Debug,
        4.. => Verbosity::Vomit,
      },
      input_mode: if cli.json {
        InputMode::Json
      } else {
        InputMode::Auto
      },
      log_prefix_style: cli.log_prefix,
      log_line_limit: cli.log_lines,
      ..EngineConfig::default()
    },
    render: RenderConfig {
      ansi: io::stderr().is_terminal(),
      format: cli.format,
      legend_style: cli.legend,
      summary_style: cli.summary,
      ..RenderConfig::default()
    },
  };
  let config = WrapperConfig {
    platform: cli.platform.unwrap_or_else(cognos::Platform::detect),
    verbose: cli.verbose,
    monitor,
  };

  match cli.command {
    Some(
      Commands::Build { packages, .. }
      | Commands::Shell { packages, .. }
      | Commands::Develop { packages, .. },
    ) if packages.is_empty()
      && config.monitor.engine.input_mode == InputMode::Json =>
    {
      run_input(io::stdin(), config.monitor).map(|()| ExitCode::SUCCESS)
    },
    Some(Commands::Build {
      packages,
      nix_flags,
    }) => build(packages, nix_flags, &config),
    Some(Commands::Shell {
      packages,
      nix_flags,
    }) => shell(packages, nix_flags, &config),
    Some(Commands::Develop {
      packages,
      nix_flags,
    }) => develop(packages, nix_flags, &config),
    None => run_input(io::stdin(), config.monitor).map(|()| ExitCode::SUCCESS),
  }
}

fn nix_verbosity_flag(verbose: u8) -> String {
  format!("-{}", "v".repeat(verbose.max(1) as usize))
}

fn build(
  packages: Vec<String>,
  nix_flags: Vec<String>,
  config: &WrapperConfig,
) -> misstep::Result<ExitCode> {
  misstep::ensure!(
    !packages.is_empty(),
    "No package or flake specified for build"
  );
  let mut arguments = vec![
    "build".to_owned(),
    nix_verbosity_flag(config.verbose),
    "--log-format".to_owned(),
    "internal-json".to_owned(),
  ];
  arguments.extend(packages);
  arguments.extend(nix_flags);
  Ok(exit_code(run_monitored_command(
    config.platform.binary(),
    arguments,
    config,
  )?))
}

fn shell(
  packages: Vec<String>,
  nix_flags: Vec<String>,
  config: &WrapperConfig,
) -> misstep::Result<ExitCode> {
  misstep::ensure!(
    !packages.is_empty(),
    "No package or flake specified for shell"
  );
  let original: Vec<_> = packages.iter().chain(&nix_flags).cloned().collect();
  let mut monitored = vec![
    "shell".to_owned(),
    nix_verbosity_flag(config.verbose),
    "--log-format".to_owned(),
    "internal-json".to_owned(),
  ];
  monitored.extend(replace_command_with_exit(&original));
  let code =
    run_monitored_command(config.platform.binary(), monitored, config)?;
  if code != 0 {
    return Ok(exit_code(code));
  }
  let mut arguments = vec!["shell".to_owned()];
  arguments.extend(packages);
  arguments.extend(nix_flags);
  Ok(exit_code(run_inherited(
    config.platform.binary(),
    &arguments,
  )?))
}

fn develop(
  packages: Vec<String>,
  nix_flags: Vec<String>,
  config: &WrapperConfig,
) -> misstep::Result<ExitCode> {
  misstep::ensure!(
    !packages.is_empty(),
    "No package or flake specified for develop"
  );
  let original: Vec<_> = packages.iter().chain(&nix_flags).cloned().collect();
  let mut monitored = vec![
    "develop".to_owned(),
    nix_verbosity_flag(config.verbose),
    "--log-format".to_owned(),
    "internal-json".to_owned(),
  ];
  monitored.extend(replace_command_with_exit(&original));
  let code =
    run_monitored_command(config.platform.binary(), monitored, config)?;
  if code != 0 {
    return Ok(exit_code(code));
  }
  let mut arguments = vec!["develop".to_owned()];
  arguments.extend(packages);
  arguments.extend(nix_flags);
  Ok(exit_code(run_inherited(
    config.platform.binary(),
    &arguments,
  )?))
}

fn run_inherited(command: &str, arguments: &[String]) -> io::Result<i32> {
  let status = Command::new(command).args(arguments).status()?;
  Ok(exit_status_code(status))
}

/// Converts a child's exit status to ours, keeping the low byte as `exit`
/// does.
fn exit_code(code: i32) -> ExitCode {
  let [low, ..] = code.to_le_bytes();
  ExitCode::from(low)
}

fn run_monitored_command(
  program: &str,
  arguments: Vec<String>,
  config: &WrapperConfig,
) -> misstep::Result<i32> {
  #[cfg(unix)] use std::os::unix::process::CommandExt;

  let mut command = Command::new(program);
  command
    .args(arguments)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  #[cfg(unix)]
  command.process_group(0);
  let mut child = command
    .spawn()
    .with_context(|| format!("running {program}"))?;
  #[cfg(unix)]
  let signal_forwarder = SignalForwarder::start(child.id())?;
  #[cfg(unix)]
  let interrupted = Arc::clone(&signal_forwarder.interrupted);
  #[cfg(not(unix))]
  let interrupted = Arc::new(AtomicBool::new(false));
  let stdout = child.stdout.take().context("missing child stdout")?;
  let stderr = child.stderr.take().context("missing child stderr")?;

  let stdout_thread = thread::spawn(move || -> io::Result<()> {
    let mut source = stdout;
    let mut destination = io::stdout().lock();
    io::copy(&mut source, &mut destination)?;
    destination.flush()
  });
  let (receiver, reader_thread) = byte_reader(stderr);
  let mut monitor_config = config.monitor.clone();
  monitor_config.engine.input_mode = InputMode::Auto;
  let result = drive(&receiver, monitor_config, false, &interrupted);
  if result.is_err() {
    terminate_process(&mut child);
  }
  let status = child
    .wait()
    .with_context(|| format!("waiting for {program}"))?;
  #[cfg(unix)]
  signal_forwarder.stop();
  reader_thread
    .join()
    .map_err(|_| misstep::report!("stderr reader panicked"))?
    .context("reading child stderr")?;
  stdout_thread
    .join()
    .map_err(|_| misstep::report!("stdout relay panicked"))?
    .context("relaying child stdout")?;
  result?;
  Ok(exit_status_code(status))
}

#[cfg_attr(
  unix,
  expect(
    clippy::needless_pass_by_ref_mut,
    reason = "other platforms kill through the handle"
  )
)]
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
  handle:      signal_hook::iterator::Handle,
  interrupted: Arc<AtomicBool>,
  _thread:     thread::JoinHandle<()>,
}

#[cfg(unix)]
impl SignalForwarder {
  #[expect(
    clippy::semicolon_outside_block,
    reason = "semicolon_inside_block asks for the opposite"
  )]
  fn start(process_group: u32) -> io::Result<Self> {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let mut signals =
      signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP])?;
    let handle = signals.handle();
    let interrupted = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&interrupted);
    let thread = thread::spawn(move || {
      for signal in signals.forever() {
        // A second signal arrives when the first did not end rom quickly
        // enough, so it stops everything instead of being forwarded again.
        let repeated = seen.swap(true, Ordering::SeqCst);
        // SAFETY: the child was spawned as the leader of this process group.
        unsafe {
          libc::kill(
            -(process_group as i32),
            if repeated { libc::SIGKILL } else { signal },
          );
        }
        if repeated {
          // exit skips the live view's teardown, which shows the cursor again
          if io::stderr().is_terminal() {
            let _ = crossterm::execute!(io::stderr(), crossterm::cursor::Show);
          }
          std::process::exit(128 + signal);
        }
      }
    });
    Ok(Self {
      handle,
      interrupted,
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
) -> misstep::Result<()> {
  let (receiver, reader_thread) = byte_reader(reader);
  drive(&receiver, config, true, &AtomicBool::new(false))?;
  reader_thread
    .join()
    .map_err(|_| misstep::report!("input reader panicked"))?
    .context("reading standard input")?;
  Ok(())
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
  receiver: &Receiver<io::Result<Vec<u8>>>,
  config: Config,
  semantic_failure_is_error: bool,
  interrupted: &AtomicBool,
) -> misstep::Result<()> {
  let Config { engine, render } = config;
  let silent = engine.silent;
  let history = BuildReportCache::new(
    BuildReportCache::default_cache_path()
      .context("locating the build history")?,
  );
  let mut stream = StreamEngine::new(engine);
  stream.engine_mut().set_resolver(FilesystemResolver);
  stream.engine_mut().load_history(&history);
  let mut presenter = Presenter::new(silent);
  let mut dirty = false;
  let mut last_frame = Instant::now()
    .checked_sub(Duration::from_secs(1))
    .unwrap_or_else(Instant::now);
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
      Ok(Err(error)) => Err(error).context("reading build output")?,
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
    for id in available_derivations(stream.engine().state(), interrupted)? {
      stream.engine_mut().mark_available(id);
    }
    let _ = presenter.render(&stream, &render, current_time(), true)?;
    presenter.final_append(&stream, &render, silent, current_time())?;
  }
  if let Err(error) = stream.engine().save_history(&history) {
    tracing::debug!("failed to save build history: {error}");
  }
  if semantic_failure_is_error && stream.engine().state().has_errors() {
    misstep::bail!(RomError::BuildFailed);
  }
  if semantic_failure_is_error && stream.engine().state().has_unfinished() {
    misstep::bail!(RomError::Incomplete);
  }
  Ok(())
}

fn available_derivations(
  state: &State,
  interrupted: &AtomicBool,
) -> io::Result<Vec<DerivationId>> {
  #[cfg(unix)] use std::os::unix::process::CommandExt;

  let mut relevant: HashSet<_> = state
    .derivations()
    .iter()
    .filter_map(|(&id, info)| {
      matches!(info.build_status, BuildStatus::Planned).then_some(id)
    })
    .collect();
  let mut ancestors: Vec<_> = relevant.iter().copied().collect();
  while let Some(id) = ancestors.pop() {
    for &parent in &state.derivations()[&id].derivation_parents {
      if relevant.insert(parent) {
        ancestors.push(parent);
      }
    }
  }
  let candidates: Vec<_> = state
    .derivations()
    .iter()
    .filter(|(id, info)| {
      relevant.contains(id)
        && matches!(
          info.build_status,
          BuildStatus::Unknown | BuildStatus::Planned
        )
        && info.name.path.is_file()
    })
    .map(|(&id, info)| (id, &info.name.path))
    .collect();
  let mut pending: Vec<_> = candidates.chunks(128).collect();
  let mut available = Vec::new();
  while let Some(batch) = pending.pop() {
    if interrupted.load(Ordering::SeqCst) {
      break;
    }
    let mut query = Command::new("nix");
    // Its own group keeps a terminal Ctrl-C from killing the query, which
    // would read as a missing output and split the batch further.
    #[cfg(unix)]
    query.process_group(0);
    let output = query
      .args([
        "path-info",
        "--offline",
        "--extra-experimental-features",
        "nix-command",
      ])
      .arg("--")
      .args(
        batch
          .iter()
          .map(|(_, path)| format!("{}^*", path.display())),
      )
      .stdin(Stdio::null())
      .output()?;
    if output.status.success() {
      available.extend(batch.iter().map(|(id, _)| *id));
    } else if batch.len() > 1 {
      let (left, right) = batch.split_at(batch.len() / 2);
      pending.extend([right, left]);
    } else {
      tracing::debug!(
        "could not confirm outputs for {}: {}",
        batch[0].1.display(),
        String::from_utf8_lossy(&output.stderr).trim()
      );
    }
  }
  Ok(available)
}

/// Replaces any `--command` with `--command sh -c exit`, so the shell
/// realizes its inputs and exits immediately.
#[must_use]
pub fn replace_command_with_exit(arguments: &[String]) -> Vec<String> {
  let mut result = Vec::new();
  for argument in arguments {
    if argument == "--command" || argument == "-c" {
      break;
    }
    result.push(argument.clone());
  }
  result.extend(["--command", "sh", "-c", "exit"].map(str::to_owned));
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
