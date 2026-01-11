use std::sync::{Arc, Mutex};

use des_core::{DrawSite, RandomProvider};
use rand::SeedableRng;

use crate::trace::{DrawValue, RandomDraw, TraceEvent, TraceRecorder};

/// A cloneable `RandomProvider` implementation that shares a single RNG stream.
///
/// This is important for replay/splitting when multiple distributions must
/// consume randomness from a single, globally ordered stream.
#[derive(Clone)]
pub struct SharedTracingRandomProvider {
    rng: Arc<Mutex<rand::rngs::StdRng>>,
    recorder: Arc<Mutex<TraceRecorder>>,
}

impl SharedTracingRandomProvider {
    pub fn new(seed: u64, recorder: Arc<Mutex<TraceRecorder>>) -> Self {
        Self {
            rng: Arc::new(Mutex::new(rand::rngs::StdRng::seed_from_u64(seed))),
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

impl RandomProvider for SharedTracingRandomProvider {
    fn sample_exp_seconds(&mut self, site: DrawSite, rate: f64) -> f64 {
        use rand::Rng;

        let exp = rand_distr::Exp::new(rate).expect("rate must be positive");
        let mut rng = self.rng.lock().unwrap();
        let value: f64 = rng.sample(exp);
        drop(rng);

        self.record_f64(site, value);
        value
    }
}
