//! Shared synchronous ingestion engine and append-only stream adapter.

use std::{
  collections::{BTreeSet, VecDeque, hash_map::Entry},
  fs::{self, File, OpenOptions},
  io::{self, BufRead, BufWriter, Read, Write},
  iter,
  path::{Path, PathBuf},
  process::{self, Child, Command, Stdio},
  sync::{
    Arc,
    Condvar,
    Mutex,
    OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
  },
  thread,
  time::Duration,
};

use cognos::{DecodedAction, Id, UnsupportedRecord};
use etcetera::{BaseStrategy, choose_base_strategy};
use foldhash::{HashMap, HashMapExt, HashSet, HashSetExt};
use serde::Deserialize;

use crate::{
  cache::BuildReportCache,
  display::{format_log, write_final},
  error::{Result, RomError},
  event::Event,
  state::{BuildStatus, Derivation, DerivationId, State, current_time},
  types::{Config, EngineConfig, InputMode, LogLine, RenderConfig},
  update::{self, LogEffect},
  usage::CgroupUsage,
};

/// Optional source of `.drv` metadata.
///
/// Resolver failures are diagnostic-only; the activity remains visible as a
/// truthful root rather than aborting work.
pub trait DerivationResolver: Send + Sync {
  /// Parses the derivation at `path`.
  ///
  /// # Errors
  ///
  /// Returns an error if the derivation cannot be read or parsed.
  fn resolve(
    &self,
    path: &Path,
  ) -> std::result::Result<cognos::ParsedDerivation, cognos::aterm::Error>;

  /// Asks for the `.drv` each dynamic-derivation producer wrote to its `out`.
  ///
  /// It must return without waiting, since the engine calls it while
  /// ingesting. Answers come back through [`Self::answers`].
  fn request_produced(
    &self,
    _producers: Vec<PathBuf>,
    _priority: LookupPriority,
  ) {
  }

  /// Records that `producer`, by its original path, wrote `drv`, so a later
  /// run that finds it already realised need not look it up.
  fn remember_produced(&self, _producer: &Path, _drv: &Path) {}

  /// Asks which of `producers` wrote `drv`, answering only a match. It must
  /// return without waiting.
  fn request_writer(&self, _drv: PathBuf, _producers: Vec<PathBuf>) {}

  /// Asks whether each content-addressed derivation's outputs are already
  /// realised, without building anything.
  ///
  /// Like [`Self::request_produced`], it must return without waiting.
  fn request_realised(&self, _drvs: Vec<PathBuf>) {}

  /// Returns the answers that arrived since the last call.
  fn answers(&self) -> Vec<Answer> {
    Vec::new()
  }
}

/// How soon a producer lookup is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LookupPriority {
  /// Its answer attaches a subtree that is cached or already building.
  Attach,
  /// It follows the producer's own build.
  Built,
}

/// The answer to a lookup the engine asked a [`DerivationResolver`] for.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Answer {
  /// The answer to a [`DerivationResolver::request_produced`] lookup.
  Produced(ProducedDrv),
  /// The answer to a [`DerivationResolver::request_realised`] lookup.
  Realised(RealisedDrv),
}

/// What a dynamic-derivation producer wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducedDrv {
  /// The dynamic-derivation producer.
  pub producer: PathBuf,
  /// The `.drv` it wrote, or `None` if Nix has not realised it yet.
  pub drv:      Option<PathBuf>,
}

/// Whether a content-addressed derivation's outputs are already realised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealisedDrv {
  /// The derivation, as it was asked about.
  pub drv:      PathBuf,
  /// Whether Nix has a realisation for its outputs.
  pub realised: bool,
}

/// Resolver used by the CLI for local store derivations.
///
/// Produced derivations have no deriver, and a content-addressed derivation
/// Nix finds realised gets no build activity, so Nix's build trace is the
/// only record of either. A worker thread reads it, batching lookups that
/// arrive together into as few `nix` calls as it can.
#[derive(Default)]
pub struct FilesystemResolver {
  worker: OnceLock<TraceWorker>,
}

enum TraceRequest {
  Writer {
    drv:       PathBuf,
    producers: Vec<PathBuf>,
  },
  Realised(Vec<PathBuf>),
}

/// Producer lookups run on their own thread, since a writer search or a
/// realisation check over originals can take seconds per call.
struct TraceWorker {
  requests: Sender<TraceRequest>,
  lookups:  Arc<(Mutex<LookupQueue>, Condvar)>,
  known:    Mutex<KnownProducers>,
  answered: Sender<Answer>,
  answers:  Mutex<Receiver<Answer>>,
}

/// Producers an earlier run linked, kept since looking one up by its original
/// resolves its whole closure again.
struct KnownProducers {
  path:  Option<PathBuf>,
  file:  Option<File>,
  links: HashMap<PathBuf, PathBuf>,
}

impl KnownProducers {
  fn load() -> Self {
    let path = choose_base_strategy().ok().map(|strategy| {
      strategy
        .state_dir()
        .unwrap_or_else(|| strategy.data_dir())
        .join("rom")
        .join("producers-v1")
    });
    let text = path
      .as_ref()
      .and_then(|path| fs::read_to_string(path).ok())
      .unwrap_or_default();
    let links = text
      .lines()
      .filter_map(|line| line.split_once(' '))
      .map(|(producer, drv)| (PathBuf::from(producer), PathBuf::from(drv)))
      .filter(|(_, drv)| drv.exists())
      .collect::<HashMap<PathBuf, PathBuf>>();

    let known = Self {
      path,
      file: None,
      links,
    };
    if known.links.len() < text.lines().count()
      && let Err(error) = known.rewrite()
    {
      tracing::debug!("cannot prune known producers: {error}");
    }
    known
  }

  /// Rewrites the file with only the links whose derivation still exists.
  fn rewrite(&self) -> io::Result<()> {
    let Some(path) = &self.path else {
      return Ok(());
    };

    let mut file = BufWriter::new(File::create(path)?);
    self.links.iter().try_for_each(|(producer, drv)| {
      writeln!(file, "{} {}", producer.display(), drv.display())
    })?;
    file.flush()
  }

  fn remember(&mut self, producer: &Path, drv: &Path) {
    if self.links.contains_key(producer) {
      return;
    }
    self.links.insert(producer.to_path_buf(), drv.to_path_buf());

    if self.file.is_none()
      && let Some(path) = &self.path
    {
      self.file = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| OpenOptions::new().create(true).append(true).open(path))
        .inspect_err(|error| {
          tracing::debug!("cannot remember producers: {error}");
        })
        .ok();
    }
    if let Some(file) = &mut self.file
      && let Err(error) =
        writeln!(file, "{} {}", producer.display(), drv.display())
    {
      tracing::debug!("cannot remember a producer: {error}");
    }
  }
}

#[derive(Default)]
struct LookupQueue {
  attach: VecDeque<PathBuf>,
  built:  VecDeque<PathBuf>,
  closed: bool,
}

impl Drop for TraceWorker {
  fn drop(&mut self) {
    let (queue, ready) = &*self.lookups;
    if let Ok(mut queue) = queue.lock() {
      queue.closed = true;
    }
    ready.notify_all();
    if let Ok(live) = LIVE_LOOKUPS.lock() {
      live.iter().copied().for_each(kill_lookup);
    }
  }
}

impl TraceWorker {
  fn spawn() -> Self {
    let (requests, pending) = mpsc::channel::<TraceRequest>();
    let (answered, answers) = mpsc::channel();
    let lookups =
      Arc::new((Mutex::new(LookupQueue::default()), Condvar::new()));
    let known = Mutex::new(KnownProducers::load());
    for _ in 0..PARALLEL_LOOKUPS {
      let queued = Arc::clone(&lookups);
      let answered = answered.clone();
      let lookup_thread = thread::Builder::new()
        .name("rom-producers".to_owned())
        .spawn(move || {
          loop {
            let Some(batch) = next_batch(&queued) else {
              return;
            };

            let mut answers = Vec::new();
            look_up_batch(batch, &mut answers);
            for answer in answers {
              if answered.send(answer).is_err() {
                return;
              }
            }
          }
        });
      if let Err(error) = lookup_thread {
        tracing::warn!("cannot read the build trace: {error}");
      }
    }
    let traced = answered.clone();
    let spawned = thread::Builder::new()
      .name("rom-build-trace".to_owned())
      .spawn(move || {
        while let Ok(first) = pending.recv() {
          let mut drvs = Vec::new();
          let mut batch = Vec::new();
          for request in iter::once(first).chain(pending.try_iter()) {
            match request {
              TraceRequest::Writer {
                drv,
                producers: candidates,
              } => {
                batch.extend(find_writer(&drv, candidates).map(|producer| {
                  Answer::Produced(ProducedDrv {
                    producer,
                    drv: Some(drv),
                  })
                }));
              },
              TraceRequest::Realised(paths) => drvs.extend(paths),
            }
          }
          check_realised(drvs, &mut batch);
          for answer in batch {
            if traced.send(answer).is_err() {
              return;
            }
          }
        }
      });
    if let Err(error) = spawned {
      tracing::warn!("cannot read the build trace: {error}");
    }
    Self {
      requests,
      lookups,
      known,
      answered,
      answers: Mutex::new(answers),
    }
  }
}

impl FilesystemResolver {
  fn request(&self, request: TraceRequest) {
    // A send only fails once the worker is gone, which it already reported.
    let _sent = self
      .worker
      .get_or_init(TraceWorker::spawn)
      .requests
      .send(request);
  }
}

impl DerivationResolver for FilesystemResolver {
  fn resolve(
    &self,
    path: &Path,
  ) -> std::result::Result<cognos::ParsedDerivation, cognos::aterm::Error> {
    cognos::parse_drv_file(path)
  }

  fn request_produced(
    &self,
    mut producers: Vec<PathBuf>,
    priority: LookupPriority,
  ) {
    let worker = self.worker.get_or_init(TraceWorker::spawn);
    if let Ok(known) = worker.known.lock() {
      producers.retain(|producer| {
        let Some(drv) = known.links.get(producer).cloned() else {
          return true;
        };
        let answer = Answer::Produced(ProducedDrv {
          producer: producer.clone(),
          drv:      Some(drv),
        });
        worker.answered.send(answer).is_err()
      });
    }

    let (queue, ready) = &*worker.lookups;
    let Ok(mut queue) = queue.lock() else {
      return;
    };
    match priority {
      LookupPriority::Attach => queue.attach.extend(producers),
      LookupPriority::Built => queue.built.extend(producers),
    }
    ready.notify_all();
  }

  fn remember_produced(&self, producer: &Path, drv: &Path) {
    let worker = self.worker.get_or_init(TraceWorker::spawn);
    if let Ok(mut known) = worker.known.lock() {
      known.remember(producer, drv);
    }
  }

  fn request_writer(&self, drv: PathBuf, producers: Vec<PathBuf>) {
    self.request(TraceRequest::Writer { drv, producers });
  }

  fn request_realised(&self, drvs: Vec<PathBuf>) {
    self.request(TraceRequest::Realised(drvs));
  }

  fn answers(&self) -> Vec<Answer> {
    let Some(worker) = self.worker.get() else {
      return Vec::new();
    };
    let Ok(answers) = worker.answers.lock() else {
      return Vec::new();
    };
    answers.try_iter().collect()
  }
}

/// Nix lists a batch's realised paths out of request order and fails it if
/// one is unbuilt, so a hit is halved down to the one producer.
fn find_writer(drv: &Path, producers: Vec<PathBuf>) -> Option<PathBuf> {
  if producers.is_empty() || TRACE_UNAVAILABLE.load(Ordering::Relaxed) {
    return None;
  }
  let found = match build_trace(&producers) {
    Ok(entries) => {
      entries
        .iter()
        .any(|entry| entry.opaque_path.as_deref() == Some(drv))
    },
    Err(TraceError::Unbuilt) => producers.len() > 1,
    Err(TraceError::Unavailable(reason)) => {
      report_unavailable(&reason);
      false
    },
  };
  if !found {
    return None;
  }
  let mut first = producers;
  if first.len() == 1 {
    return first.pop();
  }
  let second = first.split_off(first.len() / 2);
  find_writer(drv, first).or_else(|| find_writer(drv, second))
}

static TRACE_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

fn report_unavailable(reason: &str) {
  if !TRACE_UNAVAILABLE.swap(true, Ordering::Relaxed) {
    tracing::warn!("cannot link dynamic derivations to consumers: {reason}");
  }
}

/// Answers whether each derivation is realised, halving any batch Nix
/// rejects, since one unbuilt derivation fails the whole call.
fn check_realised(drvs: Vec<PathBuf>, answers: &mut Vec<Answer>) {
  if drvs.is_empty() {
    return;
  }
  let realised = build_trace(&drvs).is_ok();
  if realised || drvs.len() == 1 {
    answers.extend(
      drvs
        .into_iter()
        .map(|drv| Answer::Realised(RealisedDrv { drv, realised })),
    );
    return;
  }
  let mut first = drvs;
  let second = first.split_off(first.len() / 2);
  check_realised(first, answers);
  check_realised(second, answers);
}

/// Waits for queued producer lookups and takes a batch, attaching ones first,
/// or returns `None` once the worker is dropped.
fn next_batch(lookups: &(Mutex<LookupQueue>, Condvar)) -> Option<Vec<PathBuf>> {
  let (queue, ready) = lookups;
  let mut queue = queue.lock().ok()?;
  loop {
    if queue.closed {
      return None;
    }

    if let Some(producer) = queue.attach.pop_front() {
      return Some(vec![producer]);
    }
    if !queue.built.is_empty() {
      let count = queue.built.len().min(BATCH_LOOKUPS);
      return Some(queue.built.drain(..count).collect());
    }

    queue = ready.wait(queue).ok()?;
  }
}

const PARALLEL_LOOKUPS: usize = 2;
const REALISED_DELAY: f64 = 2.0;
const BATCH_LOOKUPS: usize = 64;
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(60);

/// Lookups still running, killed when their worker is dropped.
static LIVE_LOOKUPS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Returns a `nix` command that the kernel kills along with rom's lookup
/// thread, since Nix can spin on some lookups for good.
fn nix_command() -> Command {
  #[cfg(target_os = "linux")]
  {
    use std::env;

    static SETPRIV: OnceLock<bool> = OnceLock::new();
    let setpriv = *SETPRIV.get_or_init(|| {
      let found = env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths).any(|dir| dir.join("setpriv").is_file())
      });
      if !found {
        tracing::warn!(
          "setpriv not found, so nix lookups outlive a killed rom"
        );
      }
      found
    });
    if setpriv {
      let mut command = Command::new("setpriv");
      command.args(["--pdeathsig", "KILL", "--", "nix"]);
      return command;
    }
  }
  Command::new("nix")
}

/// Kills a lookup, which must still be in the locked `LIVE_LOOKUPS`.
fn kill_lookup(pid: u32) {
  if let Ok(target) = libc::pid_t::try_from(pid) {
    // SAFETY: kill only sends a signal, and a listed lookup is not reaped, so
    // its PID cannot have been reused.
    unsafe {
      libc::kill(target, libc::SIGKILL);
    }
  }
}

/// Runs a Nix lookup, killed past the timeout. No pre-exec hook, which would
/// make std fork rather than `posix_spawn`.
fn run_lookup(command: &mut Command) -> io::Result<process::Output> {
  command
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

  let child = command.spawn()?;
  let pid = child.id();
  if let Ok(mut live) = LIVE_LOOKUPS.lock() {
    live.push(pid);
  }
  let (sender, receiver) = mpsc::channel();
  thread::spawn(move || sender.send(collect_lookup(child)));

  if let Ok(output) = receiver.recv_timeout(LOOKUP_TIMEOUT) {
    return output;
  }

  if let Ok(live) = LIVE_LOOKUPS.lock()
    && live.contains(&pid)
  {
    kill_lookup(pid);
  }
  drop(receiver.recv());
  Err(io::Error::from(io::ErrorKind::TimedOut))
}

/// Reads a lookup's output and reaps it only after it leaves `LIVE_LOOKUPS`.
fn collect_lookup(mut child: Child) -> io::Result<process::Output> {
  let mut errors = child.stderr.take();
  let stderr = thread::spawn(move || {
    let mut bytes = Vec::new();
    if let Some(pipe) = &mut errors {
      pipe.read_to_end(&mut bytes)?;
    }
    Ok::<Vec<u8>, io::Error>(bytes)
  });
  let mut stdout = Vec::new();
  if let Some(pipe) = &mut child.stdout {
    pipe.read_to_end(&mut stdout)?;
  }
  let stderr = stderr
    .join()
    .map_err(|_| io::Error::other("lookup stderr reader panicked"))??;

  let pid = child.id();
  loop {
    // SAFETY: `siginfo_t` is plain data that `waitid` fills in.
    let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
    // SAFETY: `WNOWAIT` leaves the child unreaped, and `waitid` writes only
    // to `info`.
    let waited = unsafe {
      libc::waitid(
        libc::P_PID,
        pid,
        &raw mut info,
        libc::WEXITED | libc::WNOWAIT,
      )
    };
    if waited == 0 {
      break;
    }
    let error = io::Error::last_os_error();
    if error.kind() != io::ErrorKind::Interrupted {
      return Err(error);
    }
  }
  if let Ok(mut live) = LIVE_LOOKUPS.lock() {
    live.retain(|&running| running != pid);
  }

  let status = child.wait()?;
  Ok(process::Output {
    status,
    stdout,
    stderr,
  })
}

/// Answers a batch of producers in one `nix path-info`, halving any batch Nix
/// rejects, since one unbuilt producer fails the whole call.
fn look_up_batch(producers: Vec<PathBuf>, answers: &mut Vec<Answer>) {
  if producers.is_empty() {
    return;
  }

  // Nix prints realised paths out of request order, so a batch holds one
  // producer per output name to map each line back.
  let mut names = HashSet::new();
  let (batch, rest) =
    producers
      .into_iter()
      .partition::<Vec<PathBuf>, _>(|producer| {
        names.insert(output_name(producer))
      });
  look_up_batch(rest, answers);

  let unanswered = |batch: Vec<PathBuf>, answers: &mut Vec<Answer>| {
    answers.extend(batch.into_iter().map(|producer| {
      Answer::Produced(ProducedDrv {
        producer,
        drv: None,
      })
    }));
  };
  let halve = |mut first: Vec<PathBuf>, answers: &mut Vec<Answer>| {
    if first.len() == 1 {
      unanswered(first, answers);
      return;
    }
    let second = first.split_off(first.len() / 2);
    look_up_batch(first, answers);
    look_up_batch(second, answers);
  };
  if TRACE_UNAVAILABLE.load(Ordering::Relaxed) {
    unanswered(batch, answers);
    return;
  }

  let output = match run_lookup(
    nix_command().args(["path-info", "--max-jobs", "0"]).args(
      batch
        .iter()
        .map(|producer| format!("{}^out", producer.display())),
    ),
  ) {
    Ok(output) => output,
    Err(error) if error.kind() == io::ErrorKind::TimedOut => {
      halve(batch, answers);
      return;
    },
    Err(error) => {
      report_unavailable(&error.to_string());
      unanswered(batch, answers);
      return;
    },
  };

  if output.status.success() {
    let mut realised = String::from_utf8_lossy(&output.stdout)
      .lines()
      .map(|line| PathBuf::from(line.trim()))
      .map(|drv| (store_name(&drv).to_owned(), drv))
      .collect::<HashMap<String, PathBuf>>();
    answers.extend(batch.into_iter().map(|producer| {
      let drv = realised.remove(&output_name(&producer));
      Answer::Produced(ProducedDrv { producer, drv })
    }));
    return;
  }

  let stderr = String::from_utf8_lossy(&output.stderr);
  if !unbuilt(&stderr) {
    report_unavailable(stderr.trim());
    unanswered(batch, answers);
    return;
  }
  halve(batch, answers);
}

fn store_name(path: &Path) -> &str {
  path
    .file_name()
    .and_then(|name| name.to_str())
    .and_then(|name| name.split_once('-'))
    .map_or("", |(_, name)| name)
}

/// Returns the name of the derivation a producer writes, which is its own
/// without the last `.drv`.
fn output_name(producer: &Path) -> String {
  let name = store_name(producer);
  name.strip_suffix(".drv").unwrap_or(name).to_owned()
}

enum TraceError {
  Unbuilt,
  Unavailable(String),
}

/// An entry of `nix store build-trace info --json`, whose trailing entry is
/// each installable's realised path.
#[derive(Deserialize)]
struct TraceEntry {
  #[serde(rename = "opaquePath")]
  opaque_path: Option<PathBuf>,
}

/// Reads Nix's build trace for each producer's `out`.
fn build_trace(
  producers: &[PathBuf],
) -> std::result::Result<Vec<TraceEntry>, TraceError> {
  let output = run_lookup(
    nix_command()
      .args(["store", "build-trace", "info", "--json", "--max-jobs", "0"])
      .args(
        producers
          .iter()
          .map(|producer| format!("{}^out", producer.display())),
      ),
  )
  .map_err(|error| {
    if error.kind() == io::ErrorKind::TimedOut {
      TraceError::Unbuilt
    } else {
      TraceError::Unavailable(error.to_string())
    }
  })?;

  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    return Err(if unbuilt(&stderr) {
      TraceError::Unbuilt
    } else {
      TraceError::Unavailable(stderr.trim().to_owned())
    });
  }

  serde_json::from_slice(&output.stdout)
    .map_err(|error| TraceError::Unavailable(error.to_string()))
}

/// Whether Nix failed only because something is not built yet, which with
/// `--max-jobs 0` it reports as either error.
fn unbuilt(stderr: &str) -> bool {
  stderr.contains("unbuilt derivation")
    || stderr.contains("Unable to start any build")
}

/// Returns the producer in Nix's warning that it skipped one while planning.
fn ignored_producer(json: &[u8]) -> Option<PathBuf> {
  const MARKER: &[u8] = b"Ignoring dynamic derivation ";
  let start = json
    .windows(MARKER.len())
    .position(|window| window == MARKER)?
    + MARKER.len();
  let rest = &json[start..];
  let end = rest.iter().position(|&byte| byte == b'^')?;
  std::str::from_utf8(&rest[..end]).ok().map(PathBuf::from)
}

/// Bytes or decoded logs which must be emitted exactly once by an adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
  /// Non-protocol bytes.
  ///
  /// Their content and order are never changed.
  Passthrough(Vec<u8>),
  /// A decoded protocol message or a nonfatal unsupported-record diagnostic.
  Log(LogLine),
}

/// Result of feeding one or more bytes into the engine.
#[derive(Debug, Default)]
pub struct Processed {
  /// Whether the state changed.
  pub changed: bool,
  /// Output to emit, in order.
  pub output:  Vec<Output>,
}

impl Processed {
  fn merge(&mut self, mut other: Self) {
    self.changed |= other.changed;
    self.output.append(&mut other.output);
  }

  fn passthrough(&mut self, bytes: &[u8]) {
    if bytes.is_empty() {
      return;
    }
    if let Some(Output::Passthrough(previous)) = self.output.last_mut() {
      previous.extend_from_slice(bytes);
    } else {
      self.output.push(Output::Passthrough(bytes.to_vec()));
    }
  }
}

fn diagnostic(message: String) -> Processed {
  Processed {
    changed: false,
    output:  vec![Output::Log(LogLine {
      prefix: String::new(),
      styled: message.clone(),
      plain:  message,
    })],
  }
}

/// The reusable ROM state machine.
///
/// It owns no terminal, threads, async runtime, or global clock. All adapters
/// feed it bytes and supply a timestamp.
pub struct Engine {
  state:                State,
  config:               EngineConfig,
  log_counts:           HashMap<Id, usize>,
  suppressed_logs:      HashSet<Id>,
  reported_unsupported: HashSet<UnsupportedRecord>,
  resolved_drvs:        HashSet<PathBuf>,
  producers:            Producers,
  next_retry:           Option<f64>,
  /// Planned content-addressed derivations not yet asked about, which wait
  /// until their planned inputs are realised, since Nix cannot resolve them
  /// before then.
  unchecked:            BTreeSet<DerivationId>,
  /// Derivations asked about.
  checking:             HashSet<DerivationId>,
  /// Realisation checks waiting out [`REALISED_DELAY`].
  realising:            Vec<(DerivationId, f64)>,
  resolver:             Option<Box<dyn DerivationResolver>>,
  usage:                Option<CgroupUsage>,
  next_sample:          f64,
}

impl Engine {
  /// Creates an engine with empty state.
  #[must_use]
  pub fn new(config: EngineConfig) -> Self {
    Self {
      state: State::new(),
      config,
      log_counts: HashMap::new(),
      suppressed_logs: HashSet::new(),
      reported_unsupported: HashSet::new(),
      resolved_drvs: HashSet::new(),
      producers: Producers::default(),
      next_retry: None,
      unchecked: BTreeSet::new(),
      checking: HashSet::new(),
      realising: Vec::new(),
      resolver: None,
      usage: None,
      next_sample: 0.0,
    }
  }

  /// Returns the current state.
  #[must_use]
  pub const fn state(&self) -> &State {
    &self.state
  }

  /// Marks a planned derivation as available without building it.
  ///
  /// Returns whether its status changed.
  pub fn mark_available(&mut self, id: DerivationId) -> bool {
    self.state.mark_available(id)
  }

  /// Marks planned derivations available once Nix has exited successfully.
  pub fn mark_planned_available(&mut self) {
    self.state.mark_planned_available();
  }

  /// Applies the resolver's answers and asks about planned
  /// content-addressed derivations whose inputs are now realised.
  ///
  /// Nix announces no units, so each package is asked about once its producer
  /// links, and a realised one settles every unit under it. Units of an
  /// unrealised package settle as their dependents start, so a cached unit
  /// reads waiting until then.
  ///
  /// Returns whether the state changed. Adapters call this while idle, since
  /// answers also arrive between records.
  pub fn poll_lookups(&mut self, now: f64) -> bool {
    let Some(resolver) = self.resolver.as_ref() else {
      return false;
    };
    let mut tree = DerivationTree {
      state:      &mut self.state,
      resolver:   resolver.as_ref(),
      resolved:   &mut self.resolved_drvs,
      producers:  &mut self.producers,
      next_retry: &mut self.next_retry,
    };
    let mut changed = false;
    let mut packages = Vec::new();
    for answer in tree.resolver.answers() {
      changed |= match answer {
        Answer::Produced(produced) => {
          let Some(written) = tree.link_produced(produced, now) else {
            continue;
          };
          packages.extend(
            tree
              .state
              .get_derivation_info(written)
              .and_then(|info| info.package(written)),
          );
          true
        },
        Answer::Realised(RealisedDrv { drv, realised }) => {
          let Some(id) = tree.state.derivation_id(&drv) else {
            continue;
          };
          self.checking.remove(&id);
          if realised {
            tree.state.mark_realised(id)
          } else {
            tree.state.mark_unbuilt(id)
          }
        },
      };
    }
    self.ask_realised(packages, now);
    self.retry_produced(now);

    let state = &self.state;
    let mut due = Vec::new();
    self.unchecked.retain(|&id| {
      let Some(info) = state.get_derivation_info(id) else {
        return false;
      };
      if !info.floating || !matches!(info.build_status, BuildStatus::Planned) {
        return false;
      }
      let ready = info.input_derivations.iter().all(|input| {
        state.get_derivation_info(*input).is_none_or(|input| {
          matches!(
            input.build_status,
            BuildStatus::Unknown
              | BuildStatus::Available
              | BuildStatus::Built { .. }
          )
        })
      });
      if ready {
        due.push(id);
      }
      !ready
    });
    self.ask_realised(due, now);
    self.flush_realised(now);
    changed
  }

  /// Looks up a producer Nix skipped while planning, which an earlier run
  /// built, so the chain under it links before its units start.
  fn ask_ignored(&mut self, path: &Path) {
    let Some(resolver) = self.resolver.as_ref() else {
      return;
    };
    let mut tree = DerivationTree {
      state:      &mut self.state,
      resolver:   resolver.as_ref(),
      resolved:   &mut self.resolved_drvs,
      producers:  &mut self.producers,
      next_retry: &mut self.next_retry,
    };
    tree.register(path, Vec::new());
    let Some(id) = tree.state.derivation_id(path) else {
      return;
    };
    if let Some(producer) = tree.producers.entries.get_mut(&id)
      && !std::mem::replace(&mut producer.requested, true)
    {
      tree
        .resolver
        .request_produced(vec![producer.path.clone()], LookupPriority::Attach);
    }
  }

  fn retry_produced(&mut self, now: f64) {
    if self.next_retry.is_none_or(|at| at > now) {
      return;
    }
    let Some(resolver) = self.resolver.as_ref() else {
      return;
    };
    let mut next = None::<f64>;
    let due = self
      .producers
      .entries
      .values_mut()
      .filter_map(|producer| {
        let at = producer.retry_at?;
        if at > now || producer.requested {
          next = Some(next.map_or(at, |next| next.min(at)));
          return None;
        }
        producer.retry_at = None;
        producer.lookups += 1;
        producer.requested = true;
        Some(producer.path.clone())
      })
      .collect::<Vec<PathBuf>>();
    self.next_retry = next;
    if !due.is_empty() {
      resolver.request_produced(due, LookupPriority::Built);
    }
  }

  /// Queues a realisation check, asked only if the derivation has not
  /// started by then.
  #[expect(clippy::float_arithmetic, reason = "timestamps are `f64` seconds")]
  fn ask_realised(&mut self, ids: Vec<DerivationId>, now: f64) {
    let checking = &mut self.checking;
    self.realising.extend(
      ids
        .into_iter()
        .filter(|&id| checking.insert(id))
        .map(|id| (id, now + REALISED_DELAY)),
    );
  }

  fn flush_realised(&mut self, now: f64) {
    let Some(resolver) = self.resolver.as_ref() else {
      return;
    };
    let (state, checking) = (&self.state, &mut self.checking);
    let mut drvs = Vec::new();
    self.realising.retain(|&(id, at)| {
      if at > now {
        return true;
      }
      match state.get_derivation_info(id) {
        // Nix spins on a realisation check of one with dynamic inputs.
        Some(info)
          if !info.dynamic
            && matches!(
              info.build_status,
              BuildStatus::Unknown | BuildStatus::Planned
            ) =>
        {
          drvs.push(info.name.path.clone());
        },
        Some(_) | None => {
          checking.remove(&id);
        },
      }
      false
    });
    if !drvs.is_empty() {
      resolver.request_realised(drvs);
    }
  }

  /// Sets the source of `.drv` metadata.
  pub fn set_resolver<R: DerivationResolver + 'static>(&mut self, resolver: R) {
    self.resolver = Some(Box::new(resolver));
  }

  /// Opts into sampling the CPU use of local builds.
  pub fn set_usage(&mut self, usage: CgroupUsage) {
    self.usage = Some(usage);
  }

  /// Samples the CPU use of local builds, at most once a second.
  ///
  /// Returns whether the state changed. Adapters call this while idle.
  #[expect(clippy::float_arithmetic, reason = "timestamps are `f64` seconds")]
  pub fn sample_usage(&mut self, now: f64) -> bool {
    let Some(usage) = self.usage.as_mut() else {
      return false;
    };
    if now < self.next_sample {
      return false;
    }

    self.next_sample = now + 1.0;

    let running = self
      .state
      .local_builds()
      .collect::<Vec<(DerivationId, &Path, f64)>>();

    let mut changed = false;
    for (id, cores) in usage.sample(&running, now) {
      changed |= self.state.set_build_cpu(id, cores);
    }
    changed
  }

  /// Returns the engine's configuration.
  #[must_use]
  pub const fn config(&self) -> &EngineConfig {
    &self.config
  }

  /// Opts into build-history estimates from an injected store.
  pub fn load_history(&mut self, history: &BuildReportCache) {
    self.state.replace_build_history(history.load());
  }

  /// Persists build history through an injected store.
  ///
  /// # Errors
  ///
  /// Returns an error if the store cannot be written.
  pub fn save_history(
    &self,
    history: &BuildReportCache,
  ) -> std::io::Result<()> {
    history.save(self.state.build_history())
  }

  /// Processes one complete input record received at `now`.
  ///
  /// Unsupported JSON produces an ordered diagnostic without changing build
  /// state.
  ///
  /// # Errors
  ///
  /// Returns an error if the record is not valid internal JSON, or if a record
  /// without the `@nix ` prefix reaches the decoder in auto-detect mode.
  pub fn process_record_at(
    &mut self,
    record: &[u8],
    now: f64,
  ) -> Result<Processed> {
    let record = record
      .strip_suffix(b"\n")
      .unwrap_or(record)
      .strip_suffix(b"\r")
      .unwrap_or_else(|| record.strip_suffix(b"\n").unwrap_or(record));
    if record.is_empty() {
      return Ok(Processed::default());
    }
    let json = match self.config.input_mode {
      InputMode::Auto => {
        record.strip_prefix(b"@nix ").ok_or(RomError::NotProtocol)?
      },
      InputMode::Json => record,
    };
    let action = match cognos::decode_action(json) {
      Ok(DecodedAction::Known(action)) => action,
      Ok(DecodedAction::Unsupported(record)) => {
        if self.reported_unsupported.contains(&record) {
          return Ok(Processed::default());
        }
        let kind = record
          .kind
          .map_or_else(String::new, |kind| format!(" type {kind}"));
        let message = format!(
          "rom: ignored unsupported internal-JSON {}{kind}",
          record.action.escape_debug()
        );
        self.reported_unsupported.insert(record);
        return Ok(diagnostic(message));
      },
      Err(error) => return Err(RomError::Json(error)),
    };
    let event = Event::decode(action);
    if let Some(producer) = ignored_producer(json) {
      self.ask_ignored(&producer);
    }
    let linked = self.poll_lookups(now);

    let show_log = match &event {
      Event::Message(message) => message.level <= self.config.verbosity,
      Event::Start(_) | Event::Stop { .. } | Event::Result { .. } => true,
    };

    let mut output = Vec::new();
    let effects = update::apply_event_at(&mut self.state, event, now);

    if show_log
      && let Some(log) = effects.log
      && (!self.config.silent || log.force)
      && let Some(log) = self.render_log(log)
    {
      output.push(Output::Log(log));
    }

    let mut changed = effects.changed || linked;
    if let Some(resolver) = self.resolver.as_ref() {
      let mut tree = DerivationTree {
        state:      &mut self.state,
        resolver:   resolver.as_ref(),
        resolved:   &mut self.resolved_drvs,
        producers:  &mut self.producers,
        next_retry: &mut self.next_retry,
      };
      for path in effects.resolve {
        changed |= tree.resolve(path);
      }
      if effects.stopped.is_some() {
        tree.request_produced(Lookup::Built(effects.built));
      }
      if let Some(id) = effects.started {
        tree.request_produced(Lookup::Consumer(id));
      }
      if let Some(id) = effects.started
        && let Some(info) = tree.state.get_derivation_info(id)
        && info.derivation_parents.is_empty()
      {
        tree.request_produced(Lookup::Orphan(id));
      }
    }
    if let Some(id) = effects.planned {
      self.unchecked.insert(id);
    }
    if let Some(id) = effects.resolved_derivation
      && self
        .state
        .get_derivation_info(id)
        .is_some_and(|info| matches!(info.build_status, BuildStatus::Planned))
    {
      self.unchecked.remove(&id);
      self.ask_realised(vec![id], now);
    }
    if let Some(id) = effects.stopped {
      self.log_counts.remove(&id);
      self.suppressed_logs.remove(&id);
    }
    Ok(Processed { changed, output })
  }

  fn render_log(&mut self, log: LogEffect) -> Option<LogLine> {
    let prefix = log.activity.map_or_else(String::new, |id| {
      self
        .state
        .get_activity_prefix(id, self.config.log_prefix_style)
        .unwrap_or_default()
    });
    if let Some(id) = log.activity {
      let count = self.log_counts.entry(id).or_default();
      if self
        .config
        .log_line_limit
        .is_some_and(|limit| *count >= limit)
      {
        return self.suppressed_logs.insert(id).then(|| {
          LogLine {
            prefix,
            styled: "… further build logs suppressed".to_owned(),
            plain: "… further build logs suppressed".to_owned(),
          }
        });
      }
      *count += 1;
    }
    Some(LogLine {
      prefix,
      styled: log.styled,
      plain: log.plain,
    })
  }

  /// Marks the input source as closed without pretending unfinished work
  /// succeeded.
  pub const fn finish(&mut self) {
    self.state.finish();
  }
}

#[derive(Default)]
struct Producers {
  entries:     HashMap<DerivationId, Producer>,
  by_consumer: HashMap<DerivationId, Vec<DerivationId>>,
  /// Keyed by the name of the derivation each one writes.
  by_name:     HashMap<String, Vec<DerivationId>>,
  /// Registered after their build stopped, so asked about at the next stop.
  built_late:  Vec<DerivationId>,
}

/// A dynamic-derivation producer whose output is not yet linked.
struct Producer {
  /// Its resolved copy once known, since Nix re-resolves an original's inputs.
  path:      PathBuf,
  consumers: Vec<DerivationId>,
  /// How many times it was asked about after its build.
  lookups:   u8,
  /// Whether a lookup is waiting on the resolver.
  requested: bool,
  /// When to ask again after an unbuilt answer.
  ///
  /// Nix records a content-addressed producer's output off-thread after its
  /// build activity stops, so a lookup right at the stop can be too early.
  retry_at:  Option<f64>,
}

/// Seconds before each retry of a built producer that answered unbuilt,
/// bounded since one with dynamic inputs answers so until its whole chain is
/// built.
const RETRY_DELAYS: [f64; 9] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 8.0, 8.0, 8.0];

/// How many times a built producer is asked about.
const BUILT_LOOKUPS: u8 = RETRY_DELAYS.len() as u8 + 1;

/// Most producers of a derivation looked up as soon as its own producer links,
/// which follows a chain an earlier run built.
const LINKED_LOOKUPS: usize = 64;

#[derive(Clone, Copy)]
enum Lookup {
  Built(Option<DerivationId>),
  /// Ask a last time about the producers of a build that started, which Nix
  /// only starts once they are realised.
  Consumer(DerivationId),
  /// Look up the producer of a build that started without a known parent,
  /// among those named after it.
  Orphan(DerivationId),
}

struct DerivationTree<'engine> {
  state:      &'engine mut State,
  resolver:   &'engine dyn DerivationResolver,
  resolved:   &'engine mut HashSet<PathBuf>,
  producers:  &'engine mut Producers,
  /// The earliest [`Producer::retry_at`].
  next_retry: &'engine mut Option<f64>,
}

impl DerivationTree<'_> {
  fn resolve(&mut self, root: PathBuf) -> bool {
    let mut pending = vec![root];
    let mut changed = false;
    while let Some(path) = pending.pop() {
      if self.resolved.contains(&path) {
        continue;
      }
      let Some(derivation) = path.to_str().and_then(Derivation::parse) else {
        continue;
      };
      let id = self.state.get_or_create_derivation_id(derivation);
      let parsed = match self.resolver.resolve(&path) {
        Ok(parsed) => parsed,
        Err(error) => {
          tracing::debug!(
            error = &error as &dyn std::error::Error,
            "could not resolve derivation"
          );
          continue;
        },
      };

      for producer in &parsed.dynamic_drvs {
        self.register(Path::new(producer), vec![id]);
      }
      pending.extend(
        parsed
          .input_drvs
          .iter()
          .map(|(dependency, _)| PathBuf::from(dependency)),
      );
      // Nix only lets a derivation named `x.drv` write a derivation.
      if self.state.get_derivation_info(id).is_some_and(|info| {
        Path::new(&info.name.name)
          .extension()
          .is_some_and(|extension| extension == "drv")
      }) {
        self.register(&path, Vec::new());
      }
      self.state.populate_parsed_derivation(id, parsed);
      self.state.adopt_inputs(id);
      self.resolved.insert(path);
      changed = true;
    }
    changed
  }

  fn register(&mut self, path: &Path, consumers: Vec<DerivationId>) {
    let Some(derivation) = path.to_str().and_then(Derivation::parse) else {
      return;
    };
    let id = self.state.get_or_create_derivation_id(derivation);
    let resolved = self
      .state
      .get_derivation_info(id)
      .is_some_and(|info| info.name.path != path);
    let Producers {
      entries,
      by_consumer,
      by_name,
      built_late,
    } = &mut *self.producers;
    let producer = match entries.entry(id) {
      Entry::Occupied(entry) => entry.into_mut(),
      Entry::Vacant(entry) => {
        if let Some(info) = self.state.get_derivation_info(id) {
          if let Some(name) = info.name.name.strip_suffix(".drv") {
            by_name.entry(name.to_owned()).or_default().push(id);
          }
          if matches!(info.build_status, BuildStatus::Built { .. }) {
            built_late.push(id);
          }
        }
        entry.insert(Producer {
          path:      path.to_path_buf(),
          consumers: Vec::new(),
          lookups:   0,
          requested: false,
          retry_at:  None,
        })
      },
    };
    if resolved {
      producer.path = path.to_path_buf();
    }
    for consumer in consumers {
      if !producer.consumers.contains(&consumer) {
        producer.consumers.push(consumer);
        by_consumer.entry(consumer).or_default().push(id);
      }
    }
  }

  fn request_produced(&mut self, lookup: Lookup) {
    let Producers {
      entries,
      by_consumer,
      by_name,
      built_late,
    } = &mut *self.producers;
    if entries.is_empty() {
      return;
    }

    match lookup {
      Lookup::Built(built) => {
        let due = built_late
          .drain(..)
          .chain(built)
          .filter_map(|id| {
            let producer = entries.get_mut(&id)?;
            let built =
              self.state.get_derivation_info(id).is_some_and(|info| {
                matches!(info.build_status, BuildStatus::Built { .. })
              });
            (built && !producer.requested && producer.lookups == 0).then(|| {
              producer.lookups += 1;
              producer.requested = true;
              producer.path.clone()
            })
          })
          .collect::<Vec<PathBuf>>();
        if !due.is_empty() {
          self.resolver.request_produced(due, LookupPriority::Built);
        }
      },
      Lookup::Consumer(consumer) => {
        let Some(ids) = by_consumer.get(&consumer) else {
          return;
        };
        let due = ids
          .iter()
          .filter_map(|id| {
            let producer = entries.get_mut(id)?;
            let ask = !producer.requested && producer.lookups < BUILT_LOOKUPS;
            producer.lookups = BUILT_LOOKUPS;
            producer.retry_at = None;
            ask.then(|| {
              producer.requested = true;
              producer.path.clone()
            })
          })
          .collect::<Vec<PathBuf>>();
        if !due.is_empty() {
          self.resolver.request_produced(due, LookupPriority::Attach);
        }
      },
      Lookup::Orphan(orphan) => {
        let Some(drv) = self.state.get_derivation_info(orphan) else {
          return;
        };
        let Some(ids) = by_name.get(&drv.name.name) else {
          return;
        };
        let candidates = ids
          .iter()
          .filter(|id| {
            self.state.get_derivation_info(**id).is_some_and(|info| {
              matches!(
                info.build_status,
                BuildStatus::Unknown
                  | BuildStatus::Available
                  | BuildStatus::Built { .. }
              )
            })
          })
          .filter_map(|id| entries.get(id))
          .map(|producer| producer.path.clone())
          .collect::<Vec<PathBuf>>();
        if !candidates.is_empty() {
          self
            .resolver
            .request_writer(drv.name.path.clone(), candidates);
        }
      },
    }
  }

  /// Returns the derivation the producer wrote, once linked.
  #[expect(clippy::float_arithmetic, reason = "timestamps are `f64` seconds")]
  fn link_produced(
    &mut self,
    answer: ProducedDrv,
    now: f64,
  ) -> Option<DerivationId> {
    let id = answer
      .producer
      .to_str()
      .and_then(Derivation::parse)
      .map(|derivation| self.state.get_or_create_derivation_id(derivation))?;
    let Some(drv) = answer.drv else {
      if let Some(producer) = self.producers.entries.get_mut(&id) {
        producer.requested = false;
        if let Some(delay) =
          RETRY_DELAYS.get(usize::from(producer.lookups).wrapping_sub(1))
        {
          let at = now + delay;
          producer.retry_at = Some(at);
          *self.next_retry =
            Some(self.next_retry.map_or(at, |next| next.min(at)));
        }
      }
      return None;
    };
    let derivation = drv.to_str().and_then(Derivation::parse)?;
    let producer = self.producers.entries.remove(&id)?;
    if let Some(info) = self.state.get_derivation_info(id) {
      self.resolver.remember_produced(&info.name.path, &drv);
    }
    self.state.mark_available(id);
    let produced_id = self.state.get_or_create_derivation_id(derivation);
    for &consumer in &producer.consumers {
      self.state.link_dependency(consumer, produced_id);
    }
    // A text output is named after its derivation, so a produced
    // `x.drv.drv` is itself the producer of `x.drv`.
    let stem = drv.file_stem().map(Path::new);
    if stem.is_some_and(|stem| stem.extension().is_some_and(|ext| ext == "drv"))
    {
      self.register(&drv, producer.consumers);
    }
    self.resolve(drv);
    self.state.join_package(id, produced_id);
    let due = self
      .producers
      .by_consumer
      .get(&produced_id)
      .into_iter()
      .flatten()
      .copied()
      .filter(|id| self.producers.entries.contains_key(id))
      .take(LINKED_LOOKUPS + 1)
      .collect::<Vec<DerivationId>>();
    if due.len() <= LINKED_LOOKUPS {
      let due = due
        .iter()
        .filter_map(|id| {
          let producer = self.producers.entries.get_mut(id)?;
          (!std::mem::replace(&mut producer.requested, true))
            .then(|| producer.path.clone())
        })
        .collect::<Vec<PathBuf>>();
      if !due.is_empty() {
        self.resolver.request_produced(due, LookupPriority::Attach);
      }
    }
    Some(produced_id)
  }
}

enum RecordState {
  Undecided(Vec<u8>),
  Structured(Vec<u8>),
  Passthrough,
}

/// Incremental framing around [`Engine`].
///
/// Non-protocol lines become passthrough as soon as their prefix differs from
/// `@nix `, so an arbitrarily long ordinary line is never accumulated.
pub struct StreamEngine {
  engine: Engine,
  record: RecordState,
}

impl StreamEngine {
  /// Creates a stream engine with empty state.
  #[must_use]
  pub fn new(config: EngineConfig) -> Self {
    let record = match config.input_mode {
      InputMode::Auto => RecordState::Undecided(Vec::with_capacity(5)),
      InputMode::Json => RecordState::Structured(Vec::new()),
    };
    Self {
      engine: Engine::new(config),
      record,
    }
  }

  /// Returns the wrapped engine.
  #[must_use]
  pub const fn engine(&self) -> &Engine {
    &self.engine
  }

  /// Returns the wrapped engine mutably.
  pub const fn engine_mut(&mut self) -> &mut Engine {
    &mut self.engine
  }

  /// Feeds input bytes received at `now`, processing every completed record.
  ///
  /// # Errors
  ///
  /// Returns an error if a structured record exceeds the configured size limit
  /// or cannot be processed.
  pub fn push_at(&mut self, bytes: &[u8], now: f64) -> Result<Processed> {
    const PREFIX: &[u8] = b"@nix ";
    let mut processed = Processed::default();
    let mut offset = 0;
    while offset < bytes.len() {
      match &mut self.record {
        RecordState::Undecided(prefix) => {
          let byte = bytes[offset];
          prefix.push(byte);
          offset += 1;
          let still_prefix = PREFIX.starts_with(prefix);
          if !still_prefix || byte == b'\n' {
            processed.passthrough(prefix);
            prefix.clear();
            self.record = if byte == b'\n' {
              RecordState::Undecided(Vec::with_capacity(PREFIX.len()))
            } else {
              RecordState::Passthrough
            };
          } else if prefix.len() == PREFIX.len() {
            self.record = RecordState::Structured(std::mem::take(prefix));
          }
        },
        RecordState::Structured(record) => {
          let rest = &bytes[offset..];
          let newline = rest.iter().position(|byte| *byte == b'\n');
          let take = newline.map_or(rest.len(), |position| position + 1);
          record.extend_from_slice(&rest[..take]);
          offset += take;
          if record.len() > self.engine.config.max_record_bytes {
            return Err(RomError::RecordTooLarge {
              limit: self.engine.config.max_record_bytes,
            });
          }
          if newline.is_some() {
            let complete = std::mem::take(record);
            processed.merge(self.engine.process_record_at(&complete, now)?);
            self.record = match self.engine.config.input_mode {
              InputMode::Auto => {
                RecordState::Undecided(Vec::with_capacity(PREFIX.len()))
              },
              InputMode::Json => RecordState::Structured(Vec::new()),
            };
          }
        },
        RecordState::Passthrough => {
          let rest = &bytes[offset..];
          if let Some(position) = rest.iter().position(|byte| *byte == b'\n') {
            processed.passthrough(&rest[..=position]);
            offset += position + 1;
            self.record =
              RecordState::Undecided(Vec::with_capacity(PREFIX.len()));
          } else {
            processed.passthrough(rest);
            offset = bytes.len();
          }
        },
      }
    }
    Ok(processed)
  }

  /// Processes any unterminated final record and marks the input as closed.
  ///
  /// # Errors
  ///
  /// Returns an error if the final structured record cannot be processed.
  pub fn finish_at(&mut self, now: f64) -> Result<Processed> {
    let mut processed = Processed::default();
    match std::mem::replace(
      &mut self.record,
      RecordState::Undecided(Vec::new()),
    ) {
      RecordState::Undecided(bytes) => processed.passthrough(&bytes),
      RecordState::Structured(bytes) if !bytes.is_empty() => {
        processed.merge(self.engine.process_record_at(&bytes, now)?);
      },
      RecordState::Structured(_) | RecordState::Passthrough => {},
    }
    self.engine.finish();
    Ok(processed)
  }
}

/// Append-only stream adapter.
///
/// It writes logs immediately and one final plain presentation; it never emits
/// cursor-control sequences.
pub struct Monitor<W: Write> {
  stream: StreamEngine,
  writer: W,
  render: RenderConfig,
}

impl<W: Write> Monitor<W> {
  /// Creates a monitor that writes to `writer`.
  #[must_use]
  pub fn new(config: Config, writer: W) -> Self {
    let Config { engine, render } = config;
    Self {
      stream: StreamEngine::new(engine),
      writer,
      render,
    }
  }

  /// Returns the current state.
  #[must_use]
  pub const fn state(&self) -> &State {
    self.stream.engine().state()
  }

  /// Feeds input bytes received at `now` and writes the resulting output.
  ///
  /// # Errors
  ///
  /// Returns an error if the input cannot be processed or the output cannot be
  /// written.
  pub fn process_bytes_at(&mut self, bytes: &[u8], now: f64) -> Result<()> {
    let processed = self.stream.push_at(bytes, now)?;
    write_outputs(&mut self.writer, processed.output, &self.render)
  }

  /// Returns the engine mutably.
  pub const fn engine_mut(&mut self) -> &mut Engine {
    self.stream.engine_mut()
  }

  /// Monitors `reader` until it ends, then writes the final presentation.
  ///
  /// # Errors
  ///
  /// Returns an error if reading, processing, or writing fails, if a build
  /// failed, or if the input ended while work was still active.
  pub fn process_stream<R: BufRead>(&mut self, mut reader: R) -> Result<()> {
    let mut bytes = [0_u8; 16 * 1024];
    loop {
      let count = reader.read(&mut bytes)?;
      if count == 0 {
        break;
      }
      self.process_bytes_at(&bytes[..count], current_time())?;
    }
    let final_output = self.stream.finish_at(current_time())?;
    write_outputs(&mut self.writer, final_output.output, &self.render)?;
    write_final(
      &mut self.writer,
      self.stream.engine().state(),
      &self.render,
      current_time(),
    )?;
    if self.stream.engine().state().has_errors() {
      return Err(RomError::BuildFailed);
    }
    if self.stream.engine().state().has_unfinished() {
      return Err(RomError::Incomplete);
    }
    Ok(())
  }
}

fn write_outputs<W: Write>(
  writer: &mut W,
  output: Vec<Output>,
  render: &RenderConfig,
) -> Result<()> {
  for item in output {
    match item {
      Output::Passthrough(bytes) => writer.write_all(&bytes)?,
      Output::Log(line) => {
        let line = format_log(&line, render);
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
      },
    }
  }
  writer.flush()?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn arbitrary_passthrough_does_not_accumulate_a_line() {
    let mut stream = StreamEngine::new(EngineConfig::default());
    let first = stream.push_at(b"ordinary", 0.0).unwrap();
    assert_eq!(first.output, vec![Output::Passthrough(
      b"ordinary".to_vec()
    )]);
    let second = stream.push_at(b" bytes\n", 0.1).unwrap();
    assert_eq!(second.output, vec![Output::Passthrough(
      b" bytes\n".to_vec()
    )]);
  }

  #[test]
  fn malformed_claimed_json_is_fatal() {
    let mut stream = StreamEngine::new(EngineConfig::default());
    stream.push_at(b"@nix nope\n", 0.0).unwrap_err();
    assert!(!stream.engine().state().has_errors());
  }

  #[test]
  fn oversized_structured_record_is_fatal() {
    let config = EngineConfig {
      max_record_bytes: 8,
      ..EngineConfig::default()
    };
    let mut stream = StreamEngine::new(config);
    stream
      .push_at(b"@nix {\"action\":\"stop\",\"id\":1}\n", 0.0)
      .unwrap_err();
  }

  #[test]
  fn long_passthrough_ignores_structured_record_limit() {
    let config = EngineConfig {
      max_record_bytes: 8,
      ..EngineConfig::default()
    };
    let mut stream = StreamEngine::new(config);
    stream.push_at(&vec![b'x'; 1024], 0.0).unwrap();
  }
}
