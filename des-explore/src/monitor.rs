use std::collections::HashMap;
use std::time::Duration;

use des_core::SimTime;
use hdrhistogram::Histogram;
use serde::{Deserialize, Serialize};

/// Identifier for a queue/metric source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QueueId(pub u64);

/// A single window summary of the monitored system trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSummary {
    pub window_start: SimTime,
    pub window_end: SimTime,

    // Queueing
    pub queue_mean: f64,
    pub queue_max: u64,

    // Counts
    pub completed_in_time: u64,
    pub completed_late: u64,
    pub dropped: u64,
    pub timeouts: u64,
    pub retries: u64,

    // Latency (for in-time completions)
    pub latency_mean_ms: f64,
    pub latency_p95_ms: Option<f64>,
    pub latency_p99_ms: Option<f64>,

    // Rates derived from window duration
    pub throughput_rps: f64,
    pub timeout_rate_rps: f64,
    pub retry_rate_rps: f64,
    pub drop_rate_rps: f64,

    // Derived ratios
    pub retry_amplification: f64,

    // Baseline distance (if baseline is enabled)
    pub distance_to_baseline: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub window: Duration,

    /// Number of windows used to build a baseline.
    pub baseline_warmup_windows: usize,

    /// Recovery requires this many consecutive windows within the threshold.
    pub recovery_hold_windows: usize,

    /// Threshold on baseline distance.
    pub baseline_epsilon: f64,

    /// Persistence window count for metastability (distance stays high).
    pub metastable_persist_windows: usize,

    /// Optional maximum allowed recovery time; exceeding it marks metastable.
    pub recovery_time_limit: Option<Duration>,

    /// If true, compute p95/p99 using a bounded histogram.
    pub track_latency_quantiles: bool,

    /// Histogram upper bound in milliseconds.
    pub latency_histogram_max_ms: u64,

    /// Cap applied to retry amplification when used in distances/scores.
    ///
    /// This avoids pathological values when `completed_in_time == 0` in a window.
    pub retry_amplification_cap: f64,

    /// Score weights for splitting/search.
    pub score_weights: ScoreWeights,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(1),
            baseline_warmup_windows: 20,
            recovery_hold_windows: 3,
            baseline_epsilon: 3.0,
            metastable_persist_windows: 10,
            recovery_time_limit: None,
            track_latency_quantiles: true,
            latency_histogram_max_ms: 60_000,
            retry_amplification_cap: 100.0,
            score_weights: ScoreWeights::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScoreWeights {
    pub queue_mean: f64,
    pub retry_amplification: f64,
    pub timeout_rate_rps: f64,
    pub distance: f64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            queue_mean: 1.0,
            retry_amplification: 1.0,
            timeout_rate_rps: 1.0,
            distance: 1.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MonitorStatus {
    pub windows_seen: usize,
    pub recovered: bool,
    pub metastable: bool,
    pub recovery_time: Option<Duration>,
    pub last_distance: Option<f64>,
    pub score: f64,
}

#[derive(Debug, Default, Clone)]
struct RunningStats {
    n: u64,
    mean: f64,
    m2: f64,
}

impl RunningStats {
    fn update(&mut self, x: f64) {
        self.n += 1;
        let n = self.n as f64;
        let delta = x - self.mean;
        self.mean += delta / n;
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;
    }

    fn variance(&self) -> f64 {
        if self.n < 2 {
            0.0
        } else {
            self.m2 / (self.n as f64 - 1.0)
        }
    }
}

#[derive(Debug, Default, Clone)]
struct BaselineModel {
    // Feature vector: [queue_mean, latency_mean_ms, drop_rate_rps, timeout_rate_rps, retry_amp, throughput_rps]
    stats: [RunningStats; 6],
}

impl BaselineModel {
    fn observe(&mut self, features: [f64; 6]) {
        for (s, x) in self.stats.iter_mut().zip(features) {
            s.update(x);
        }
    }

    fn mean(&self) -> [f64; 6] {
        let mut out = [0.0; 6];
        for (i, s) in self.stats.iter().enumerate() {
            out[i] = s.mean;
        }
        out
    }

    fn var(&self) -> [f64; 6] {
        let mut out = [0.0; 6];
        for (i, s) in self.stats.iter().enumerate() {
            // In many steady-state baselines, some features can have near-zero
            // variance (e.g., drop rate = 0). For exploration we want a stable,
            // bounded distance metric rather than exploding z-scores.
            out[i] = s.variance().max(1.0);
        }
        out
    }

    fn distance(&self, features: [f64; 6]) -> f64 {
        // Diagonal Mahalanobis distance.
        let mu = self.mean();
        let var = self.var();
        let mut acc = 0.0;
        for i in 0..6 {
            let z = (features[i] - mu[i]) / var[i].sqrt();
            acc += z * z;
        }
        acc.sqrt()
    }
}

#[derive(Debug, Clone)]
struct QueueState {
    len: u64,
    max: u64,

    window_start: SimTime,
    last_time: SimTime,

    // Integral of queue length over time in nanoseconds.
    integral_len_nanos: u128,
}

impl QueueState {
    fn new(window_start: SimTime) -> Self {
        Self {
            len: 0,
            max: 0,
            window_start,
            last_time: window_start,
            integral_len_nanos: 0,
        }
    }

    fn observe_len(&mut self, now: SimTime, new_len: u64) {
        let dt = now.duration_since(self.last_time);
        self.integral_len_nanos += self.len as u128 * dt.as_nanos();

        self.last_time = now;
        self.len = new_len;
        self.max = self.max.max(new_len);
    }

    fn finalize_window(&mut self, window_end: SimTime) -> (f64, u64) {
        // Close integral at window end.
        let dt = window_end.duration_since(self.last_time);
        self.integral_len_nanos += self.len as u128 * dt.as_nanos();
        self.last_time = window_end;

        (self.mean(window_end), self.max)
    }

    fn mean(&self, window_end: SimTime) -> f64 {
        let window_dur_nanos = window_end.duration_since(self.window_start).as_nanos();
        if window_dur_nanos == 0 {
            return self.len as f64;
        }

        self.integral_len_nanos as f64 / window_dur_nanos as f64
    }

    fn reset_for_next_window(&mut self, now: SimTime) {
        self.window_start = now;
        self.last_time = now;
        self.max = self.len;
        self.integral_len_nanos = 0;
    }
}

#[derive(Debug)]
pub struct Monitor {
    cfg: MonitorConfig,

    // Time management
    window_start: SimTime,
    window_end: SimTime,

    // Aggregated queue state across all queues.
    queues: HashMap<QueueId, QueueState>,

    // Counts (per window)
    completed_in_time: u64,
    completed_late: u64,
    dropped: u64,
    timeouts: u64,
    retries: u64,

    // Latency tracking (per window)
    latency_sum_ms: f64,
    latency_count: u64,
    latency_hist: Option<Histogram<u64>>,

    // Baseline and recovery
    baseline: BaselineModel,
    baseline_windows_seen: usize,
    recovered_hold: usize,
    post_spike_start: Option<SimTime>,
    metastable_persist: usize,

    // Outputs
    timeline: Vec<WindowSummary>,
}

impl Monitor {
    pub fn new(cfg: MonitorConfig, start: SimTime) -> Self {
        let window_end = start.add_duration(cfg.window);
        let latency_hist = if cfg.track_latency_quantiles {
            Some(
                Histogram::<u64>::new_with_bounds(1, cfg.latency_histogram_max_ms, 3)
                    .expect("valid histogram bounds"),
            )
        } else {
            None
        };

        Self {
            cfg,
            window_start: start,
            window_end,
            queues: HashMap::new(),
            completed_in_time: 0,
            completed_late: 0,
            dropped: 0,
            timeouts: 0,
            retries: 0,
            latency_sum_ms: 0.0,
            latency_count: 0,
            latency_hist,
            baseline: BaselineModel::default(),
            baseline_windows_seen: 0,
            recovered_hold: 0,
            post_spike_start: None,
            metastable_persist: 0,
            timeline: Vec::new(),
        }
    }

    /// Mark the spike end time. Recovery time is measured relative to this.
    pub fn mark_post_spike_start(&mut self, t: SimTime) {
        self.post_spike_start = Some(t);
    }

    pub fn timeline(&self) -> &[WindowSummary] {
        &self.timeline
    }

    pub fn status(&self) -> MonitorStatus {
        let last = self.timeline.last();
        let recovered = self.recovered_hold >= self.cfg.recovery_hold_windows;
        let metastable = self.is_metastable();

        MonitorStatus {
            windows_seen: self.timeline.len(),
            recovered,
            metastable,
            recovery_time: self.recovery_time(),
            last_distance: last.and_then(|w| w.distance_to_baseline),
            score: last.map(|w| self.score(w)).unwrap_or(0.0),
        }
    }

    pub fn observe_queue_len(&mut self, now: SimTime, queue: QueueId, len: u64) {
        self.flush_up_to(now);
        self.queues
            .entry(queue)
            .or_insert_with(|| QueueState::new(self.window_start))
            .observe_len(now, len);
    }

    pub fn observe_drop(&mut self, now: SimTime) {
        self.flush_up_to(now);
        self.dropped += 1;
    }

    pub fn observe_timeout(&mut self, now: SimTime) {
        self.flush_up_to(now);
        self.timeouts += 1;
    }

    pub fn observe_retry(&mut self, now: SimTime) {
        self.flush_up_to(now);
        self.retries += 1;
    }

    pub fn observe_complete(&mut self, now: SimTime, latency: Duration, in_time: bool) {
        self.flush_up_to(now);

        if in_time {
            self.completed_in_time += 1;

            let ms = latency.as_secs_f64() * 1000.0;
            self.latency_sum_ms += ms;
            self.latency_count += 1;

            if let Some(hist) = &mut self.latency_hist {
                let ms_u64 = ms
                    .ceil()
                    .clamp(1.0, self.cfg.latency_histogram_max_ms as f64)
                    as u64;
                let _ = hist.record(ms_u64);
            }
        } else {
            self.completed_late += 1;
        }
    }

    /// Flush windows up to time `now`.
    pub fn flush_up_to(&mut self, now: SimTime) {
        while now >= self.window_end {
            self.flush_window(self.window_end);
        }

        // Ensure queue integrals account for idle periods within the current window.
        for q in self.queues.values_mut() {
            // Observing the current length at `now` updates the integral.
            let current_len = q.len;
            q.observe_len(now, current_len);
        }
    }

    fn flush_window(&mut self, end: SimTime) {
        // Finalize queue stats.
        let mut total_integral_mean = 0.0;
        let mut total_max = 0;
        let mut contributing = 0;

        for q in self.queues.values_mut() {
            let (mean, max) = q.finalize_window(end);
            total_integral_mean += mean;
            total_max = total_max.max(max);
            contributing += 1;
        }

        let queue_mean = if contributing == 0 {
            0.0
        } else {
            total_integral_mean / contributing as f64
        };

        let window_secs = self.cfg.window.as_secs_f64();
        let throughput_rps = self.completed_in_time as f64 / window_secs;
        let timeout_rate_rps = self.timeouts as f64 / window_secs;
        let retry_rate_rps = self.retries as f64 / window_secs;
        let drop_rate_rps = self.dropped as f64 / window_secs;

        let latency_mean_ms = if self.latency_count == 0 {
            0.0
        } else {
            self.latency_sum_ms / self.latency_count as f64
        };

        let (p95, p99) = if let Some(hist) = &self.latency_hist {
            if hist.len() == 0 {
                (None, None)
            } else {
                (
                    Some(hist.value_at_quantile(0.95) as f64),
                    Some(hist.value_at_quantile(0.99) as f64),
                )
            }
        } else {
            (None, None)
        };

        let retry_amplification = if self.completed_in_time == 0 {
            f64::INFINITY
        } else {
            self.retries as f64 / self.completed_in_time as f64
        };

        let mut summary = WindowSummary {
            window_start: self.window_start,
            window_end: end,
            queue_mean,
            queue_max: total_max,
            completed_in_time: self.completed_in_time,
            completed_late: self.completed_late,
            dropped: self.dropped,
            timeouts: self.timeouts,
            retries: self.retries,
            latency_mean_ms,
            latency_p95_ms: p95,
            latency_p99_ms: p99,
            throughput_rps,
            timeout_rate_rps,
            retry_rate_rps,
            drop_rate_rps,
            retry_amplification,
            distance_to_baseline: None,
        };

        let retry_amp_feature = summary
            .retry_amplification
            .min(self.cfg.retry_amplification_cap);

        let features = [
            summary.queue_mean,
            summary.latency_mean_ms,
            summary.drop_rate_rps,
            summary.timeout_rate_rps,
            retry_amp_feature,
            summary.throughput_rps,
        ];

        if self.baseline_windows_seen < self.cfg.baseline_warmup_windows {
            self.baseline.observe(features);
            self.baseline_windows_seen += 1;
        } else {
            let d = self.baseline.distance(features);
            summary.distance_to_baseline = Some(d);

            if d <= self.cfg.baseline_epsilon {
                self.recovered_hold += 1;
                self.metastable_persist = 0;
            } else {
                self.recovered_hold = 0;
                self.metastable_persist += 1;
            }
        }

        self.timeline.push(summary);

        // Advance window.
        self.window_start = end;
        self.window_end = end.add_duration(self.cfg.window);

        // Reset per-window counters.
        self.completed_in_time = 0;
        self.completed_late = 0;
        self.dropped = 0;
        self.timeouts = 0;
        self.retries = 0;
        self.latency_sum_ms = 0.0;
        self.latency_count = 0;
        if let Some(hist) = &mut self.latency_hist {
            hist.reset();
        }

        for q in self.queues.values_mut() {
            q.reset_for_next_window(end);
        }
    }

    fn recovery_time(&self) -> Option<Duration> {
        let recovered = self.recovered_hold >= self.cfg.recovery_hold_windows;
        if !recovered {
            return None;
        }

        let post = self.post_spike_start?;
        let last_end = self.timeline.last()?.window_end;
        Some(last_end.duration_since(post))
    }

    fn is_metastable(&self) -> bool {
        if self.metastable_persist >= self.cfg.metastable_persist_windows {
            return true;
        }

        if let (Some(limit), Some(post), Some(last)) = (
            self.cfg.recovery_time_limit,
            self.post_spike_start,
            self.timeline.last(),
        ) {
            let dt = last.window_end.duration_since(post);
            if dt >= limit && self.recovered_hold < self.cfg.recovery_hold_windows {
                return true;
            }
        }

        false
    }

    fn score(&self, w: &WindowSummary) -> f64 {
        let mut s = 0.0;
        s += self.cfg.score_weights.queue_mean * w.queue_mean;
        s += self.cfg.score_weights.retry_amplification
            * w.retry_amplification.min(self.cfg.retry_amplification_cap);
        s += self.cfg.score_weights.timeout_rate_rps * w.timeout_rate_rps;
        if let Some(d) = w.distance_to_baseline {
            s += self.cfg.score_weights.distance * d;
        }
        s
    }
}
