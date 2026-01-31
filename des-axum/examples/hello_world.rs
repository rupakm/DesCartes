use des_axum::Transport;
use des_core::{Execute, Executor, SimTime, Simulation};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sim = Simulation::default();

    // Install deterministic async runtime.
    des_tokio::runtime::install(&mut sim);

    // Transport (deterministic, no latency by default).
    let transport = Transport::install_default(&mut sim);

    // Serve an axum router as a simulated endpoint.
    let app = axum::Router::new().route(
        "/hello",
        axum::routing::get(|| async move { "hello" }),
    );

    transport.serve_named(&mut sim, "hello", "hello-1", app)?;

    let client = transport.connect(&mut sim, "hello")?;

    des_tokio::task::spawn_local(async move {
        for i in 0..5u32 {
            if i > 0 {
                des_tokio::time::sleep(Duration::from_millis(50)).await;
            }

            let t = des_core::scheduler::current_time().unwrap_or(SimTime::zero());
            let resp = client.get("/hello", Some(Duration::from_secs(1))).await;
            match resp {
                Ok(resp) => {
                    let status = resp.status();
                    println!("t={t:?} i={i} status={status}");
                }
                Err(e) => {
                    println!("t={t:?} i={i} error={e}");
                }
            }
        }
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    Ok(())
}
