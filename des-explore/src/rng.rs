use std::sync::{Arc, Mutex};

use descartes_core::{DrawSite, RandomProvider};
use rand::SeedableRng;

use crate::trace::{DrawValue, RandomDraw, TraceEvent, TraceRecorder};

/// A `des-core` `RandomProvider` implementation that logs draws into a trace.
///
/// This is a building block for bug reproduction and (later) importance sampling.
pub struct TracingRandomProvider {
    rng: rand::rngs::StdRng,
    recorder: Arc<Mutex<TraceRecorder>>,
}

impl TracingRandomProvider {
    pub fn new(seed: u64, recorder: Arc<Mutex<TraceRecorder>>) -> Self {
        Self {
            rng: rand::rngs::StdRng::seed_from_u64(seed),
            recorder,
        }
    }

    fn record_f64(&self, site: DrawSite, value: f64) {
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

impl RandomProvider for TracingRandomProvider {
    fn sample_exp_seconds(&mut self, site: DrawSite, rate: f64) -> f64 {
        use rand::Rng;

        let exp = rand_distr::Exp::new(rate).expect("rate must be positive");
        let value: f64 = self.rng.sample(exp);
        self.record_f64(site, value);
        value
    }
}
