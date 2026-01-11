use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use des_core::{DrawSite, RandomProvider};

use crate::shared_rng::SharedTracingRandomProvider;
use crate::trace::{DrawValue, RandomDraw, Trace, TraceEvent, TraceRecorder};

#[derive(Clone)]
pub struct SharedChainedRandomProvider {
    draws: Arc<Mutex<VecDeque<RandomDraw>>>,
    recorder: Arc<Mutex<TraceRecorder>>,
    fallback: SharedTracingRandomProvider,
}

impl SharedChainedRandomProvider {
    pub fn new(
        prefix: Option<&Trace>,
        fallback_seed: u64,
        recorder: Arc<Mutex<TraceRecorder>>,
    ) -> Self {
        let draws = prefix
            .map(|t| {
                t.events
                    .iter()
                    .filter_map(|e| match e {
                        TraceEvent::RandomDraw(d) => Some(d.clone()),
                        _ => None,
                    })
                    .collect::<VecDeque<_>>()
            })
            .unwrap_or_default();

        Self {
            draws: Arc::new(Mutex::new(draws)),
            recorder: recorder.clone(),
            fallback: SharedTracingRandomProvider::new(fallback_seed, recorder),
        }
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

impl RandomProvider for SharedChainedRandomProvider {
    fn sample_exp_seconds(&mut self, site: DrawSite, rate: f64) -> f64 {
        if let Some(next) = self.draws.lock().unwrap().pop_front() {
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
        } else {
            self.fallback.sample_exp_seconds(site, rate)
        }
    }
}
