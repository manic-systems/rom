//! Shared synchronous ingestion engine and append-only stream adapter.

use std::{
  collections::{HashMap, HashSet},
  io::{BufRead, Write},
  path::{Path, PathBuf},
};

use cognos::{Actions, Id};

use crate::{
  cache::BuildReportCache,
  display::{format_log, write_final},
  error::{Result, RomError},
  event::Event,
  state::{Derivation, State, current_time},
  types::{Config, EngineConfig, InputMode, LogLine, RenderConfig},
  update::{self, LogEffect},
};

/// Optional source of `.drv` metadata. Resolver failures are diagnostic-only;
/// the activity remains visible as a truthful root rather than aborting work.
pub trait DerivationResolver: Send + Sync {
  fn resolve(
    &self,
    path: &Path,
  ) -> std::result::Result<cognos::ParsedDerivation, String>;
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
}

/// Bytes or decoded logs which must be emitted exactly once by an adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
  /// Non-protocol bytes. Their content and order are never changed.
  Passthrough(Vec<u8>),
  /// A message decoded from an internal-JSON action.
  Log(LogLine),
}

/// Result of feeding one or more bytes into the engine.
#[derive(Debug, Default)]
pub struct Processed {
  pub changed: bool,
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

/// The reusable ROM state machine.
///
/// It owns no terminal, threads, async runtime, or global clock. All adapters
/// feed it bytes and supply a timestamp.
pub struct Engine {
  state:           State,
  config:          EngineConfig,
  log_counts:      HashMap<Id, usize>,
  suppressed_logs: HashSet<Id>,
  resolved_drvs:   HashSet<PathBuf>,
  resolver:        Option<Box<dyn DerivationResolver>>,
}

impl Engine {
  #[must_use]
  pub fn new(config: EngineConfig) -> Self {
    Self {
      state: State::new(),
      config,
      log_counts: HashMap::new(),
      suppressed_logs: HashSet::new(),
      resolved_drvs: HashSet::new(),
      resolver: None,
    }
  }

  #[must_use]
  pub const fn state(&self) -> &State {
    &self.state
  }

  pub fn set_resolver(&mut self, resolver: impl DerivationResolver + 'static) {
    self.resolver = Some(Box::new(resolver));
  }

  #[must_use]
  pub const fn config(&self) -> &EngineConfig {
    &self.config
  }

  /// Opt into build-history estimates from an injected store.
  pub fn load_history(&mut self, history: &BuildReportCache) {
    self.state.replace_build_history(history.load());
  }

  /// Persist build history through an injected store.
  pub fn save_history(
    &self,
    history: &BuildReportCache,
  ) -> std::io::Result<()> {
    history.save(self.state.build_history())
  }

  /// Decode and reduce exactly one complete structured record.
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
    let event = Event::decode(serde_json::from_slice::<Actions>(json)?);
    let mut output = Vec::new();
    let effects = update::apply_event_at(&mut self.state, event, now);
    if let Some(log) = effects.log
      && (!self.config.silent || log.force)
      && let Some(log) = self.render_log(log)
    {
      output.push(Output::Log(log));
    }
    let mut changed = effects.changed;
    if let Some(resolver) = self.resolver.as_ref() {
      for path in effects.resolve {
        changed |= resolve_derivation_tree(
          &mut self.state,
          resolver.as_ref(),
          &mut self.resolved_drvs,
          path,
        );
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
            styled: "… further build logs suppressed".to_string(),
            plain: "… further build logs suppressed".to_string(),
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

  /// Mark the input source as closed without pretending unfinished work
  /// succeeded.
  pub fn finish(&mut self) {
    self.state.finish();
  }
}

fn resolve_derivation_tree(
  state: &mut State,
  resolver: &dyn DerivationResolver,
  resolved: &mut HashSet<PathBuf>,
  root: PathBuf,
) -> bool {
  let mut pending = vec![root];
  let mut changed = false;
  while let Some(path) = pending.pop() {
    if resolved.contains(&path) {
      continue;
    }
    let Some(path_str) = path.to_str() else {
      continue;
    };
    let Some(derivation) = Derivation::parse(path_str) else {
      continue;
    };
    let id = state.get_or_create_derivation_id(derivation);
    match resolver.resolve(&path) {
      Ok(parsed) => {
        pending.extend(
          parsed
            .input_drvs
            .iter()
            .map(|(dependency, _)| PathBuf::from(dependency)),
        );
        state.populate_parsed_derivation(id, parsed);
        resolved.insert(path);
        changed = true;
      },
      Err(error) => {
        tracing::debug!("could not resolve {}: {error}", path.display())
      },
    }
  }
  changed
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

  #[must_use]
  pub const fn engine(&self) -> &Engine {
    &self.engine
  }

  pub fn engine_mut(&mut self) -> &mut Engine {
    &mut self.engine
  }

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

/// Append-only stream adapter. It writes logs immediately and one final plain
/// presentation; it never emits cursor-control sequences.
pub struct Monitor<W: Write> {
  stream: StreamEngine,
  writer: W,
  render: RenderConfig,
}

impl<W: Write> Monitor<W> {
  #[must_use]
  pub fn new(config: Config, writer: W) -> Self {
    let Config { engine, render } = config;
    Self {
      stream: StreamEngine::new(engine),
      writer,
      render,
    }
  }

  #[must_use]
  pub const fn state(&self) -> &State {
    self.stream.engine().state()
  }

  pub fn process_bytes_at(&mut self, bytes: &[u8], now: f64) -> Result<()> {
    let processed = self.stream.push_at(bytes, now)?;
    write_outputs(&mut self.writer, processed.output, &self.render)
  }

  pub fn engine_mut(&mut self) -> &mut Engine {
    self.stream.engine_mut()
  }

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

pub fn monitor_stream<R: BufRead, W: Write>(
  config: Config,
  reader: R,
  writer: W,
) -> Result<()> {
  Monitor::new(config, writer).process_stream(reader)
}

#[must_use]
pub fn create_monitor<W: Write>(config: Config, writer: W) -> Monitor<W> {
  Monitor::new(config, writer)
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
  fn malformed_claimed_json_is_an_error() {
    let mut stream = StreamEngine::new(EngineConfig::default());
    let error = stream.push_at(b"@nix nope\n", 0.0).unwrap_err();
    assert!(matches!(error, RomError::Json(_)));
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
