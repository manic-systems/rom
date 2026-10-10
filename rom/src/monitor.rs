//! Shared synchronous ingestion engine and append-only stream adapter.

use std::{
  collections::{HashMap, HashSet},
  io::{BufRead, Write},
  path::{Path, PathBuf},
  process::{Command, Stdio},
  sync::atomic::{AtomicBool, Ordering},
};

use cognos::{DecodedAction, Id, UnsupportedRecord};
use serde::Deserialize;

use crate::{
  cache::BuildReportCache,
  display::{format_log, write_final},
  error::{Result, RomError},
  event::Event,
  state::{BuildStatus, Derivation, DerivationId, State, current_time},
  types::{Config, EngineConfig, InputMode, LogLine, RenderConfig},
  update::{self, LogEffect},
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
  /// Returns a description of the failure if the derivation cannot be read or
  /// parsed.
  fn resolve(
    &self,
    path: &Path,
  ) -> std::result::Result<cognos::ParsedDerivation, String>;

  /// Returns the `.drv` a dynamic-derivation producer wrote to its `out`, once
  /// Nix has realised it.
  fn produced(&self, _producer: &Path) -> Option<PathBuf> {
    None
  }
}

/// Resolver used by the CLI for local store derivations.
pub struct FilesystemResolver;

impl DerivationResolver for FilesystemResolver {
  fn resolve(
    &self,
    path: &Path,
  ) -> std::result::Result<cognos::ParsedDerivation, String> {
    cognos::parse_drv_file(path)
  }

  /// Returns the `.drv` a dynamic-derivation producer wrote to its `out`, read
  /// from Nix's build trace.
  ///
  /// Produced derivations have no deriver, so the realisation is the only
  /// record of which producer wrote them.
  fn produced(&self, producer: &Path) -> Option<PathBuf> {
    // The trace lists entries keyed by resolved derivations, including the
    // producer's own CA inputs, then the installable's realised path.
    #[derive(Deserialize)]
    struct Entry {
      #[serde(rename = "opaquePath")]
      opaque_path: Option<PathBuf>,
    }

    static UNAVAILABLE: AtomicBool = AtomicBool::new(false);
    let disable = |reason: &str| {
      if !UNAVAILABLE.swap(true, Ordering::Relaxed) {
        tracing::warn!(
          "cannot link dynamic derivations to consumers: {reason}"
        );
      }
    };
    if UNAVAILABLE.load(Ordering::Relaxed) {
      return None;
    }

    let output = Command::new("nix")
      .args(["store", "build-trace", "info", "--json", "--max-jobs", "0"])
      .arg(format!("{}^out", producer.display()))
      .stdin(Stdio::null())
      .output();
    let output = match output {
      Ok(output) => output,
      Err(error) => {
        disable(&error.to_string());
        return None;
      },
    };
    if !output.status.success() {
      let stderr = String::from_utf8_lossy(&output.stderr);
      if !stderr.contains("unbuilt derivation") {
        disable(stderr.trim());
      }
      return None;
    }
    let entries: Vec<Entry> = match serde_json::from_slice(&output.stdout) {
      Ok(entries) => entries,
      Err(error) => {
        disable(&error.to_string());
        return None;
      },
    };
    entries.into_iter().find_map(|entry| entry.opaque_path)
  }
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
  producers:            HashMap<DerivationId, Producer>,
  resolver:             Option<Box<dyn DerivationResolver>>,
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
      producers: HashMap::new(),
      resolver: None,
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

  /// Sets the source of `.drv` metadata.
  pub fn set_resolver(&mut self, resolver: impl DerivationResolver + 'static) {
    self.resolver = Some(Box::new(resolver));
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
        record.strip_prefix(b"@nix ").ok_or_else(|| {
          RomError::parse("internal error: non-protocol record reached decoder")
        })?
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

    let mut changed = effects.changed;
    if let Some(resolver) = self.resolver.as_ref() {
      let mut tree = DerivationTree {
        state:     &mut self.state,
        resolver:  resolver.as_ref(),
        resolved:  &mut self.resolved_drvs,
        producers: &mut self.producers,
      };
      for path in effects.resolve {
        changed |= tree.resolve(path);
      }
      if effects.stopped.is_some() {
        changed |= tree.link_producers(Lookup::Built);
      }
      if let Some(id) = effects.started
        && let Some(info) = tree.state.get_derivation_info(id)
        && info.derivation_parents.is_empty()
      {
        changed |= tree.link_producers(Lookup::Orphan(id));
      }
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
        .get_activity_prefix(id, &self.config.log_prefix_style)
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
  pub fn finish(&mut self) {
    self.state.finish();
  }
}

/// A dynamic-derivation producer whose output is not yet linked.
struct Producer {
  path:      PathBuf,
  consumers: Vec<DerivationId>,
  /// Whether a [`Lookup::Built`] has run for this producer.
  ///
  /// Every activity stop triggers one, so each producer gets one.
  looked_up: bool,
}

#[derive(Clone, Copy)]
enum Lookup {
  Built,
  /// Look up the producer of a build that started without a known parent.
  ///
  /// Whichever producer wrote it is realised even if its trace was not yet
  /// recorded when it stopped, or it was built by an earlier run. Its producer
  /// is named after it plus `.drv`.
  Orphan(DerivationId),
}

struct DerivationTree<'engine> {
  state:     &'engine mut State,
  resolver:  &'engine dyn DerivationResolver,
  resolved:  &'engine mut HashSet<PathBuf>,
  producers: &'engine mut HashMap<DerivationId, Producer>,
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
          tracing::debug!("could not resolve {}: {error}", path.display());
          continue;
        },
      };

      // A dynamic input names its producer with no static outputs.
      for (dependency, outputs) in &parsed.input_drvs {
        let dependency = PathBuf::from(dependency);
        if outputs.is_empty() {
          self.register(&dependency, vec![id]);
        }
        pending.push(dependency);
      }
      self.state.populate_parsed_derivation(id, parsed);
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
    self
      .producers
      .entry(id)
      .or_insert_with(|| {
        Producer {
          path:      path.to_path_buf(),
          consumers: Vec::new(),
          looked_up: false,
        }
      })
      .consumers
      .extend(consumers);
  }

  fn link_producers(&mut self, lookup: Lookup) -> bool {
    if self.producers.is_empty() {
      return false;
    }

    let due: Vec<_> = self
      .producers
      .iter_mut()
      .filter_map(|(&id, producer)| {
        let info = self.state.get_derivation_info(id)?;
        let due = match lookup {
          Lookup::Built => {
            matches!(info.build_status, BuildStatus::Built { .. })
              && !std::mem::replace(&mut producer.looked_up, true)
          },
          Lookup::Orphan(orphan_id) => {
            let orphan = &self.state.get_derivation_info(orphan_id)?.name.name;
            info.name.name.strip_suffix(".drv") == Some(orphan.as_str())
              && matches!(
                info.build_status,
                BuildStatus::Unknown
                  | BuildStatus::Available
                  | BuildStatus::Built { .. }
              )
          },
        };
        due.then_some(id)
      })
      .collect();

    let mut changed = false;
    for id in due {
      let Some(produced) = self.resolver.produced(&self.producers[&id].path)
      else {
        continue;
      };
      let Some(derivation) = produced.to_str().and_then(Derivation::parse)
      else {
        continue;
      };
      let Some(producer) = self.producers.remove(&id) else {
        continue;
      };
      self.state.mark_available(id);
      let produced_id = self.state.get_or_create_derivation_id(derivation);
      for &consumer in &producer.consumers {
        self.state.link_dependency(consumer, produced_id);
      }
      // A text output is named after its derivation, so a produced
      // `x.drv.drv` is itself the producer of `x.drv`.
      let stem = produced.file_stem().and_then(|stem| stem.to_str());
      if stem.is_some_and(|stem| stem.ends_with(".drv")) {
        self.register(&produced, producer.consumers);
      }
      changed |= self.resolve(produced);
    }
    changed
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
  pub fn engine_mut(&mut self) -> &mut Engine {
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
            return Err(RomError::parse(format!(
              "internal-JSON record exceeds {} bytes",
              self.engine.config.max_record_bytes
            )));
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
  pub fn engine_mut(&mut self) -> &mut Engine {
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
    assert!(stream.push_at(b"@nix nope\n", 0.0).is_err());
    assert!(!stream.engine().state().has_errors());
  }

  #[test]
  fn oversized_structured_record_is_fatal() {
    let config = EngineConfig {
      max_record_bytes: 8,
      ..EngineConfig::default()
    };
    let mut stream = StreamEngine::new(config);
    assert!(
      stream
        .push_at(b"@nix {\"action\":\"stop\",\"id\":1}\n", 0.0)
        .is_err()
    );
  }

  #[test]
  fn long_passthrough_ignores_structured_record_limit() {
    let config = EngineConfig {
      max_record_bytes: 8,
      ..EngineConfig::default()
    };
    let mut stream = StreamEngine::new(config);
    assert!(stream.push_at(&vec![b'x'; 1024], 0.0).is_ok());
  }
}
