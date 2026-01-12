use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::dists::{
    ArrivalPattern, ExponentialDistribution, PoissonArrivals, ServiceTimeDistribution,
};
use des_core::{Simulation, SimulationConfig};
use des_explore::rng::TracingRandomProvider;
use des_explore::trace::{TraceEvent, TraceMeta, TraceRecorder};

/// Ensures distributions delegate sampling to an injected `RandomProvider`.
///
/// This validates the tracing/replay integration point in `des-core` distributions.
#[test]
fn distributions_can_delegate_sampling_to_provider() {
    let sim = Simulation::new(SimulationConfig { seed: 123 });

    let recorder = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
        seed: sim.config().seed,
        scenario: "provider_wiring".to_string(),
    })));

    let provider = Box::new(TracingRandomProvider::new(
        sim.config().seed ^ 0x9999,
        recorder.clone(),
    ));

    let mut arrivals = PoissonArrivals::from_config(sim.config(), 1.0)
        .with_provider(provider, des_core::draw_site!("arrival"));

    let _ = arrivals.next_arrival_time();

    let provider2 = Box::new(TracingRandomProvider::new(
        sim.config().seed ^ 0xAAAA,
        recorder.clone(),
    ));

    let mut service = ExponentialDistribution::from_config(sim.config(), 10.0)
        .with_provider(provider2, des_core::draw_site!("service"));

    let _ = service.sample();

    let trace = recorder.lock().unwrap().snapshot();
    assert_eq!(trace.meta.seed, 123);

    let draw_count = trace
        .events
        .iter()
        .filter(|e| matches!(e, TraceEvent::RandomDraw(_)))
        .count();

    assert_eq!(draw_count, 2);

    // sanity check: the site ids should be non-zero and stable
    let site_ids: Vec<u64> = trace
        .events
        .iter()
        .filter_map(|e| match e {
            TraceEvent::RandomDraw(d) => Some(d.site_id),
            _ => None,
        })
        .collect();

    assert!(site_ids[0] != 0);
    assert!(site_ids[1] != 0);

    // Ensure the samples are in seconds and can be turned into durations.
    let values: Vec<f64> = trace
        .events
        .iter()
        .filter_map(|e| match e {
            TraceEvent::RandomDraw(d) => match d.value {
                des_explore::trace::DrawValue::F64(v) => Some(v),
                _ => None,
            },
            _ => None,
        })
        .collect();

    assert!(Duration::from_secs_f64(values[0]) > Duration::ZERO);
    assert!(Duration::from_secs_f64(values[1]) > Duration::ZERO);
}
