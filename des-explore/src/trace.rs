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
    ///
    /// This is a placeholder for later stages; the initial implementation only
    /// records randomness.
    SchedulerDecision(SchedulerDecision),
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
