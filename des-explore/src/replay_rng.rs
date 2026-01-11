use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use des_core::{DrawSite, RandomProvider};

use crate::rng::TracingRandomProvider;
use crate::trace::{DrawValue, RandomDraw, Trace, TraceEvent, TraceRecorder};

/// A provider that replays a prefix of recorded draws.
///
/// It consumes `TraceEvent::RandomDraw` entries in order.
pub struct ReplayRandomProvider {
    draws: VecDeque<RandomDraw>,
    recorder: Arc<Mutex<TraceRecorder>>,
}

impl ReplayRandomProvider {
    pub fn new(prefix: &Trace, recorder: Arc<Mutex<TraceRecorder>>) -> Self {
        let draws = prefix
            .events
            .iter()
            .filter_map(|e| match e {
                TraceEvent::RandomDraw(d) => Some(d.clone()),
                _ => None,
            })
            .collect::<VecDeque<_>>();

        Self { draws, recorder }
    }

    fn record(&self, site: DrawSite, value: f64) {
        if let Ok(mut rec) = self.recorder.lock() {
            rec.record(TraceEvent::RandomDraw(RandomDraw {
                time_nanos: None,
                tag: site.tag.to_string(),
                site_id: site.site_id,
                value: DrawValue::F64(value),
            }));
        }
    }
}

impl RandomProvider for ReplayRandomProvider {
    fn sample_exp_seconds(&mut self, site: DrawSite, _rate: f64) -> f64 {
        let next = self.draws.pop_front().expect("replay trace exhausted");

        if next.site_id != site.site_id {
            panic!(
                "replay site mismatch: expected site_id={}, got {} (tag {})",
                site.site_id, next.site_id, next.tag
            );
        }

        let value = match next.value {
            DrawValue::F64(v) => v,
            other => panic!("unexpected draw value in replay: {other:?}"),
        };

        self.record(site, value);
        value
    }
}

/// A provider that uses a replay prefix first, then falls back to fresh RNG.
pub struct ChainedRandomProvider {
    replay: ReplayRandomProvider,
    fallback: TracingRandomProvider,
}

impl ChainedRandomProvider {
    pub fn new(prefix: &Trace, fallback_seed: u64, recorder: Arc<Mutex<TraceRecorder>>) -> Self {
        Self {
            replay: ReplayRandomProvider::new(prefix, recorder.clone()),
            fallback: TracingRandomProvider::new(fallback_seed, recorder),
        }
    }
}

impl RandomProvider for ChainedRandomProvider {
    fn sample_exp_seconds(&mut self, site: DrawSite, rate: f64) -> f64 {
        if !self.replay.draws.is_empty() {
            self.replay.sample_exp_seconds(site, rate)
        } else {
            self.fallback.sample_exp_seconds(site, rate)
        }
    }
}
