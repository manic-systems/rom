//! Incremental dependency graph indexing.

use std::{
  collections::{HashSet, VecDeque},
  sync::mpsc,
  thread,
};

use cognos::{Actions, Activities, OutputName, ParsedDerivation};
use tracing::debug;

use crate::state::{
  Derivation,
  DerivationId,
  InputDerivation,
  State,
  StorePath,
};

const INPUT_DERIVATIONS_APPLY_BUDGET: usize = 64;

pub struct GraphIndexer {
  pending_dependency_populates: VecDeque<DerivationId>,
  pending_dependency_set:       HashSet<DerivationId>,
  submitted_dependency_set:     HashSet<DerivationId>,
  pending_parsed_applies:       VecDeque<PendingParsedApply>,
  request_tx:                   mpsc::Sender<ParseRequest>,
  result_rx:                    mpsc::Receiver<ParseResult>,
}

struct ParseRequest {
  drv_id:   DerivationId,
  drv_path: String,
}

struct ParseResult {
  drv_id:   DerivationId,
  drv_path: String,
  parsed:   Result<ParsedGraph, String>,
}

struct ParsedGraph {
  pname:      Option<String>,
  platform:   String,
  outputs:    Vec<(String, String)>,
  input_drvs: VecDeque<(String, Vec<String>)>,
}

struct PendingParsedApply {
  drv_id:           DerivationId,
  drv_path:         String,
  parsed:           ParsedGraph,
  metadata_applied: bool,
}

impl From<ParsedDerivation> for ParsedGraph {
  fn from(parsed: ParsedDerivation) -> Self {
    Self {
      pname:      cognos::extract_pname(&parsed.env),
      platform:   parsed.platform,
      outputs:    parsed.outputs,
      input_drvs: parsed.input_drvs.into(),
    }
  }
}

impl PendingParsedApply {
  fn new(drv_id: DerivationId, drv_path: String, parsed: ParsedGraph) -> Self {
    Self {
      drv_id,
      drv_path,
      parsed,
      metadata_applied: false,
    }
  }
}

impl Default for GraphIndexer {
  fn default() -> Self {
    Self::new()
  }
}

impl GraphIndexer {
  #[must_use]
  pub fn new() -> Self {
    let (request_tx, request_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();

    thread::Builder::new()
      .name("rom-graph-indexer".to_string())
      .spawn(move || parse_worker(request_rx, result_tx))
      .expect("failed to spawn graph indexer");

    Self {
      pending_dependency_populates: VecDeque::new(),
      pending_dependency_set: HashSet::new(),
      submitted_dependency_set: HashSet::new(),
      pending_parsed_applies: VecDeque::new(),
      request_tx,
      result_rx,
    }
  }

  pub fn plan_derivation(
    &mut self,
    state: &mut State,
    drv: Derivation,
  ) -> DerivationId {
    let drv_id = state.plan_derivation(drv);
    self.queue_derivation(state, drv_id);
    drv_id
  }

  pub fn observe_action(
    &mut self,
    state: &mut State,
    action: &Actions,
  ) -> bool {
    let Actions::Start {
      text,
      activity,
      fields,
      ..
    } = action
    else {
      return false;
    };
    if *activity != Activities::Build {
      return false;
    }

    let Some(drv) = derivation_from_start(text, fields) else {
      return false;
    };
    let drv_id = state.get_or_create_derivation_id(drv);
    self.queue_derivation(state, drv_id);
    true
  }

  pub fn observe_plan_line(&mut self, state: &mut State, msg: &str) -> bool {
    if !(msg.starts_with("  /nix/store/") || msg.starts_with('\t')) {
      return false;
    }

    let path = msg.trim();
    let Some(drv) = Derivation::parse(path) else {
      return false;
    };

    self.plan_derivation(state, drv);
    true
  }

  pub fn populate_pending(&mut self, state: &mut State, budget: usize) -> bool {
    let changed = self.apply_ready_results(state, budget);
    self.submit_pending_requests(state, budget);
    changed
  }

  fn apply_ready_results(&mut self, state: &mut State, budget: usize) -> bool {
    let mut changed = false;
    for _ in 0..budget {
      if let Some(applied) = self.apply_next_pending_parse(state) {
        changed |= applied;
        continue;
      }

      let result = match self.result_rx.try_recv() {
        Ok(result) => result,
        Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => {
          break;
        },
      };
      self.enqueue_parse_result(result);
    }
    changed
  }

  fn submit_pending_requests(&mut self, state: &State, budget: usize) {
    for _ in 0..budget {
      let Some(drv_id) = self.pending_dependency_populates.pop_front() else {
        break;
      };
      self.pending_dependency_set.remove(&drv_id);
      if state.dependencies_populated(drv_id)
        || self.submitted_dependency_set.contains(&drv_id)
      {
        continue;
      }

      let Some(info) = state.get_derivation_info(drv_id) else {
        continue;
      };
      let drv_path = info.name.path.display().to_string();
      self.submitted_dependency_set.insert(drv_id);
      if self
        .request_tx
        .send(ParseRequest { drv_id, drv_path })
        .is_err()
      {
        debug!("graph indexer worker is unavailable");
        break;
      }
    }
  }

  fn enqueue_parse_result(&mut self, result: ParseResult) {
    let ParseResult {
      drv_id,
      drv_path,
      parsed,
    } = result;

    let parsed = match parsed {
      Ok(parsed) => {
        debug!(
          "Successfully parsed .drv file: {} with {} input derivations",
          drv_path,
          parsed.input_drvs.len()
        );
        parsed
      },
      Err(err) => {
        debug!("Failed to parse .drv file {}: {}", drv_path, err);
        return;
      },
    };

    self
      .pending_parsed_applies
      .push_back(PendingParsedApply::new(drv_id, drv_path, parsed));
  }

  fn apply_next_pending_parse(&mut self, state: &mut State) -> Option<bool> {
    let mut pending = self.pending_parsed_applies.pop_front()?;
    if state.dependencies_populated(pending.drv_id) {
      return Some(false);
    }

    let ApplyChunk {
      changed,
      completed,
      input_ids,
    } = apply_parsed_derivation_chunk(state, &mut pending);
    for input_id in input_ids {
      self.queue_derivation(state, input_id);
    }

    if completed {
      debug!(
        "Finished applying .drv dependency graph for {}",
        pending.drv_path
      );
    } else {
      self.pending_parsed_applies.push_back(pending);
    }

    Some(changed)
  }

  fn queue_derivation(&mut self, state: &State, drv_id: DerivationId) {
    if state.dependencies_populated(drv_id)
      || self.submitted_dependency_set.contains(&drv_id)
    {
      return;
    }
    if self.pending_dependency_set.insert(drv_id) {
      self.pending_dependency_populates.push_back(drv_id);
    }
  }
}

fn parse_worker(
  request_rx: mpsc::Receiver<ParseRequest>,
  result_tx: mpsc::Sender<ParseResult>,
) {
  for request in request_rx {
    let parsed =
      cognos::parse_drv_file(&request.drv_path).map(ParsedGraph::from);
    if result_tx
      .send(ParseResult {
        drv_id: request.drv_id,
        drv_path: request.drv_path,
        parsed,
      })
      .is_err()
    {
      break;
    }
  }
}

fn derivation_from_start(
  text: &str,
  fields: &[serde_json::Value],
) -> Option<Derivation> {
  fields
    .first()
    .and_then(|value| value.as_str())
    .and_then(Derivation::parse)
    .or_else(|| {
      text
        .split_whitespace()
        .map(|part| {
          part.trim_matches(|ch| ch == '\'' || ch == '"' || ch == ',')
        })
        .find_map(Derivation::parse)
    })
}

fn apply_parsed_derivation(
  state: &mut State,
  drv_id: DerivationId,
  parsed: ParsedDerivation,
) {
  let mut parsed = ParsedGraph::from(parsed);
  apply_derivation_metadata(state, drv_id, &parsed);
  while let Some((input_drv_path, outputs)) = parsed.input_drvs.pop_front() {
    apply_input_derivation(state, drv_id, input_drv_path, outputs);
  }

  state.mark_dependencies_populated(drv_id);
  state.recompute_derivation_summary(drv_id);
  state.propagate_to_parents(drv_id);
  state.touched_ids.insert(drv_id);
}

struct ApplyChunk {
  changed:   bool,
  completed: bool,
  input_ids: Vec<DerivationId>,
}

fn apply_parsed_derivation_chunk(
  state: &mut State,
  pending: &mut PendingParsedApply,
) -> ApplyChunk {
  let mut changed = false;
  let mut input_ids = Vec::new();

  if !pending.metadata_applied {
    apply_derivation_metadata(state, pending.drv_id, &pending.parsed);
    pending.metadata_applied = true;
    changed = true;
  }

  for _ in 0..INPUT_DERIVATIONS_APPLY_BUDGET {
    let Some((input_drv_path, outputs)) = pending.parsed.input_drvs.pop_front()
    else {
      break;
    };
    if let Some(input_id) =
      apply_input_derivation(state, pending.drv_id, input_drv_path, outputs)
    {
      input_ids.push(input_id);
      changed = true;
    }
  }

  let completed = pending.parsed.input_drvs.is_empty();
  if completed {
    state.mark_dependencies_populated(pending.drv_id);
    state.recompute_derivation_summary(pending.drv_id);
    state.propagate_to_parents(pending.drv_id);
    state.touched_ids.insert(pending.drv_id);
    changed = true;
  }

  ApplyChunk {
    changed,
    completed,
    input_ids,
  }
}

fn apply_derivation_metadata(
  state: &mut State,
  drv_id: DerivationId,
  parsed: &ParsedGraph,
) {
  if let Some(pname) = parsed.pname.clone()
    && let Some(info) = state.get_derivation_info_mut(drv_id)
  {
    info.pname = Some(pname);
  }

  if let Some(info) = state.get_derivation_info_mut(drv_id) {
    info.platform = Some(parsed.platform.clone());
  }

  for (output_name, store_path_str) in &parsed.outputs {
    if let Some(sp) = StorePath::parse(store_path_str) {
      let sp_id = state.get_or_create_store_path_id(sp);
      if let Some(sp_info) = state.get_store_path_info_mut(sp_id) {
        sp_info.producer = Some(drv_id);
      }
      if let Some(drv_info) = state.get_derivation_info_mut(drv_id) {
        drv_info
          .outputs
          .insert(OutputName::parse(output_name), sp_id);
      }
    }
  }
}

fn apply_input_derivation(
  state: &mut State,
  drv_id: DerivationId,
  input_drv_path: String,
  outputs: Vec<String>,
) -> Option<DerivationId> {
  let input_drv = Derivation::parse(&input_drv_path)?;
  let input_drv_id = state.get_or_create_derivation_id(input_drv);
  let outputs = outputs
    .into_iter()
    .map(|output| OutputName::parse(&output))
    .collect::<HashSet<_>>();

  if let Some(parent_info) = state.get_derivation_info_mut(drv_id) {
    let input = InputDerivation {
      derivation: input_drv_id,
      outputs,
    };
    if !parent_info
      .input_derivations
      .iter()
      .any(|d| d.derivation == input_drv_id)
    {
      parent_info.input_derivations.push(input);
    }
  }

  if let Some(child_info) = state.get_derivation_info_mut(input_drv_id) {
    child_info.derivation_parents.insert(drv_id);
  }

  state.forest_roots.retain(|&id| id != input_drv_id);
  Some(input_drv_id)
}

/// Populate derivation dependencies by parsing its .drv file.
///
/// This is retained for tests and non-interactive callers. The live monitor
/// uses `GraphIndexer`, which parses in a background worker.
pub fn populate_derivation_dependencies(
  state: &mut State,
  drv_id: DerivationId,
) {
  if state.dependencies_populated(drv_id) {
    debug!("Skipping already-parsed derivation {}", drv_id);
    return;
  }

  let drv_path = {
    let info = match state.get_derivation_info(drv_id) {
      Some(i) => i,
      None => return,
    };
    info.name.path.display().to_string()
  };

  debug!("Attempting to parse .drv file: {}", drv_path);
  match cognos::parse_drv_file(&drv_path) {
    Ok(parsed) => apply_parsed_derivation(state, drv_id, parsed),
    Err(err) => debug!("Failed to parse .drv file {}: {}", drv_path, err),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn pending_dependency_queue_deduplicates_with_set_lookup() {
    let mut state = State::new();
    let drv_id = state.get_or_create_derivation_id(
      Derivation::parse(
        "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello.drv",
      )
      .unwrap(),
    );
    let mut graph = GraphIndexer::new();

    graph.queue_derivation(&state, drv_id);
    graph.queue_derivation(&state, drv_id);

    assert_eq!(graph.pending_dependency_populates.len(), 1);
    assert_eq!(graph.pending_dependency_set.len(), 1);
  }
}
