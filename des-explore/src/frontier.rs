use std::sync::{Arc, Mutex};

use des_core::{EventFrontierPolicy, FrontierEvent, FrontierEventKind, FrontierSignature, SimTime};

use crate::trace::{FrontierEntry, FrontierKind, SchedulerDecision, TraceEvent, TraceRecorder};

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

/// Wraps an existing frontier policy and records choices into the trace.
#[derive(Clone)]
pub struct RecordingFrontierPolicy<P> {
    inner: P,
    recorder: Arc<Mutex<TraceRecorder>>,
}

impl<P> RecordingFrontierPolicy<P> {
    pub fn new(inner: P, recorder: Arc<Mutex<TraceRecorder>>) -> Self {
        Self { inner, recorder }
    }

    pub fn inner_mut(&mut self) -> &mut P {
        &mut self.inner
    }
}

impl<P: EventFrontierPolicy> EventFrontierPolicy for RecordingFrontierPolicy<P> {
    fn choose(&mut self, time: SimTime, frontier: &[FrontierEvent]) -> usize {
        let chosen_index = self.inner.choose(time, frontier);

        let chosen_index = chosen_index.min(frontier.len().saturating_sub(1));
        let chosen_seq = frontier.get(chosen_index).map(|e| e.seq);

        let FrontierSignature {
            time_nanos,
            frontier_seqs,
        } = FrontierSignature::new(time, frontier);

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

/// Replays recorded frontier choices.
///
/// This is intended to be used with a trace produced by [`RecordingFrontierPolicy`].
///
/// Note: [`EventFrontierPolicy::choose`] cannot return a `Result`, so replay
/// mismatches are stored internally and can be retrieved via [`ReplayFrontierPolicy::take_error`].
#[derive(Debug)]
struct ReplayFrontierState {
    decisions: Vec<SchedulerDecision>,
    next: usize,
    error: Option<ReplayFrontierError>,
}

#[derive(Debug, Clone)]
pub struct ReplayFrontierPolicy {
    state: Arc<Mutex<ReplayFrontierState>>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ReplayFrontierError {
    #[error(
        "trace ended before scheduler decision {index} (time={time_nanos}, frontier={frontier_seqs:?})"
    )]
    OutOfDecisions {
        index: usize,
        time_nanos: u64,
        frontier_seqs: Vec<u64>,
    },

    #[error("time mismatch at decision {index}: expected {expected}, got {actual}")]
    TimeMismatch {
        index: usize,
        expected: u64,
        actual: u64,
    },

    #[error(
        "frontier mismatch at decision {index} (time={time_nanos}): expected {expected:?}, got {actual:?}"
    )]
    FrontierMismatch {
        index: usize,
        time_nanos: u64,
        expected: Vec<u64>,
        actual: Vec<u64>,
    },

    #[error(
        "chosen seq {chosen} not found at decision {index} (time={time_nanos}, frontier={frontier:?})"
    )]
    ChosenSeqNotFound {
        index: usize,
        time_nanos: u64,
        chosen: u64,
        frontier: Vec<u64>,
    },

    #[error(
        "chosen index {chosen_index} out of bounds at decision {index} (time={time_nanos}, frontier_len={frontier_len})"
    )]
    ChosenIndexOutOfBounds {
        index: usize,
        time_nanos: u64,
        chosen_index: usize,
        frontier_len: usize,
    },
}

impl ReplayFrontierPolicy {
    pub fn from_trace_events(events: &[crate::trace::TraceEvent]) -> Self {
        let decisions = events
            .iter()
            .filter_map(|e| match e {
                crate::trace::TraceEvent::SchedulerDecision(d) => Some(d.clone()),
                _ => None,
            })
            .collect();

        Self {
            state: Arc::new(Mutex::new(ReplayFrontierState {
                decisions,
                next: 0,
                error: None,
            })),
        }
    }

    pub fn error(&self) -> Option<ReplayFrontierError> {
        self.state.lock().unwrap().error.clone()
    }

    pub fn take_error(&self) -> Option<ReplayFrontierError> {
        self.state.lock().unwrap().error.take()
    }

    fn expected_seqs(decision: &SchedulerDecision) -> Vec<u64> {
        if !decision.frontier_seqs.is_empty() {
            return decision.frontier_seqs.clone();
        }
        if !decision.frontier.is_empty() {
            return decision.frontier.iter().map(|e| e.seq).collect();
        }
        Vec::new()
    }
}

impl EventFrontierPolicy for ReplayFrontierPolicy {
    fn choose(&mut self, time: SimTime, frontier: &[FrontierEvent]) -> usize {
        let mut state = self.state.lock().unwrap();
        if state.error.is_some() {
            return 0;
        }

        let index = state.next;
        let FrontierSignature {
            time_nanos: actual_time,
            frontier_seqs: actual_seqs,
        } = FrontierSignature::new(time, frontier);

        let Some(decision) = state.decisions.get(index).cloned() else {
            state.error = Some(ReplayFrontierError::OutOfDecisions {
                index,
                time_nanos: actual_time,
                frontier_seqs: actual_seqs,
            });
            return 0;
        };
        state.next += 1;

        if decision.time_nanos != actual_time {
            state.error = Some(ReplayFrontierError::TimeMismatch {
                index,
                expected: decision.time_nanos,
                actual: actual_time,
            });
            return 0;
        }

        let expected_seqs = Self::expected_seqs(&decision);
        if !expected_seqs.is_empty() && expected_seqs != actual_seqs {
            state.error = Some(ReplayFrontierError::FrontierMismatch {
                index,
                time_nanos: decision.time_nanos,
                expected: expected_seqs,
                actual: actual_seqs,
            });
            return 0;
        }

        if let Some(chosen_seq) = decision.chosen_seq {
            if let Some(idx) = actual_seqs.iter().position(|&s| s == chosen_seq) {
                return idx;
            }

            state.error = Some(ReplayFrontierError::ChosenSeqNotFound {
                index,
                time_nanos: decision.time_nanos,
                chosen: chosen_seq,
                frontier: actual_seqs,
            });
            return 0;
        }

        if frontier.is_empty() {
            return 0;
        }

        if decision.chosen_index >= frontier.len() {
            state.error = Some(ReplayFrontierError::ChosenIndexOutOfBounds {
                index,
                time_nanos: decision.time_nanos,
                chosen_index: decision.chosen_index,
                frontier_len: frontier.len(),
            });
        }

        decision.chosen_index.min(frontier.len() - 1)
    }
}
