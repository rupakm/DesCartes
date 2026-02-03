use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use descartes_core::{
    async_runtime::{ReadyTaskPolicy, ReadyTaskSignature, TaskId},
    EventFrontierPolicy, FrontierEvent, FrontierEventKind, FrontierSignature, SimTime,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::schedule_explore::{DecisionKey, DecisionKind, DecisionScript};
use crate::trace::{
    AsyncRuntimeDecision, FrontierEntry, FrontierKind, SchedulerDecision, TraceEvent, TraceRecorder,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionDistribution {
    /// Always pick the given stable ID.
    Deterministic { chosen_id: u64 },

    /// Randomized categorical distribution over stable IDs.
    ///
    /// v1 rollout search can start with deterministic-only policies.
    #[allow(dead_code)]
    Categorical { ids: Vec<u64>, weights: Vec<f64> },
}

/// A table-based scheduler policy mapping decision keys to action distributions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TablePolicy {
    table: HashMap<DecisionKey, ActionDistribution>,
}

impl TablePolicy {
    pub fn new() -> Self {
        Self {
            table: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    pub fn get(&self, key: &DecisionKey) -> Option<&ActionDistribution> {
        let mut normalized = key.clone();
        normalized.normalize();
        self.table.get(&normalized)
    }

    pub fn set_deterministic(&mut self, key: DecisionKey, chosen_id: u64) {
        self.table.insert(
            key.normalized(),
            ActionDistribution::Deterministic { chosen_id },
        );
    }

    pub fn remove(&mut self, key: &DecisionKey) {
        let mut normalized = key.clone();
        normalized.normalize();
        self.table.remove(&normalized);
    }

    pub fn keys(&self) -> impl Iterator<Item = &DecisionKey> {
        self.table.keys()
    }

    /// Convert deterministic entries into a [`DecisionScript`].
    ///
    /// Categorical entries are ignored in v1.
    pub fn to_decision_script(&self) -> DecisionScript {
        let mut script = DecisionScript::new();
        for (key, dist) in &self.table {
            if let ActionDistribution::Deterministic { chosen_id } = dist {
                script.insert(key.clone(), *chosen_id);
            }
        }
        script
    }

    /// Policy hook used by frontier wrappers.
    ///
    /// Returns the chosen frontier `seq` (stable ID) if this decision is in the table.
    pub fn choose_frontier(
        &self,
        time_nanos: u64,
        ordinal: u64,
        frontier_seqs: &[u64],
        rng: &mut impl Rng,
    ) -> Option<u64> {
        let key = DecisionKey::new(
            DecisionKind::Frontier,
            time_nanos,
            ordinal,
            frontier_seqs.to_vec(),
        );
        self.choose_from_key(&key, rng)
    }

    /// Policy hook used by tokio-ready wrappers.
    ///
    /// Returns the chosen tokio task ID (stable ID) if this decision is in the table.
    pub fn choose_ready(
        &self,
        time_nanos: u64,
        ordinal: u64,
        ready_task_ids: &[u64],
        rng: &mut impl Rng,
    ) -> Option<u64> {
        let key = DecisionKey::new(
            DecisionKind::TokioReady,
            time_nanos,
            ordinal,
            ready_task_ids.to_vec(),
        );
        self.choose_from_key(&key, rng)
    }

    fn choose_from_key(&self, key: &DecisionKey, rng: &mut impl Rng) -> Option<u64> {
        match self.table.get(key)? {
            ActionDistribution::Deterministic { chosen_id } => Some(*chosen_id),
            ActionDistribution::Categorical { ids, weights } => {
                if ids.is_empty() || ids.len() != weights.len() {
                    return None;
                }

                let mut total = 0.0;
                for &w in weights {
                    if w.is_finite() && w > 0.0 {
                        total += w;
                    }
                }
                if total <= 0.0 {
                    return None;
                }

                let mut draw = rng.gen::<f64>() * total;
                for (id, w) in ids.iter().copied().zip(weights.iter().copied()) {
                    if !(w.is_finite() && w > 0.0) {
                        continue;
                    }
                    if draw <= w {
                        return Some(id);
                    }
                    draw -= w;
                }
                ids.last().copied()
            }
        }
    }
}

fn kind_to_trace(kind: FrontierEventKind) -> FrontierKind {
    match kind {
        FrontierEventKind::Component => FrontierKind::Component,
        FrontierEventKind::Task => FrontierKind::Task,
    }
}

fn to_frontier_entry(event: &FrontierEvent) -> FrontierEntry {
    FrontierEntry {
        seq: event.seq,
        kind: kind_to_trace(event.kind),
        component_id: event.component_id.map(|id| id.to_string()),
        task_id: event.task_id.map(|id| id.to_string()),
    }
}

/// Wraps a fallback frontier policy and applies overrides from a [`TablePolicy`].
///
/// Always records observed frontier decisions into the trace.
pub struct TableFrontierPolicy {
    policy: TablePolicy,
    fallback: Box<dyn EventFrontierPolicy>,
    recorder: Arc<Mutex<TraceRecorder>>,
    rng: StdRng,
    next_ordinal: u64,
}

impl TableFrontierPolicy {
    pub fn new(
        policy: TablePolicy,
        fallback: Box<dyn EventFrontierPolicy>,
        recorder: Arc<Mutex<TraceRecorder>>,
        seed: u64,
    ) -> Self {
        Self {
            policy,
            fallback,
            recorder,
            rng: StdRng::seed_from_u64(seed),
            next_ordinal: 0,
        }
    }
}

impl EventFrontierPolicy for TableFrontierPolicy {
    fn choose(&mut self, time: SimTime, frontier: &[FrontierEvent]) -> usize {
        let FrontierSignature {
            time_nanos,
            frontier_seqs,
        } = FrontierSignature::new(time, frontier);

        let ordinal = self.next_ordinal;
        self.next_ordinal += 1;

        let chosen_seq =
            self.policy
                .choose_frontier(time_nanos, ordinal, &frontier_seqs, &mut self.rng);

        let chosen_index = if let Some(chosen_seq) = chosen_seq {
            frontier_seqs
                .iter()
                .position(|&s| s == chosen_seq)
                .unwrap_or_else(|| self.fallback.choose(time, frontier))
        } else {
            self.fallback.choose(time, frontier)
        }
        .min(frontier.len().saturating_sub(1));

        let chosen_seq = frontier.get(chosen_index).map(|e| e.seq);
        let frontier_meta: Vec<FrontierEntry> = frontier.iter().map(to_frontier_entry).collect();

        self.recorder
            .lock()
            .unwrap()
            .record(TraceEvent::SchedulerDecision(SchedulerDecision {
                time_nanos,
                chosen_index,
                chosen_seq,
                frontier_seqs,
                frontier: frontier_meta,
            }));

        chosen_index
    }
}

/// Wraps a fallback ready-task policy and applies overrides from a [`TablePolicy`].
///
/// Always records observed tokio-ready decisions into the trace.
pub struct TableReadyTaskPolicy {
    policy: TablePolicy,
    fallback: Box<dyn ReadyTaskPolicy>,
    recorder: Arc<Mutex<TraceRecorder>>,
    rng: StdRng,
    next_ordinal: u64,
}

impl TableReadyTaskPolicy {
    pub fn new(
        policy: TablePolicy,
        fallback: Box<dyn ReadyTaskPolicy>,
        recorder: Arc<Mutex<TraceRecorder>>,
        seed: u64,
    ) -> Self {
        Self {
            policy,
            fallback,
            recorder,
            rng: StdRng::seed_from_u64(seed),
            next_ordinal: 0,
        }
    }
}

impl ReadyTaskPolicy for TableReadyTaskPolicy {
    fn choose(&mut self, time: SimTime, ready: &[TaskId]) -> usize {
        let signature = ReadyTaskSignature::new(time, ready);
        let ordinal = self.next_ordinal;
        self.next_ordinal += 1;

        let chosen_task_id = self.policy.choose_ready(
            signature.time_nanos,
            ordinal,
            &signature.ready_task_ids,
            &mut self.rng,
        );

        let chosen_index = if let Some(chosen_id) = chosen_task_id {
            signature
                .ready_task_ids
                .iter()
                .position(|&id| id == chosen_id)
                .unwrap_or_else(|| self.fallback.choose(time, ready))
        } else {
            self.fallback.choose(time, ready)
        }
        .min(ready.len().saturating_sub(1));

        let chosen_task_id = ready.get(chosen_index).map(|t| t.0);

        self.recorder
            .lock()
            .unwrap()
            .record(TraceEvent::AsyncRuntimeDecision(AsyncRuntimeDecision {
                time_nanos: signature.time_nanos,
                chosen_index,
                chosen_task_id,
                ready_task_ids: signature.ready_task_ids,
            }));

        chosen_index
    }
}
