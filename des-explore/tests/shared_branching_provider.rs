use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::dists::{
    ArrivalPattern, ExponentialDistribution, PoissonArrivals, ServiceTimeDistribution,
};
use des_core::{Component, Key, SimTime, Simulation, SimulationConfig};

use des_explore::harness::HarnessContext;
use des_explore::trace::{Trace, TraceEvent, TraceMeta, TraceRecorder};

#[derive(Debug)]
enum Ev {
    Tick,
}

struct DualRng {
    arrivals: PoissonArrivals,
    service: ExponentialDistribution,
}

impl Component for DualRng {
    type Event = Ev;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        _event: &Self::Event,
        scheduler: &mut des_core::Scheduler,
    ) {
        // Consume one draw from each distribution.
        let dt = self.arrivals.next_arrival_time();
        let _svc = self.service.sample();

        scheduler.schedule(SimTime::from_duration(dt), self_id, Ev::Tick);
    }
}

fn draws(trace: &Trace) -> Vec<(u64, f64)> {
    trace
        .events
        .iter()
        .filter_map(|e| match e {
            TraceEvent::RandomDraw(d) => match d.value {
                des_explore::trace::DrawValue::F64(v) => Some((d.site_id, v)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Ensures prefix replay works across multiple distributions.
///
/// This is a regression test for the "shared RNG stream" requirement:
/// when multiple distributions draw randomness, replay must consume a single
/// globally ordered draw stream.
#[test]
fn shared_branching_provider_replays_across_two_distributions() {
    let sim_config = SimulationConfig { seed: 123 };

    // First run: generate a prefix trace.
    let recorder = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
        seed: sim_config.seed,
        scenario: "shared_prefix".to_string(),
    })));
    let ctx = HarnessContext::new(recorder.clone());

    let prefix_provider = ctx.shared_branching_provider(None, 999);

    // Use explicit site IDs here to keep this test stable. In real use, site IDs
    // are stable because the same `setup(...)` function is reused for all runs.
    let arrival_site = des_core::DrawSite::new("arrival", 1);
    let service_site = des_core::DrawSite::new("service", 2);

    let mut sim = Simulation::new(sim_config.clone());
    let arrivals = PoissonArrivals::from_config(sim.config(), 10.0)
        .with_provider(Box::new(prefix_provider.clone()), arrival_site);
    let service = ExponentialDistribution::from_config(sim.config(), 10.0)
        .with_provider(Box::new(prefix_provider), service_site);

    let key = sim.add_component(DualRng { arrivals, service });
    sim.schedule_now(key, Ev::Tick);

    // Step a few times.
    for _ in 0..5 {
        sim.step();
    }

    let prefix = recorder.lock().unwrap().snapshot();
    let prefix_draws = draws(&prefix);
    assert!(prefix_draws.len() >= 5);

    // Second run: replay the prefix while using both distributions.
    let recorder2 = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
        seed: sim_config.seed,
        scenario: "shared_replay".to_string(),
    })));
    let ctx2 = HarnessContext::new(recorder2.clone());

    let replay_provider = ctx2.shared_branching_provider(Some(&prefix), 1001);

    let mut sim2 = Simulation::new(sim_config);
    let arrivals2 = PoissonArrivals::from_config(sim2.config(), 10.0)
        .with_provider(Box::new(replay_provider.clone()), arrival_site);
    let service2 = ExponentialDistribution::from_config(sim2.config(), 10.0)
        .with_provider(Box::new(replay_provider), service_site);

    let key2 = sim2.add_component(DualRng {
        arrivals: arrivals2,
        service: service2,
    });
    sim2.schedule_now(key2, Ev::Tick);

    for _ in 0..5 {
        sim2.step();
    }

    let replay_trace = recorder2.lock().unwrap().snapshot();
    let replay_draws = draws(&replay_trace);

    assert!(replay_draws.len() >= prefix_draws.len());
    assert_eq!(&replay_draws[..prefix_draws.len()], &prefix_draws[..]);

    // Ensure we can keep running past the prefix.
    sim2.step();
    ctx2.recorder()
        .lock()
        .unwrap()
        .record(TraceEvent::RandomDraw(des_explore::trace::RandomDraw {
            time_nanos: Some(SimTime::from_duration(Duration::from_millis(1)).as_nanos()),
            tag: "dummy".to_string(),
            site_id: 0,
            value: des_explore::trace::DrawValue::Bool(true),
        }));
}
