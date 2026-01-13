use std::sync::atomic::Ordering;

/// Events emitted by concurrency primitives for exploration tooling.
///
/// These events are **observational**: they do not affect scheduling by default.
/// Exploration tooling (e.g. `des-explore`) can install a recorder to capture
/// and validate these events during record/replay runs.
#[derive(Debug, Clone)]
pub enum ConcurrencyEvent {
    MutexContended {
        mutex_id: u64,
        task_id: u64,
        time_nanos: Option<u64>,
        waiter_count: usize,
    },
    MutexAcquire {
        mutex_id: u64,
        task_id: u64,
        time_nanos: Option<u64>,
    },
    MutexRelease {
        mutex_id: u64,
        task_id: u64,
        time_nanos: Option<u64>,
    },

    AtomicLoad {
        site_id: u64,
        task_id: u64,
        time_nanos: Option<u64>,
        ordering: Ordering,
        value: u64,
    },
    AtomicStore {
        site_id: u64,
        task_id: u64,
        time_nanos: Option<u64>,
        ordering: Ordering,
        value: u64,
    },
    AtomicFetchAdd {
        site_id: u64,
        task_id: u64,
        time_nanos: Option<u64>,
        ordering: Ordering,
        prev: u64,
        next: u64,
        delta: u64,
    },
    AtomicCompareExchange {
        site_id: u64,
        task_id: u64,
        time_nanos: Option<u64>,
        success_order: Ordering,
        failure_order: Ordering,
        current: u64,
        expected: u64,
        new: u64,
        succeeded: bool,
    },
}

pub trait ConcurrencyRecorder: Send + Sync {
    fn record(&self, event: ConcurrencyEvent);
}
