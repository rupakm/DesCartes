//! Normalized scheduling decisions for exploration tooling.
//!
//! This module defines a stable representation of *scheduling decision points*
//! (frontier/tokio-ready in v1) so higher-level algorithms can work purely from
//! traces and scripted overrides.
//!
//! A typical loop is:
//! - record a baseline run (see `descartes_explore::harness`)
//! - call [`extract_decisions`] to obtain [`ObservedDecision`] values
//! - build a [`DecisionScript`] from those decisions
//! - replay a run while forcing some decisions via `run_controlled`

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::trace::{AsyncRuntimeDecision, SchedulerDecision, Trace, TraceEvent};

/// Kind of nondeterministic scheduling decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DecisionKind {
    /// Frontier choice among same-time scheduler entries.
    Frontier,

    /// Tokio-level choice among ready async tasks.
    TokioReady,
}

/// Key used to identify a scheduling decision for scripting/replay.
///
/// `choice_set` stores stable identifiers (frontier seqs or ready task ids).
///
/// Note: for stable hashing/equality, `choice_set` is expected to be normalized
/// (sorted, deduplicated). Use [`DecisionKey::new`] / [`DecisionKey::normalize`]
/// when constructing keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DecisionKey {
    pub kind: DecisionKind,
    pub time_nanos: u64,

    /// Monotonic ordinal for this decision kind within a rollout.
    ///
    /// This avoids collisions when the same `(kind, time_nanos, choice_set)` occurs
    /// multiple times in a single run (common for tokio-ready decisions).
    pub ordinal: u64,

    pub choice_set: Vec<u64>,
}

impl DecisionKey {
    /// Construct a normalized decision key.
    pub fn new(kind: DecisionKind, time_nanos: u64, ordinal: u64, choice_set: Vec<u64>) -> Self {
        let mut key = Self {
            kind,
            time_nanos,
            ordinal,
            choice_set,
        };
        key.normalize();
        key
    }

    /// Sort/deduplicate `choice_set` in-place.
    pub fn normalize(&mut self) {
        self.choice_set.sort_unstable();
        self.choice_set.dedup();
    }

    /// Return a normalized copy of this key.
    pub fn normalized(mut self) -> Self {
        self.normalize();
        self
    }
}

/// Strongly-typed choice values, if you want to keep the origin kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DecisionChoice {
    ChosenSeq(u64),
    ChosenTask(u64),
}

impl DecisionChoice {
    pub fn id(self) -> u64 {
        match self {
            DecisionChoice::ChosenSeq(id) => id,
            DecisionChoice::ChosenTask(id) => id,
        }
    }
}

impl From<DecisionChoice> for u64 {
    fn from(value: DecisionChoice) -> Self {
        value.id()
    }
}

/// A concrete decision observed in a trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedDecision {
    pub key: DecisionKey,

    /// Chosen stable identifier (`frontier seq` or `task id`).
    pub chosen: u64,
}

/// Scripted choices for a run.
#[derive(Debug, Clone, Default)]
pub struct DecisionScript {
    decisions: HashMap<DecisionKey, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionKeyMismatch {
    pub kind: DecisionKind,
    pub time_nanos: u64,
    pub ordinal: u64,
    pub expected_choice_set: Vec<u64>,
    pub observed_choice_set: Vec<u64>,
}

impl DecisionScript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_observed<I>(decisions: I) -> Self
    where
        I: IntoIterator<Item = ObservedDecision>,
    {
        let mut script = Self::new();
        for d in decisions {
            script.insert(d.key, d.chosen);
        }
        script
    }

    pub fn insert(&mut self, key: DecisionKey, chosen: u64) -> Option<u64> {
        self.decisions.insert(key.normalized(), chosen)
    }

    pub fn get(&self, key: &DecisionKey) -> Option<u64> {
        let mut normalized = key.clone();
        normalized.normalize();
        self.decisions.get(&normalized).copied()
    }

    /// Match a decision key and provide a mismatch if the (kind,time) pair exists.
    ///
    /// This is useful when keys are stable but the observed choice set differs.
    pub fn match_decision(&self, key: &DecisionKey) -> Result<Option<u64>, DecisionKeyMismatch> {
        let mut normalized = key.clone();
        normalized.normalize();

        if let Some(chosen) = self.decisions.get(&normalized).copied() {
            return Ok(Some(chosen));
        }

        let expected = self.decisions.keys().find(|k| {
            k.kind == normalized.kind
                && k.time_nanos == normalized.time_nanos
                && k.ordinal == normalized.ordinal
        });

        let Some(expected) = expected else {
            return Ok(None);
        };

        Err(DecisionKeyMismatch {
            kind: normalized.kind,
            time_nanos: normalized.time_nanos,
            ordinal: normalized.ordinal,
            expected_choice_set: expected.choice_set.clone(),
            observed_choice_set: normalized.choice_set,
        })
    }

    pub fn len(&self) -> usize {
        self.decisions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.decisions.is_empty()
    }
}

fn choice_from_index(choice_set: &[u64], chosen_index: usize, context: &str) -> u64 {
    *choice_set.get(chosen_index).unwrap_or_else(|| {
        panic!(
            "{context}: chosen_index {chosen_index} out of bounds (len={})",
            choice_set.len()
        )
    })
}

fn scheduler_choice_set(decision: &SchedulerDecision) -> Vec<u64> {
    if !decision.frontier_seqs.is_empty() {
        return decision.frontier_seqs.clone();
    }

    if !decision.frontier.is_empty() {
        return decision.frontier.iter().map(|e| e.seq).collect();
    }

    Vec::new()
}

fn ready_task_choice_set(decision: &AsyncRuntimeDecision) -> Vec<u64> {
    decision.ready_task_ids.clone()
}

/// Extract v1 scheduling decisions from a trace.
///
/// Produces a normalized [`DecisionKey`] for each observed decision.
pub fn extract_decisions(trace: &Trace) -> Vec<ObservedDecision> {
    let mut out = Vec::new();

    let mut frontier_ordinal = 0u64;
    let mut tokio_ready_ordinal = 0u64;

    for event in &trace.events {
        match event {
            TraceEvent::SchedulerDecision(d) => {
                let choice_set = scheduler_choice_set(d);
                let chosen = d.chosen_seq.unwrap_or_else(|| {
                    choice_from_index(&choice_set, d.chosen_index, "SchedulerDecision")
                });

                let ordinal = frontier_ordinal;
                frontier_ordinal += 1;

                out.push(ObservedDecision {
                    key: DecisionKey::new(
                        DecisionKind::Frontier,
                        d.time_nanos,
                        ordinal,
                        choice_set,
                    ),
                    chosen,
                });
            }
            TraceEvent::AsyncRuntimeDecision(d) => {
                let choice_set = ready_task_choice_set(d);
                let chosen = d.chosen_task_id.unwrap_or_else(|| {
                    choice_from_index(&choice_set, d.chosen_index, "AsyncRuntimeDecision")
                });

                let ordinal = tokio_ready_ordinal;
                tokio_ready_ordinal += 1;

                out.push(ObservedDecision {
                    key: DecisionKey::new(
                        DecisionKind::TokioReady,
                        d.time_nanos,
                        ordinal,
                        choice_set,
                    ),
                    chosen,
                });
            }
            _ => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::trace::{FrontierEntry, FrontierKind, TraceMeta};

    #[test]
    fn decision_key_normalization_sorts_and_dedups() {
        let a = DecisionKey::new(DecisionKind::Frontier, 10, 0, vec![3, 1, 2, 2]);
        let b = DecisionKey::new(DecisionKind::Frontier, 10, 0, vec![2, 3, 1]);

        assert_eq!(a, b);
        assert_eq!(a.choice_set, vec![1, 2, 3]);

        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
    }

    #[test]
    fn extract_decisions_prefers_stable_ids() {
        let trace = Trace {
            meta: TraceMeta {
                seed: 1,
                scenario: "x".to_string(),
            },
            events: vec![
                TraceEvent::SchedulerDecision(SchedulerDecision {
                    time_nanos: 10,
                    chosen_index: 0,
                    chosen_seq: Some(777),
                    frontier_seqs: vec![5, 777, 9],
                    frontier: vec![],
                }),
                TraceEvent::AsyncRuntimeDecision(AsyncRuntimeDecision {
                    time_nanos: 11,
                    chosen_index: 0,
                    chosen_task_id: Some(2),
                    ready_task_ids: vec![1, 2, 3],
                }),
            ],
        };

        let decisions = extract_decisions(&trace);
        assert_eq!(decisions.len(), 2);

        assert_eq!(decisions[0].chosen, 777);
        assert_eq!(decisions[0].key.kind, DecisionKind::Frontier);
        assert_eq!(decisions[0].key.time_nanos, 10);
        assert_eq!(decisions[0].key.ordinal, 0);
        assert_eq!(decisions[0].key.choice_set, vec![5, 9, 777]);

        assert_eq!(decisions[1].chosen, 2);
        assert_eq!(decisions[1].key.kind, DecisionKind::TokioReady);
        assert_eq!(decisions[1].key.time_nanos, 11);
        assert_eq!(decisions[1].key.ordinal, 0);
        assert_eq!(decisions[1].key.choice_set, vec![1, 2, 3]);
    }

    #[test]
    fn extract_decisions_falls_back_to_index_and_frontier_metadata() {
        let trace = Trace {
            meta: TraceMeta {
                seed: 1,
                scenario: "x".to_string(),
            },
            events: vec![TraceEvent::SchedulerDecision(SchedulerDecision {
                time_nanos: 10,
                chosen_index: 1,
                chosen_seq: None,
                frontier_seqs: vec![],
                frontier: vec![
                    FrontierEntry {
                        seq: 10,
                        kind: FrontierKind::Task,
                        component_id: None,
                        task_id: None,
                    },
                    FrontierEntry {
                        seq: 20,
                        kind: FrontierKind::Task,
                        component_id: None,
                        task_id: None,
                    },
                ],
            })],
        };

        let decisions = extract_decisions(&trace);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].chosen, 20);
        assert_eq!(decisions[0].key.choice_set, vec![10, 20]);
    }

    #[test]
    fn decision_script_normalizes_keys_on_insert_and_get() {
        let mut script = DecisionScript::new();

        // Insert an intentionally un-normalized key.
        script.insert(
            DecisionKey::new(DecisionKind::Frontier, 1, 0, vec![3, 2, 2, 1]),
            2,
        );

        // Lookup should succeed for differently-ordered/deduped variants.
        assert_eq!(
            script.get(&DecisionKey {
                kind: DecisionKind::Frontier,
                time_nanos: 1,
                ordinal: 0,
                choice_set: vec![1, 3, 2, 3, 2],
            }),
            Some(2)
        );

        // Different kind/time should not match.
        assert_eq!(
            script.get(&DecisionKey::new(
                DecisionKind::Frontier,
                2,
                0,
                vec![1, 2, 3]
            )),
            None
        );
        assert_eq!(
            script.get(&DecisionKey::new(
                DecisionKind::TokioReady,
                1,
                0,
                vec![1, 2, 3]
            )),
            None
        );
    }

    #[test]
    fn decision_script_match_decision_reports_mismatched_choice_set() {
        let mut script = DecisionScript::new();
        script.insert(
            DecisionKey::new(DecisionKind::Frontier, 10, 0, vec![5, 9]),
            5,
        );

        // Same (kind,time), but different choice set should yield a typed mismatch.
        let err = script
            .match_decision(&DecisionKey::new(DecisionKind::Frontier, 10, 0, vec![5, 7]))
            .unwrap_err();

        assert_eq!(err.kind, DecisionKind::Frontier);
        assert_eq!(err.time_nanos, 10);
        assert_eq!(err.ordinal, 0);
        assert_eq!(err.expected_choice_set, vec![5, 9]);
        assert_eq!(err.observed_choice_set, vec![5, 7]);

        // If there's no matching (kind,time) at all, return Ok(None).
        assert!(script
            .match_decision(&DecisionKey::new(DecisionKind::Frontier, 11, 0, vec![5, 7]))
            .unwrap()
            .is_none());
    }
}
