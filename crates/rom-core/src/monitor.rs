//! Monitor module for orchestrating state updates and display rendering
use std::{
  io::{BufRead, Write},
  time::Duration,
};

use cognos::Host;
use tracing::debug;

use crate::{
  cache::BuildReportCache,
  display::{Display, DisplayConfig},
  error::{Result, RomError},
  graph::GraphIndexer,
  state::{BuildStatus, Derivation, FailType, State, StorePath},
  types::{Config, InputMode},
  update,
};

const DEPENDENCY_POPULATE_BUDGET_PER_RENDER: usize = 1;

enum HumanParserState {
  Idle,
  PlanBuilds,
  PlanDownloads,
}

struct HumanParser {
  state:   HumanParserState,
  pending: Vec<String>,
}

impl HumanParser {
  fn new() -> Self {
    Self {
      state:   HumanParserState::Idle,
      pending: Vec::new(),
    }
  }
}

/// Processes nix output and displays progress
pub struct Monitor<W: Write> {
  state:        State,
  graph:        GraphIndexer,
  display:      Display<W>,
  config:       Config,
  human_parser: HumanParser,
}

impl<W: Write> Monitor<W> {
  /// Create a new monitor
  pub fn new(config: Config, writer: W) -> Result<Self> {
    let display_config = DisplayConfig {
      show_timers:       config.show_timers,
      max_tree_depth:    10,
      max_visible_lines: 100,
      use_color:         !config.piping,
      format:            config.format,
      legend_style:      config.legend_style,
      summary_style:     config.summary_style,
      icons:             crate::icons::detect(),
    };

    let display = Display::new(writer, display_config)?;
    let mut state = State::new();

    // Load build cache for predictions
    let cache_path = BuildReportCache::default_cache_path();
    let cache = BuildReportCache::new(cache_path);
    state.build_cache = cache.load();

    Ok(Self {
      state,
      graph: GraphIndexer::new(),
      display,
      config,
      human_parser: HumanParser::new(),
    })
  }

  /// Process a stream of input
  pub fn process_stream<R: BufRead>(&mut self, reader: R) -> Result<()> {
    let mut last_render = std::time::Instant::now();
    let render_interval = Duration::from_millis(100);

    // XXX: Poll for local build completions every 200ms. It's specifically this
    // value because NOM 200ms polling to detect builds that finished
    // without explicit stop events. I'll probably redo this.
    let mut last_poll = std::time::Instant::now();
    let poll_interval = Duration::from_millis(200);

    for line in reader.lines() {
      let line = line.map_err(RomError::Io)?;

      // Process the line
      self.process_line(&line)?;

      // Poll for local build completions
      if last_poll.elapsed() >= poll_interval {
        let now = crate::state::current_time();
        crate::update::detect_local_completed_builds(&mut self.state, now);
        if self.graph.populate_pending(
          &mut self.state,
          DEPENDENCY_POPULATE_BUDGET_PER_RENDER,
        ) {
          crate::update::maintain_state(&mut self.state, now);
        }
        last_poll = std::time::Instant::now();
      }

      // Render periodically
      if last_render.elapsed() >= render_interval {
        self.display.render(&self.state, &[])?;
        last_render = std::time::Instant::now();
      }
    }

    // Mark as finished and do final render
    crate::update::finish_state(&mut self.state);

    self.display.render_final(&self.state)?;

    // Save build cache for future predictions
    let cache_path = BuildReportCache::default_cache_path();
    let cache = BuildReportCache::new(cache_path);
    if let Err(e) = cache.save(&self.state.build_cache) {
      debug!("Failed to save build cache: {}", e);
      // Don't fail the build if cache save fails
    }

    // Return error code if there were failures
    if self.state.has_errors() {
      return Err(RomError::BuildFailed);
    }

    Ok(())
  }

  /// Process a single line of input
  fn process_line(&mut self, line: &str) -> Result<bool> {
    // Auto-detect format: lines starting with "@nix " are JSON
    if line.starts_with("@nix ") {
      self.process_json_line(line)
    } else {
      match self.config.input_mode {
        InputMode::Json => self.process_json_line(line),
        InputMode::Human => self.process_human_line(line),
      }
    }
  }

  /// Process a JSON-formatted line
  fn process_json_line(&mut self, line: &str) -> Result<bool> {
    // Nix JSON lines are prefixed with "@nix "
    if let Some(json_str) = line.strip_prefix("@nix ") {
      match serde_json::from_str::<cognos::Actions>(json_str) {
        Ok(action) => {
          // Handle message passthrough by writing through display's writer
          if let cognos::Actions::Message { msg, .. } = &action {
            writeln!(self.display.writer(), "{msg}").map_err(RomError::Io)?;
          }

          let changed =
            update::process_message(&mut self.state, action.clone());
          self.graph.observe_action(&mut self.state, &action);
          if let cognos::Actions::Message { msg, .. } = &action {
            self.graph.observe_plan_line(&mut self.state, msg);
          }
          Ok(changed)
        },
        Err(e) => {
          // Log parsing errors but don't fail
          tracing::debug!("Failed to parse JSON message: {}", e);
          Ok(false)
        },
      }
    } else {
      // Non-JSON lines in JSON mode are passed through
      writeln!(self.display.writer(), "{line}").map_err(RomError::Io)?;
      Ok(false)
    }
  }

  /// Process a human-readable line
  fn process_human_line(&mut self, line: &str) -> Result<bool> {
    // Multi-line state: if we're collecting paths, check for continuation
    match self.human_parser.state {
      HumanParserState::PlanBuilds | HumanParserState::PlanDownloads => {
        if line.starts_with("  /nix/store/")
          || line.starts_with("\t/nix/store/")
        {
          // Accumulate store paths during plan listing
          let path = line.trim().to_string();
          self.human_parser.pending.push(path);
          return Ok(true);
        } else {
          // Flush accumulated paths
          let pending = std::mem::take(&mut self.human_parser.pending);
          let is_builds =
            matches!(self.human_parser.state, HumanParserState::PlanBuilds);
          self.human_parser.state = HumanParserState::Idle;

          for path_str in pending {
            if is_builds {
              if let Some(drv) = crate::state::Derivation::parse(&path_str) {
                self.graph.plan_derivation(&mut self.state, drv);
              }
            } else if let Some(sp) = crate::state::StorePath::parse(&path_str) {
              let sp_id = self.state.get_or_create_store_path_id(sp);
              self.state.full_summary.planned_downloads.insert(sp_id);
            }
          }
          // Fall through to process current line normally
        }
      },
      HumanParserState::Idle => {},
    }

    let trimmed = line.trim();

    if trimmed.is_empty() {
      return Ok(false);
    }

    // Plan detection: "these N derivations will be built:" or "this derivation
    // will be built:"
    if trimmed.ends_with("derivations will be built:")
      || trimmed == "this derivation will be built:"
      || trimmed.ends_with("derivation will be built:")
    {
      self.human_parser.state = HumanParserState::PlanBuilds;
      self.human_parser.pending.clear();
      return Ok(true);
    }

    // Plan detection: "these N paths will be fetched"
    if trimmed.contains("paths will be fetched")
      || trimmed.contains("path will be fetched")
    {
      self.human_parser.state = HumanParserState::PlanDownloads;
      self.human_parser.pending.clear();
      return Ok(true);
    }

    // "building '/nix/store/....drv' on 'ssh://host'..." -> remote build
    if trimmed.starts_with("building '")
      && trimmed.contains(".drv'")
      && trimmed.contains(" on '")
      && let Some(drv_path) = extract_path_from_message(trimmed)
      && let Some(drv) = crate::state::Derivation::parse(&drv_path)
    {
      let host = extract_remote_host(trimmed).unwrap_or(Host::Localhost);
      let drv_id = self.state.get_or_create_derivation_id(drv);
      let now = crate::state::current_time();
      self.state.update_build_status(
        drv_id,
        crate::state::BuildStatus::Building(crate::state::BuildInfo {
          start: now,
          host,
          estimate: None,
          activity_id: None,
        }),
      );
      return Ok(true);
    }

    // "building '/nix/store/....drv'..." -> local build
    if (trimmed.starts_with("building") || trimmed.contains("building '"))
      && let Some(drv_path) = extract_path_from_message(trimmed)
      && let Some(drv) = crate::state::Derivation::parse(&drv_path)
    {
      let drv_id = self.state.get_or_create_derivation_id(drv);
      let now = crate::state::current_time();
      self.state.update_build_status(
        drv_id,
        crate::state::BuildStatus::Building(crate::state::BuildInfo {
          start:       now,
          host:        Host::Localhost,
          estimate:    None,
          activity_id: None,
        }),
      );
      return Ok(true);
    }

    // "copying path '/nix/store/...' from 'ssh://...'..." -> download
    if trimmed.starts_with("copying path '")
      && trimmed.contains("' from '")
      && let Some(path_str) = extract_path_from_message(trimmed)
      && let Some(path) = crate::state::StorePath::parse(&path_str)
    {
      let path_id = self.state.get_or_create_store_path_id(path);
      let now = crate::state::current_time();
      let host =
        extract_remote_host_after(trimmed, "from '").unwrap_or(Host::Localhost);
      self.state.full_summary.running_downloads.insert(
        path_id,
        crate::state::TransferInfo {
          start: now,
          host,
          activity_id: 0,
          bytes_transferred: 0,
          total_bytes: None,
        },
      );
      return Ok(true);
    }

    // "copying path '/nix/store/...' to 'ssh://...'..." -> upload
    if trimmed.starts_with("copying path '")
      && trimmed.contains("' to '")
      && let Some(path_str) = extract_path_from_message(trimmed)
      && let Some(path) = crate::state::StorePath::parse(&path_str)
    {
      let path_id = self.state.get_or_create_store_path_id(path);
      let now = crate::state::current_time();
      let host =
        extract_remote_host_after(trimmed, "to '").unwrap_or(Host::Localhost);
      self.state.full_summary.running_uploads.insert(
        path_id,
        crate::state::TransferInfo {
          start: now,
          host,
          activity_id: 0,
          bytes_transferred: 0,
          total_bytes: None,
        },
      );
      return Ok(true);
    }

    // "builder for '/nix/store/....drv' failed with exit code N"
    if trimmed.starts_with("builder for '")
      && trimmed.contains("failed with exit code")
      && let Some(drv_path) = extract_path_from_message(trimmed)
      && let Some(drv) = crate::state::Derivation::parse(&drv_path)
    {
      let exit_code = extract_exit_code(trimmed);
      let fail_type =
        exit_code.map_or(FailType::Unknown, FailType::BuildFailed);
      let drv_id = self.state.get_or_create_derivation_id(drv);
      let now = crate::state::current_time();
      let build_info = self
        .state
        .get_derivation_info(drv_id)
        .and_then(|info| {
          if let crate::state::BuildStatus::Building(b) = &info.build_status {
            Some(b.clone())
          } else {
            None
          }
        })
        .unwrap_or(crate::state::BuildInfo {
          start:       now,
          host:        Host::Localhost,
          estimate:    None,
          activity_id: None,
        });
      self.state.update_build_status(
        drv_id,
        crate::state::BuildStatus::Failed {
          info: build_info,
          fail: crate::state::BuildFail { at: now, fail_type },
        },
      );
      return Ok(true);
    }

    // "error: hash mismatch" lines
    if trimmed.starts_with("error:") && trimmed.contains("hash mismatch") {
      self.state.nix_errors.push(trimmed.to_string());
      return Ok(true);
    }

    // General error lines
    if trimmed.starts_with("error:") || trimmed.contains("error:") {
      self.state.nix_errors.push(trimmed.to_string());

      if let Some(drv_path) = extract_path_from_message(trimmed)
        && let Some(drv) = crate::state::Derivation::parse(&drv_path)
        && let Some(&drv_id) = self.state.derivation_ids.get(&drv)
        && let Some(info) = self.state.get_derivation_info(drv_id)
        && let crate::state::BuildStatus::Building(build_info) =
          &info.build_status
      {
        let now = crate::state::current_time();
        self.state.update_build_status(
          drv_id,
          crate::state::BuildStatus::Failed {
            info: build_info.clone(),
            fail: crate::state::BuildFail {
              at:        now,
              fail_type: FailType::Unknown,
            },
          },
        );
      }

      return Ok(true);
    }

    // "checking outputs of '/nix/store/....drv'..."
    if trimmed.contains("checking outputs of")
      && let Some(drv_path) = extract_path_from_message(trimmed)
      && let Some(drv) = crate::state::Derivation::parse(&drv_path)
    {
      let drv_id = self.state.get_or_create_derivation_id(drv);
      self.state.touched_ids.insert(drv_id);
      return Ok(true);
    }

    // Detect downloads (old-style)
    if (trimmed.starts_with("downloading") || trimmed.contains("downloading '"))
      && let Some(path_str) = extract_path_from_message(trimmed)
      && let Some(path) = crate::state::StorePath::parse(&path_str)
    {
      let path_id = self.state.get_or_create_store_path_id(path);
      let now = crate::state::current_time();
      let total_bytes = extract_byte_size(trimmed);
      self.state.full_summary.running_downloads.insert(
        path_id,
        crate::state::TransferInfo {
          start: now,
          host: Host::Localhost,
          activity_id: 0,
          bytes_transferred: 0,
          total_bytes,
        },
      );
      return Ok(true);
    }

    // Detect download completions
    if (trimmed.starts_with("downloaded") || trimmed.contains("downloaded '"))
      && let Some(path_str) = extract_path_from_message(trimmed)
      && let Some(path) = StorePath::parse(&path_str)
      && let Some(&path_id) = self.state.store_path_ids.get(&path)
    {
      let now = crate::state::current_time();
      let total_bytes = extract_byte_size(trimmed).unwrap_or(0);
      let start = self
        .state
        .full_summary
        .running_downloads
        .get(&path_id)
        .map_or(now, |t| t.start);
      let completed = crate::state::CompletedTransferInfo {
        start,
        end: now,
        host: Host::Localhost,
        total_bytes,
      };
      self.state.full_summary.running_downloads.remove(&path_id);
      self
        .state
        .full_summary
        .completed_downloads
        .insert(path_id, completed);
      return Ok(true);
    }

    // Detect build completions (old-style)
    if (trimmed.starts_with("built") || trimmed.contains("built '"))
      && let Some(drv_path) = extract_path_from_message(trimmed)
      && let Some(drv) = Derivation::parse(&drv_path)
      && let Some(&drv_id) = self.state.derivation_ids.get(&drv)
      && let Some(info) = self.state.get_derivation_info(drv_id)
      && let BuildStatus::Building(build_info) = &info.build_status
    {
      let now = crate::state::current_time();
      self.state.update_build_status(drv_id, BuildStatus::Built {
        info: build_info.clone(),
        end:  now,
      });
      return Ok(true);
    }

    // Unrecognized lines go through display's writer
    writeln!(self.display.writer(), "{line}").map_err(RomError::Io)?;
    Ok(false)
  }

  /// Get a reference to the current state
  pub const fn state(&self) -> &State {
    &self.state
  }

  /// Get a mutable reference to the current state
  pub const fn state_mut(&mut self) -> &mut State {
    &mut self.state
  }
}

/// Extract a remote host from "... on 'ssh://host'..." pattern
fn extract_remote_host(line: &str) -> Option<Host> {
  extract_remote_host_after(line, "on '")
}

/// Extract a remote host from a pattern like "from 'ssh://host'" or "to
/// 'ssh://host'"
fn extract_remote_host_after(line: &str, marker: &str) -> Option<Host> {
  let pos = line.find(marker)?;
  let after = &line[pos + marker.len()..];
  let end = after.find('\'')?;
  let raw = &after[..end];
  let name = raw
    .strip_prefix("ssh://")
    .or_else(|| raw.strip_prefix("https://"))
    .or_else(|| raw.strip_prefix("http://"))
    .unwrap_or(raw)
    .trim_end_matches('/');
  if name.is_empty() || name == "localhost" {
    Some(Host::Localhost)
  } else {
    Some(Host::Remote(name.to_string()))
  }
}

fn extract_exit_code(line: &str) -> Option<i32> {
  let pos = line.find("exit code")?;
  let after = &line[pos + "exit code".len()..];
  let trimmed = after.trim_start();
  let code_str = trimmed.split(|c: char| !c.is_ascii_digit()).next()?;
  code_str.parse().ok()
}

/// Extract a path from a message line
pub fn extract_path_from_message(line: &str) -> Option<String> {
  // Look for quoted paths
  if let Some(start) = line.find('\'')
    && let Some(end) = line[start + 1..].find('\'')
  {
    return Some(line[start + 1..start + 1 + end].to_string());
  }

  // Look for unquoted store paths
  for word in line.split_whitespace() {
    if word.starts_with("/nix/store/") {
      return Some(
        word
          .trim_matches(|c: char| {
            !c.is_ascii_alphanumeric() && c != '/' && c != '-' && c != '.'
          })
          .to_string(),
      );
    }
  }

  None
}

/// Extract byte size from a message line (e.g., "downloaded 123 KiB")
pub fn extract_byte_size(line: &str) -> Option<u64> {
  // Look for patterns like "123 KiB", "6.7 MiB", etc.
  // Haha 6.7
  let words: Vec<&str> = line.split_whitespace().collect();
  for (i, word) in words.iter().enumerate() {
    if i + 1 < words.len() {
      let unit = words[i + 1];
      if matches!(unit, "B" | "KiB" | "MiB" | "GiB" | "TiB" | "PiB")
        && let Ok(value) = word.parse::<f64>()
      {
        let multiplier = match unit {
          "B" => 1_u64,
          "KiB" => 1024,
          "MiB" => 1024 * 1024,
          "GiB" => 1024 * 1024 * 1024,
          "TiB" => 1024_u64 * 1024 * 1024 * 1024,
          "PiB" => 1024_u64 * 1024 * 1024 * 1024 * 1024,
          _ => 1,
        };
        return Some((value * multiplier as f64) as u64);
      }
    }
  }
  None
}
