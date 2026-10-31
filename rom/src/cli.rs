//! Command-line interface and Nix/Lix process adapters.

use std::{
  collections::HashSet,
  env::{self, VarError},
  fs,
  io::{self, IsTerminal, Read, Write},
  iter,
  path::{Path, PathBuf},
  process::{Child, Command, ExitCode, Stdio},
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError},
  },
  thread,
  time::{Duration, Instant},
};

use cognos::Verbosity;
use etcetera::{BaseStrategy, choose_base_strategy};
use misstep::{OptionExt, ResultExt};
use pound::{External, Parse};
use serde::Deserialize;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
  filter::Targets,
  fmt,
  layer::SubscriberExt,
  util::SubscriberInitExt,
};

use crate::{
  cache::BuildReportCache,
  display::{format_log, render_frame, write_final},
  error::RomError,
  fixture::{FIXTURE_DERIVATIONS_ENV, FixtureResolver},
  monitor::{FilesystemResolver, Output, Processed, StreamEngine},
  state::{BuildStatus, DerivationId, State, current_time},
  terminal::{Admission, LiveTerminal},
  types::{
    Config,
    DisplayFormat,
    EngineConfig,
    IconMode,
    InputMode,
    LegendStyle,
    LogPrefixStyle,
    RenderConfig,
    SummaryStyle,
  },
  usage::CgroupUsage,
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

  #[pound(flatten)]
  pub display: DisplayOptions,

  /// Nix-family evaluator to use, nix or lix. Auto-detected by default.
  #[pound(long, global)]
  pub platform: Option<cognos::Platform>,

  /// Increase Nix and ROM diagnostic verbosity.
  #[pound(short = 'v', count, global)]
  pub verbose: u8,
}

/// Display flags, which can also be set in `$XDG_CONFIG_HOME/rom/config.toml`.
///
/// Flags given on the command line override the file.
#[derive(Debug, Clone, Copy, Default, Parse, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct DisplayOptions {
  /// Output format. Defaults to tree.
  #[pound(long, global)]
  pub format: Option<DisplayFormat>,

  /// Legend style. Defaults to table.
  #[pound(long, global)]
  pub legend: Option<LegendStyle>,

  /// Final summary style. Defaults to concise.
  #[pound(long, global)]
  pub summary: Option<SummaryStyle>,

  /// Icon set. Defaults to nerd.
  #[pound(long, global)]
  pub icons: Option<IconMode>,

  /// Builder-log prefix. Defaults to short.
  #[pound(long, global)]
  pub log_prefix: Option<LogPrefixStyle>,

  /// Maximum decoded builder log lines per activity.
  #[pound(long, global)]
  pub log_lines: Option<usize>,
}

impl DisplayOptions {
  /// Sets the options given here on `config`, keeping the others.
  pub fn apply(self, config: &mut Config) {
    let Self {
      format,
      legend,
      summary,
      icons,
      log_prefix,
      log_lines,
    } = self;
    let (engine, render) = (&mut config.engine, &mut config.render);
    render.format = format.unwrap_or(render.format);
    render.legend_style = legend.unwrap_or(render.legend_style);
    render.summary_style = summary.unwrap_or(render.summary_style);
    render.icons = icons.unwrap_or(render.icons);
    engine.log_prefix_style = log_prefix.unwrap_or(engine.log_prefix_style);
    engine.log_line_limit = log_lines.or(engine.log_line_limit);
  }

  /// Fills the flags left unset from `file`.
  fn or(self, file: Self) -> Self {
    Self {
      format:     self.format.or(file.format),
      legend:     self.legend.or(file.legend),
      summary:    self.summary.or(file.summary),
      icons:      self.icons.or(file.icons),
      log_prefix: self.log_prefix.or(file.log_prefix),
      log_lines:  self.log_lines.or(file.log_lines),
    }
  }
}

/// Reads the display flags set in the config file, if there is one.
fn read_config() -> misstep::Result<DisplayOptions> {
  let Ok(strategy) = choose_base_strategy() else {
    return Ok(DisplayOptions::default());
  };
  let path = strategy.config_dir().join("rom").join("config.toml");
  let text = match fs::read_to_string(&path) {
    Ok(text) => text,
    Err(error) if error.kind() == io::ErrorKind::NotFound => {
      return Ok(DisplayOptions::default());
    },
    Err(error) => {
      Err(error).with_context(|| format!("reading {}", path.display()))?
    },
  };
  Ok(
    toml::from_str(&text)
      .with_context(|| format!("parsing {}", path.display()))?,
  )
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
  /// Run any other nix/lix subcommand, monitored unless it needs the terminal.
  #[pound(external)]
  Other(External),
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
  let mut process_args = env::args_os();
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

  let mut cli = Cli::parse_from(arguments.iter().map(String::as_str));
  cli.display = cli.display.or(read_config()?);

  let filter = match env::var("RUST_LOG") {
    Ok(directives) => {
      directives
        .parse::<Targets>()
        .with_context(|| format!("parsing RUST_LOG={directives:?}"))?
    },
    Err(VarError::NotPresent) => {
      Targets::new().with_target("rom", match cli.verbose {
        0 => LevelFilter::WARN,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
      })
    },
    Err(error) => Err(error).context("reading RUST_LOG")?,
  };
  tracing_subscriber::registry()
    .with(fmt::layer().with_target(false).with_writer(io::stderr))
    .with(filter)
    .init();

  let mut monitor = Config {
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
      ..EngineConfig::default()
    },
    render: RenderConfig {
      ansi: io::stderr().is_terminal(),
      ..RenderConfig::default()
    },
  };
  cli.display.apply(&mut monitor);
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
    }) => enter("shell", packages, nix_flags, &config),
    Some(Commands::Develop {
      packages,
      nix_flags,
    }) => enter("develop", packages, nix_flags, &config),
    Some(Commands::Other(External { name, args, .. })) => {
      other(name, args, &config)
    },
    None => run_input(io::stdin(), config.monitor).map(|()| ExitCode::SUCCESS),
  }
}

/// Runs `program` monitored, with Nix's log flags after `head`.
fn monitored(
  program: &str,
  head: &[&str],
  rest: impl IntoIterator<Item = String>,
  config: &WrapperConfig,
) -> misstep::Result<i32> {
  let verbosity = (config.verbose > 0)
    .then(|| format!("-{}", "v".repeat(config.verbose.into())));
  let log_flags = ["--log-format".to_owned(), "internal-json".to_owned()];

  let arguments = head
    .iter()
    .map(|&word| word.to_owned())
    .chain(verbosity)
    .chain(log_flags)
    .chain(rest)
    .collect::<Vec<String>>();

  run_monitored_command(program, arguments, config)
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

  let rest = packages.into_iter().chain(nix_flags);

  Ok(exit_code(monitored(
    config.platform.binary(),
    &["build"],
    rest,
    config,
  )?))
}

/// Realises a `shell` or `develop` environment monitored, then enters it.
fn enter(
  subcommand: &str,
  packages: Vec<String>,
  nix_flags: Vec<String>,
  config: &WrapperConfig,
) -> misstep::Result<ExitCode> {
  misstep::ensure!(
    !packages.is_empty(),
    "No package or flake specified for {subcommand}"
  );

  let binary = config.platform.binary();

  let arguments = packages
    .into_iter()
    .chain(nix_flags)
    .collect::<Vec<String>>();

  let realise = replace_command_with_exit(&arguments);

  let code = monitored(binary, &[subcommand], realise, config)?;
  if code != 0 {
    return Ok(exit_code(code));
  }

  let arguments = iter::once(subcommand.to_owned()).chain(arguments);
  Ok(exit_code(run_inherited(binary, arguments)?))
}

fn other(
  name: String,
  mut args: Vec<String>,
  config: &WrapperConfig,
) -> misstep::Result<ExitCode> {
  let binary = config.platform.binary();

  match name.as_str() {
    "help"
      if args.first().is_none_or(|word| {
        matches!(word.as_str(), "build" | "shell" | "develop")
      }) =>
    {
      let words = args.iter().map(String::as_str).chain(["--help"]);
      let help = Cli::try_parse_from(words)
        .err()
        .context("--help did not end the parse")?;
      help.exit()
    },
    "env" if args.first().is_some_and(|word| word == "shell") => {
      let packages = args.split_off(1);
      enter("shell", packages, Vec::new(), config)
    },
    // These own the terminal, which a piped Nix cannot.
    "repl" | "edit" | "help" | "log" => {
      Ok(exit_code(run_inherited(
        binary,
        iter::once(name).chain(args),
      )?))
    },
    "run" => run_program(args, config),
    _ => {
      let rest = iter::once(name).chain(args);
      Ok(exit_code(monitored(binary, &[], rest, config)?))
    },
  }
}

fn run_program(
  args: Vec<String>,
  config: &WrapperConfig,
) -> misstep::Result<ExitCode> {
  let binary = config.platform.binary();

  // Nix treats the first positional after `--` as the installable.
  let separator = args.iter().position(|word| word == "--");
  let realise = separator.map_or_else(
    || args.clone(),
    |index| {
      let mut realise = args[..index].to_vec();
      if !realise.iter().any(|word| !word.starts_with('-'))
        && let Some(installable) = args.get(index + 1)
      {
        realise.push(installable.clone());
      }
      realise
    },
  );

  // Program args and flag values can pass for the installable. A failed
  // realise still runs, since `nix build` never looks in `apps`.
  if realise.iter().filter(|word| !word.starts_with('-')).count() <= 1 {
    let rest = realise.iter().cloned();
    monitored(binary, &["build", "--no-link"], rest, config)?;
  }

  let arguments = iter::once("run".to_owned()).chain(args);
  Ok(exit_code(run_inherited(binary, arguments)?))
}

fn run_inherited(
  command: &str,
  arguments: impl IntoIterator<Item = String>,
) -> io::Result<i32> {
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

  // A dry run exits 0 with the same "will be built" announcements.
  let builds = !arguments.iter().any(|argument| argument == "--dry-run");

  let mut command = Command::new(program);
  command
    .args(arguments)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

  #[cfg(unix)]
  command.process_group(0);

  // Its own process group keeps terminal signals from reaching the build, so
  // a killed rom would leave it running.
  #[cfg(target_os = "linux")]
  {
    let parent = std::process::id();
    let hook = move || {
      // SAFETY: prctl is async-signal-safe.
      if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0 {
        return Err(io::Error::last_os_error());
      }

      // SAFETY: getppid is async-signal-safe.
      if u32::try_from(unsafe { libc::getppid() }) != Ok(parent) {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
      }

      Ok(())
    };

    // SAFETY: the hook only makes async-signal-safe calls.
    unsafe {
      command.pre_exec(hook);
    }
  }

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

  let result = drive(
    &receiver,
    monitor_config,
    false,
    &interrupted,
    builds.then_some(&mut child),
  );
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
fn terminate_process(child: &mut Child) {
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
  drive(&receiver, config, true, &AtomicBool::new(false), None)?;
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

  fn frame_interval(&self) -> Duration {
    match self {
      Self::Live(terminal) => terminal.frame_interval(),
      Self::Fallback { .. } | Self::Append(_) => Duration::from_millis(50),
    }
  }

  fn flush_logs(&mut self) -> io::Result<()> {
    match self {
      Self::Live(terminal) => terminal.flush_logs(),
      Self::Fallback { .. } | Self::Append(_) => Ok(()),
    }
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

const LOG_INTERVAL: Duration = Duration::from_millis(50);

/// Feeds Nix's output to `stream` and redraws the live frame until the output
/// ends.
fn watch(
  receiver: &Receiver<io::Result<Vec<u8>>>,
  stream: &mut StreamEngine,
  presenter: &mut Presenter,
  render: &RenderConfig,
  silent: bool,
) -> misstep::Result<()> {
  let rendered =
    !silent && presenter.render_initial(stream, render, current_time())?;

  let started = Instant::now();
  let mut last_frame = rendered.then_some(started);
  let mut last_timer = started;
  let mut last_logs = started;
  let mut dirty = false;
  loop {
    match receiver.recv_timeout(Duration::from_millis(25)) {
      Ok(Ok(bytes)) => {
        let Processed { changed, output } =
          stream.push_at(&bytes, current_time())?;
        let has_output = !output.is_empty();
        presenter.output(output, render)?;
        dirty |= changed || has_output;
      },
      Ok(Err(error)) => Err(error).context("reading build output")?,
      Err(RecvTimeoutError::Disconnected) => return Ok(()),
      Err(RecvTimeoutError::Timeout) => {},
    }

    let now = Instant::now();
    let time = current_time();
    dirty |= stream.engine_mut().poll_lookups(time);
    dirty |= stream.engine_mut().sample_usage(time);

    let timer_due = now - last_timer >= Duration::from_secs(1);
    // Animated build and transfer bars need redraws even while Nix is quiet.
    let animating = stream.engine().state().has_unfinished();
    let redraw = !silent && (dirty || timer_due || animating);
    let frame_due =
      last_frame.is_none_or(|frame| now - frame >= presenter.frame_interval());

    if redraw && frame_due {
      presenter.render(stream, render, time, false)?;
      dirty = false;
      last_frame = Some(now);
      if timer_due {
        last_timer = now;
      }
    } else if now - last_logs >= LOG_INTERVAL {
      presenter.flush_logs()?;
      last_logs = now;
    }
  }
}

fn drive(
  receiver: &Receiver<io::Result<Vec<u8>>>,
  config: Config,
  semantic_failure_is_error: bool,
  interrupted: &AtomicBool,
  child: Option<&mut Child>,
) -> misstep::Result<()> {
  let Config { engine, render } = config;
  let silent = engine.silent;
  let history = BuildReportCache::new(
    BuildReportCache::default_cache_path()
      .context("locating the build history")?,
  );

  let mut stream = StreamEngine::new(engine);
  // Replayed fixtures name derivations that are not in the store.
  if let Some(path) = std::env::var_os(FIXTURE_DERIVATIONS_ENV) {
    let resolver = FixtureResolver::load(Path::new(&path))
      .context("loading fixture derivations")?;
    stream.engine_mut().set_resolver(resolver);
  } else {
    stream
      .engine_mut()
      .set_resolver(FilesystemResolver::default());
  }
  stream.engine_mut().load_history(&history);

  if let Some(usage) = CgroupUsage::detect() {
    stream.engine_mut().set_usage(usage);
  }

  let mut presenter = Presenter::new(silent);
  watch(receiver, &mut stream, &mut presenter, &render, silent)?;

  let final_output = stream.finish_at(current_time())?;
  presenter.output(final_output.output, &render)?;
  let succeeded = match child {
    Some(child) => child.wait().context("waiting for nix")?.success(),
    None => false,
  };

  if succeeded {
    stream.engine_mut().mark_planned_available();
  }

  if !silent {
    if !succeeded {
      for id in available_derivations(stream.engine().state(), interrupted)? {
        stream.engine_mut().mark_available(id);
      }
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

  let mut relevant = state
    .derivations()
    .iter()
    .filter_map(|(&id, info)| {
      matches!(info.build_status, BuildStatus::Planned).then_some(id)
    })
    .collect::<HashSet<DerivationId>>();
  let mut ancestors = relevant.iter().copied().collect::<Vec<DerivationId>>();
  while let Some(id) = ancestors.pop() {
    for &parent in &state.derivations()[&id].derivation_parents {
      if relevant.insert(parent) {
        ancestors.push(parent);
      }
    }
  }
  let candidates = state
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
    .collect::<Vec<(DerivationId, &PathBuf)>>();
  let mut pending = candidates
    .chunks(128)
    .collect::<Vec<&[(DerivationId, &PathBuf)]>>();
  let mut available = Vec::new();

  // Older Nix clients can spin for good resolving dynamic derivations.
  let deadline = Instant::now() + Duration::from_secs(10);

  while let Some(batch) = pending.pop() {
    if interrupted.load(Ordering::SeqCst) {
      break;
    }
    let mut query = Command::new("nix");
    // Its own group keeps a terminal Ctrl-C from killing the query, which
    // would read as a missing output and split the batch further.
    #[cfg(unix)]
    query.process_group(0);
    let mut child = query
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
      .stdout(Stdio::null())
      .stderr(Stdio::piped())
      .spawn()?;

    while child.try_wait()?.is_none() && Instant::now() < deadline {
      thread::sleep(Duration::from_millis(20));
    }
    if child.try_wait()?.is_none() {
      child.kill()?;
      child.wait()?;
      tracing::debug!("gave up confirming outputs past the deadline");
      break;
    }

    let output = child.wait_with_output()?;
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
