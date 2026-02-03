use std::sync::{Arc, Mutex};

use descartes_core::async_runtime::{ReadyTaskPolicy, ReadyTaskSignature, TaskId};
use descartes_core::SimTime;

use crate::schedule_explore::{DecisionKey, DecisionKind, DecisionScript};
use crate::trace::{AsyncRuntimeDecision, TraceEvent, TraceRecorder};

/// Ready-task policy wrapper used for schedule exploration.
///
/// Combines:
/// - an optional [`DecisionScript`] override
/// - an underlying policy `P`
/// - trace recording of the realized decision
#[derive(Clone)]
pub struct ExplorationReadyTaskPolicy<P> {
    inner: P,
    script: Option<Arc<DecisionScript>>,
    recorder: Arc<Mutex<TraceRecorder>>,
    next_ordinal: u64,
}

impl<P> ExplorationReadyTaskPolicy<P> {
    pub fn new(
        inner: P,
        script: Option<Arc<DecisionScript>>,
        recorder: Arc<Mutex<TraceRecorder>>,
    ) -> Self {
        Self {
            inner,
            script,
            recorder,
            next_ordinal: 0,
        }
    }

    pub fn inner_mut(&mut self) -> &mut P {
        &mut self.inner
    }
}

impl<P: ReadyTaskPolicy> ReadyTaskPolicy for ExplorationReadyTaskPolicy<P> {
    fn choose(&mut self, time: SimTime, ready: &[TaskId]) -> usize {
        let signature = ReadyTaskSignature::new(time, ready);
        let ordinal = self.next_ordinal;
        self.next_ordinal += 1;

        let key = DecisionKey::new(
            DecisionKind::TokioReady,
            signature.time_nanos,
            ordinal,
            signature.ready_task_ids.clone(),
        );

        let chosen_index = match self.script.as_ref() {
            None => self.inner.choose(time, ready),
            Some(script) => match script.get(&key) {
                Some(chosen_task_id) => {
                    if let Some(pos) = signature
                        .ready_task_ids
                        .iter()
                        .position(|&id| id == chosen_task_id)
                    {
                        pos
                    } else {
                        tracing::warn!(
                            time_nanos = signature.time_nanos,
                            forced_task_id = chosen_task_id,
                            ready_task_ids = ?signature.ready_task_ids,
                            "DecisionScript forced a task id not present; falling back to base policy"
                        );
                        self.inner.choose(time, ready)
                    }
                }
                None => self.inner.choose(time, ready),
            },
        };

        let chosen_index = chosen_index.min(ready.len().saturating_sub(1));
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

/// Wraps an existing ready-task policy and records choices into the trace.
#[derive(Clone)]
pub struct RecordingReadyTaskPolicy<P> {
    inner: P,
    recorder: Arc<Mutex<TraceRecorder>>,
}

impl<P> RecordingReadyTaskPolicy<P> {
    pub fn new(inner: P, recorder: Arc<Mutex<TraceRecorder>>) -> Self {
        Self { inner, recorder }
    }

    pub fn inner_mut(&mut self) -> &mut P {
        &mut self.inner
    }
}

impl<P: ReadyTaskPolicy> ReadyTaskPolicy for RecordingReadyTaskPolicy<P> {
    fn choose(&mut self, time: SimTime, ready: &[TaskId]) -> usize {
        let chosen_index = self.inner.choose(time, ready);
        let chosen_index = chosen_index.min(ready.len().saturating_sub(1));

        let signature = ReadyTaskSignature::new(time, ready);
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

/// Replays recorded ready-task choices.
///
/// If the input trace contains no async-runtime decision events, this policy
/// falls back to FIFO and emits a warning once per run.
#[derive(Debug, Clone)]
pub struct ReplayReadyTaskPolicy {
    state: Arc<Mutex<ReplayReadyTaskState>>,
}

#[derive(Debug)]
struct ReplayReadyTaskState {
    decisions: Vec<AsyncRuntimeDecision>,
    next: usize,
    error: Option<ReplayReadyTaskError>,

    fifo_fallback: bool,
    warned: bool,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ReplayReadyTaskError {
    #[error(
        "trace ended before async-runtime decision {index} (time={time_nanos}, ready={ready_task_ids:?})"
    )]
    OutOfDecisions {
        index: usize,
        time_nanos: u64,
        ready_task_ids: Vec<u64>,
    },

    #[error("time mismatch at async-runtime decision {index}: expected {expected}, got {actual}")]
    TimeMismatch {
        index: usize,
        expected: u64,
        actual: u64,
    },

    #[error(
        "ready set mismatch at async-runtime decision {index} (time={time_nanos}): expected {expected:?}, got {actual:?}"
    )]
    ReadySetMismatch {
        index: usize,
        time_nanos: u64,
        expected: Vec<u64>,
        actual: Vec<u64>,
    },

    #[error(
        "chosen task id {chosen} not found at async-runtime decision {index} (time={time_nanos}, ready={ready:?})"
    )]
    ChosenTaskNotFound {
        index: usize,
        time_nanos: u64,
        chosen: u64,
        ready: Vec<u64>,
    },
}

impl ReplayReadyTaskPolicy {
    pub fn from_trace_events(events: &[TraceEvent]) -> Self {
        let decisions: Vec<AsyncRuntimeDecision> = events
            .iter()
            .filter_map(|e| match e {
                TraceEvent::AsyncRuntimeDecision(d) => Some(d.clone()),
                _ => None,
            })
            .collect();

        let fifo_fallback = decisions.is_empty();

        Self {
            state: Arc::new(Mutex::new(ReplayReadyTaskState {
                decisions,
                next: 0,
                error: None,
                fifo_fallback,
                warned: false,
            })),
        }
    }

    pub fn take_error(&self) -> Option<ReplayReadyTaskError> {
        self.state.lock().unwrap().error.take()
    }

    pub fn error(&self) -> Option<ReplayReadyTaskError> {
        self.state.lock().unwrap().error.clone()
    }
}

impl ReadyTaskPolicy for ReplayReadyTaskPolicy {
    fn choose(&mut self, time: SimTime, ready: &[TaskId]) -> usize {
        let mut state = self.state.lock().unwrap();
        if state.error.is_some() {
            return 0;
        }

        let signature = ReadyTaskSignature::new(time, ready);

        if state.fifo_fallback {
            if !state.warned {
                state.warned = true;
                let msg = "Replay trace contains no async-runtime (tokio) scheduling decisions; falling back to FIFO ready-task polling order";
                tracing::warn!("{msg}");
                eprintln!("Warning: {msg}");
            }
            return 0;
        }

        let index = state.next;
        let Some(decision) = state.decisions.get(index).cloned() else {
            state.error = Some(ReplayReadyTaskError::OutOfDecisions {
                index,
                time_nanos: signature.time_nanos,
                ready_task_ids: signature.ready_task_ids,
            });
            return 0;
        };
        state.next += 1;

        if decision.time_nanos != signature.time_nanos {
            state.error = Some(ReplayReadyTaskError::TimeMismatch {
                index,
                expected: decision.time_nanos,
                actual: signature.time_nanos,
            });
            return 0;
        }

        if !decision.ready_task_ids.is_empty() {
            let mut expected_norm = decision.ready_task_ids.clone();
            expected_norm.sort_unstable();
            expected_norm.dedup();

            let mut actual_norm = signature.ready_task_ids.clone();
            actual_norm.sort_unstable();
            actual_norm.dedup();

            if expected_norm != actual_norm {
                state.error = Some(ReplayReadyTaskError::ReadySetMismatch {
                    index,
                    time_nanos: decision.time_nanos,
                    expected: decision.ready_task_ids,
                    actual: signature.ready_task_ids,
                });
                return 0;
            }
        }

        if let Some(chosen_task_id) = decision.chosen_task_id {
            if let Some(pos) = signature
                .ready_task_ids
                .iter()
                .position(|&id| id == chosen_task_id)
            {
                return pos;
            }

            state.error = Some(ReplayReadyTaskError::ChosenTaskNotFound {
                index,
                time_nanos: decision.time_nanos,
                chosen: chosen_task_id,
                ready: signature.ready_task_ids,
            });
            return 0;
        }

        if ready.is_empty() {
            return 0;
        }

        decision.chosen_index.min(ready.len() - 1)
    }
}
