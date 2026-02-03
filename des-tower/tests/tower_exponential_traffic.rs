//! Integration test for load balancing under exponential traffic.

use descartes_core::{scheduler, Execute, Executor, SimTime, Simulation};
use descartes_tower::{DesLoadBalanceStrategy, DesLoadBalancer, DesServiceBuilder, SimBody};
use http::{Method, Request};
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use tower::ServiceExt;

fn sample_exponential(rng: &mut ChaCha8Rng, rate: f64) -> Duration {
    // Inverse-CDF for Exp(rate): -ln(U)/rate
    let u: f64 = rng.gen_range(f64::MIN_POSITIVE..1.0);
    Duration::from_secs_f64(-u.ln() / rate)
}

#[test]
fn load_balancer_completes_requests_under_exponential_interarrival() {
    let mut simulation = Simulation::default();
    descartes_tokio::runtime::install(&mut simulation);

    let backends = vec![
        DesServiceBuilder::new("backend-1".to_string())
            .thread_capacity(1)
            .service_time(Duration::from_millis(20))
            .build(&mut simulation)
            .expect("build backend-1"),
        DesServiceBuilder::new("backend-2".to_string())
            .thread_capacity(1)
            .service_time(Duration::from_millis(25))
            .build(&mut simulation)
            .expect("build backend-2"),
        DesServiceBuilder::new("backend-3".to_string())
            .thread_capacity(1)
            .service_time(Duration::from_millis(30))
            .build(&mut simulation)
            .expect("build backend-3"),
    ];

    let load_balancer = DesLoadBalancer::new(backends, DesLoadBalanceStrategy::RoundRobin);

    let latencies: Rc<RefCell<Vec<Duration>>> = Rc::new(RefCell::new(Vec::new()));

    let mut rng = ChaCha8Rng::seed_from_u64(123);
    let rate = 50.0; // 50 req/s mean inter-arrival 20ms
    let total_requests = 50usize;

    let mut current = Duration::ZERO;

    for _ in 0..total_requests {
        current += sample_exponential(&mut rng, rate);

        let load_balancer = load_balancer.clone();
        let latencies = latencies.clone();

        simulation.schedule_closure(SimTime::from_duration(current), move |_scheduler| {
            let load_balancer = load_balancer.clone();
            let latencies = latencies.clone();

            descartes_tokio::task::spawn_local(async move {
                let request = Request::builder()
                    .method(Method::POST)
                    .uri("/api/test")
                    .body(SimBody::from_static("ping"))
                    .unwrap();

                let start = scheduler::current_time().expect("in scheduler context");
                let response = load_balancer
                    .oneshot(request)
                    .await
                    .expect("load balanced request succeeds");
                assert!(response.status().is_success());

                let end = scheduler::current_time().expect("in scheduler context");
                latencies.borrow_mut().push(end.duration_since(start));
            });
        });
    }

    Executor::timed(SimTime::from_duration(current + Duration::from_millis(500)))
        .execute(&mut simulation);

    let latencies = latencies.borrow();
    assert_eq!(latencies.len(), total_requests);

    // Sanity: all requests have non-zero latency.
    assert!(latencies.iter().all(|d| *d > Duration::ZERO));
}
