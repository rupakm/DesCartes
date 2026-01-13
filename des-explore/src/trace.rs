use serde::{Deserialize, Serialize};

/// Recorded execution trace.
///
/// This is an intentionally minimal, forward-compatible format. The early stages
/// of `des-explore` use traces primarily for bug reproduction and replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    pub meta: TraceMeta,
    pub events: Vec<TraceEvent>,
}

impl Trace {
    pub fn new(meta: TraceMeta) -> Self {
        Self {
            meta,
            events: Vec::new(),
        }
    }

    pub fn record(&mut self, event: TraceEvent) {
        self.events.push(event);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceMeta {
    /// Global seed for this run.
    pub seed: u64,

    /// Human-readable scenario label.
    pub scenario: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TraceEvent {
    /// Random draw recorded at a tagged site.
    RandomDraw(RandomDraw),

    /// Scheduler decision among same-time frontier entries.
    SchedulerDecision(SchedulerDecision),

    /// Async runtime decision among ready tasks (tokio-level scheduling).
    AsyncRuntimeDecision(AsyncRuntimeDecision),

    /// Concurrency primitive observation (mutex/atomic/etc.).
    Concurrency(ConcurrencyTraceEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConcurrencyTraceEvent {
    /// Simulation time (nanos) when the event occurred, if known.
    #[serde(default)]
    pub time_nanos: Option<u64>,

    /// Async task id (tokio-level "thread id").
    pub task_id: u64,

    pub event: ConcurrencyEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConcurrencyEventKind {
    MutexContended {
        mutex_id: u64,
        #[serde(default)]
        waiter_count: usize,
    },
    MutexAcquire {
        mutex_id: u64,
    },
    MutexRelease {
        mutex_id: u64,
    },

    AtomicLoad {
        site_id: u64,
        ordering: AtomicOrdering,
        value: u64,
    },
    AtomicStore {
        site_id: u64,
        ordering: AtomicOrdering,
        value: u64,
    },
    AtomicFetchAdd {
        site_id: u64,
        ordering: AtomicOrdering,
        prev: u64,
        next: u64,
        delta: u64,
    },
    AtomicCompareExchange {
        site_id: u64,
        success_order: AtomicOrdering,
        failure_order: AtomicOrdering,
        current: u64,
        expected: u64,
        new: u64,
        succeeded: bool,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AtomicOrdering {
    Relaxed,
    Acquire,
    Release,
    AcqRel,
    SeqCst,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsyncRuntimeDecision {
    pub time_nanos: u64,

    /// Index chosen within the observed ready set.
    pub chosen_index: usize,

    /// Chosen task ID (preferred for replay).
    #[serde(default)]
    pub chosen_task_id: Option<u64>,

    /// Observed ready task IDs in order.
    #[serde(default)]
    pub ready_task_ids: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RandomDraw {
    /// Simulation time (nanos) when the draw occurred, if known.
    pub time_nanos: Option<u64>,

    /// Category (arrival/service/network/retry/etc.).
    pub tag: String,

    /// Stable identifier of the draw site.
    pub site_id: u64,

    /// Drawn value.
    pub value: DrawValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DrawValue {
    F64(f64),
    U64(u64),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FrontierKind {
    Component,
    Task,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontierEntry {
    pub seq: u64,
    pub kind: FrontierKind,

    /// Debug-only identifiers (not used for replay matching).
    #[serde(default)]
    pub component_id: Option<String>,

    /// Debug-only identifiers (not used for replay matching).
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerDecision {
    pub time_nanos: u64,

    /// Index of the chosen event within the frontier.
    pub chosen_index: usize,

    /// Sequence number of the chosen event (preferred for replay).
    #[serde(default)]
    pub chosen_seq: Option<u64>,

    /// Sequence numbers of the frontier entries in the observed order.
    #[serde(default)]
    pub frontier_seqs: Vec<u64>,

    /// Full frontier metadata for debugging.
    #[serde(default)]
    pub frontier: Vec<FrontierEntry>,
}

/// Trace builder used during simulation.
#[derive(Debug)]
pub struct TraceRecorder {
    trace: Trace,
}

impl TraceRecorder {
    pub fn new(meta: TraceMeta) -> Self {
        Self {
            trace: Trace::new(meta),
        }
    }

    pub fn record(&mut self, event: TraceEvent) {
        self.trace.record(event);
    }

    /// Borrow the current trace (read-only).
    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    /// Snapshot the current trace.
    pub fn snapshot(&self) -> Trace {
        self.trace.clone()
    }

    pub fn into_trace(self) -> Trace {
        self.trace
    }
}
