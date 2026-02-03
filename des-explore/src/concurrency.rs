#[cfg(feature = "tokio")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "tokio")]
use crate::trace::{
    AtomicOrdering, ConcurrencyEventKind, ConcurrencyTraceEvent, TraceEvent, TraceRecorder,
};

#[cfg(feature = "tokio")]
use des_tokio::concurrency::{ConcurrencyEvent, ConcurrencyRecorder};

#[cfg(feature = "tokio")]
#[derive(Clone)]
pub struct RecordingConcurrencyRecorder {
    recorder: Arc<Mutex<TraceRecorder>>,
}

#[cfg(feature = "tokio")]
impl RecordingConcurrencyRecorder {
    pub fn new(recorder: Arc<Mutex<TraceRecorder>>) -> Self {
        Self { recorder }
    }
}

#[cfg(feature = "tokio")]
impl ConcurrencyRecorder for RecordingConcurrencyRecorder {
    fn record(&self, event: ConcurrencyEvent) {
        let trace_event = TraceEvent::Concurrency(to_trace_event(event));
        self.recorder.lock().unwrap().record(trace_event);
    }
}

#[cfg(feature = "tokio")]
#[derive(Clone)]
pub struct TeeConcurrencyRecorder {
    recorders: Vec<Arc<dyn ConcurrencyRecorder>>,
}

#[cfg(feature = "tokio")]
impl TeeConcurrencyRecorder {
    pub fn new(recorders: Vec<Arc<dyn ConcurrencyRecorder>>) -> Self {
        Self { recorders }
    }
}

#[cfg(feature = "tokio")]
impl ConcurrencyRecorder for TeeConcurrencyRecorder {
    fn record(&self, event: ConcurrencyEvent) {
        for r in self.recorders.iter() {
            r.record(event.clone());
        }
    }
}

#[cfg(feature = "tokio")]
#[derive(Debug, Clone, thiserror::Error)]
pub enum ReplayConcurrencyError {
    #[error("trace ended before concurrency event {index}")]
    OutOfEvents { index: usize },

    #[error("concurrency event mismatch at index {index}: expected {expected:?}, got {actual:?}")]
    Mismatch {
        index: usize,
        expected: ConcurrencyTraceEvent,
        actual: ConcurrencyTraceEvent,
    },
}

#[cfg(feature = "tokio")]
#[derive(Clone)]
pub struct ReplayConcurrencyValidator {
    expected: Arc<Vec<ConcurrencyTraceEvent>>,
    next: Arc<Mutex<usize>>,
    error: Arc<Mutex<Option<ReplayConcurrencyError>>>,
}

#[cfg(feature = "tokio")]
impl ReplayConcurrencyValidator {
    pub fn from_trace_events(events: &[TraceEvent]) -> Self {
        let expected: Vec<ConcurrencyTraceEvent> = events
            .iter()
            .filter_map(|e| match e {
                TraceEvent::Concurrency(ev) => Some(ev.clone()),
                _ => None,
            })
            .collect();

        Self {
            expected: Arc::new(expected),
            next: Arc::new(Mutex::new(0)),
            error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn has_expected_events(&self) -> bool {
        !self.expected.is_empty()
    }

    pub fn error(&self) -> Option<ReplayConcurrencyError> {
        self.error.lock().unwrap().clone()
    }

    pub fn take_error(&self) -> Option<ReplayConcurrencyError> {
        self.error.lock().unwrap().take()
    }

    fn fail(&self, err: ReplayConcurrencyError) {
        let mut slot = self.error.lock().unwrap();
        if slot.is_none() {
            *slot = Some(err);
        }
    }
}

#[cfg(feature = "tokio")]
impl ConcurrencyRecorder for ReplayConcurrencyValidator {
    fn record(&self, event: ConcurrencyEvent) {
        if self.error.lock().unwrap().is_some() {
            return;
        }

        let actual = to_trace_event(event);
        let mut idx = self.next.lock().unwrap();
        let index = *idx;

        let Some(expected) = self.expected.get(index).cloned() else {
            self.fail(ReplayConcurrencyError::OutOfEvents { index });
            return;
        };

        if expected != actual {
            self.fail(ReplayConcurrencyError::Mismatch {
                index,
                expected,
                actual,
            });
            return;
        }

        *idx += 1;
    }
}

#[cfg(feature = "tokio")]
fn to_trace_event(event: ConcurrencyEvent) -> ConcurrencyTraceEvent {
    match event {
        ConcurrencyEvent::MutexContended {
            mutex_id,
            task_id,
            time_nanos,
            waiter_count,
        } => ConcurrencyTraceEvent {
            time_nanos,
            task_id,
            event: ConcurrencyEventKind::MutexContended {
                mutex_id,
                waiter_count,
            },
        },
        ConcurrencyEvent::MutexAcquire {
            mutex_id,
            task_id,
            time_nanos,
        } => ConcurrencyTraceEvent {
            time_nanos,
            task_id,
            event: ConcurrencyEventKind::MutexAcquire { mutex_id },
        },
        ConcurrencyEvent::MutexRelease {
            mutex_id,
            task_id,
            time_nanos,
        } => ConcurrencyTraceEvent {
            time_nanos,
            task_id,
            event: ConcurrencyEventKind::MutexRelease { mutex_id },
        },
        ConcurrencyEvent::AtomicLoad {
            site_id,
            task_id,
            time_nanos,
            ordering,
            value,
        } => ConcurrencyTraceEvent {
            time_nanos,
            task_id,
            event: ConcurrencyEventKind::AtomicLoad {
                site_id,
                ordering: to_atomic_ordering(ordering),
                value,
            },
        },
        ConcurrencyEvent::AtomicStore {
            site_id,
            task_id,
            time_nanos,
            ordering,
            value,
        } => ConcurrencyTraceEvent {
            time_nanos,
            task_id,
            event: ConcurrencyEventKind::AtomicStore {
                site_id,
                ordering: to_atomic_ordering(ordering),
                value,
            },
        },
        ConcurrencyEvent::AtomicFetchAdd {
            site_id,
            task_id,
            time_nanos,
            ordering,
            prev,
            next,
            delta,
        } => ConcurrencyTraceEvent {
            time_nanos,
            task_id,
            event: ConcurrencyEventKind::AtomicFetchAdd {
                site_id,
                ordering: to_atomic_ordering(ordering),
                prev,
                next,
                delta,
            },
        },
        ConcurrencyEvent::AtomicCompareExchange {
            site_id,
            task_id,
            time_nanos,
            success_order,
            failure_order,
            current,
            expected,
            new,
            succeeded,
        } => ConcurrencyTraceEvent {
            time_nanos,
            task_id,
            event: ConcurrencyEventKind::AtomicCompareExchange {
                site_id,
                success_order: to_atomic_ordering(success_order),
                failure_order: to_atomic_ordering(failure_order),
                current,
                expected,
                new,
                succeeded,
            },
        },
    }
}

#[cfg(feature = "tokio")]
fn to_atomic_ordering(ordering: std::sync::atomic::Ordering) -> AtomicOrdering {
    use std::sync::atomic::Ordering;

    match ordering {
        Ordering::Relaxed => AtomicOrdering::Relaxed,
        Ordering::Acquire => AtomicOrdering::Acquire,
        Ordering::Release => AtomicOrdering::Release,
        Ordering::AcqRel => AtomicOrdering::AcqRel,
        Ordering::SeqCst => AtomicOrdering::SeqCst,
        _ => AtomicOrdering::SeqCst,
    }
}
