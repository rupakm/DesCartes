use std::time::Duration;

use des_core::SimTime;
use des_explore::monitor::{Monitor, MonitorConfig, QueueId};

#[test]
fn monitor_emits_window_summaries_and_detects_recovery() {
    let mut cfg = MonitorConfig::default();
    cfg.window = Duration::from_millis(100);
    cfg.baseline_warmup_windows = 2;
    cfg.recovery_hold_windows = 2;
    cfg.baseline_epsilon = 0.5;
    cfg.metastable_persist_windows = 5;

    let start = SimTime::zero();
    let mut m = Monitor::new(cfg, start);
    m.mark_post_spike_start(SimTime::from_millis(200));

    let q = QueueId(1);

    // Baseline windows: stable queue ~0, low latency.
    for i in 0..200 {
        let t = SimTime::from_millis(i);
        m.observe_queue_len(t, q, 0);
        if i % 10 == 0 {
            m.observe_complete(t, Duration::from_millis(5), true);
        }
    }

    // Inject a short "spike" window with high queue and timeouts.
    for i in 200..300 {
        let t = SimTime::from_millis(i);
        m.observe_queue_len(t, q, 50);
        if i % 10 == 0 {
            m.observe_timeout(t);
            m.observe_retry(t);
            m.observe_complete(t, Duration::from_millis(200), false);
        }
    }

    // Return to baseline-like behavior.
    for i in 300..500 {
        let t = SimTime::from_millis(i);
        m.observe_queue_len(t, q, 0);
        if i % 10 == 0 {
            m.observe_complete(t, Duration::from_millis(5), true);
        }
    }

    m.flush_up_to(SimTime::from_millis(500));

    // We should have produced several windows.
    assert!(m.timeline().len() >= 4);

    let status = m.status();
    assert!(status.windows_seen >= 4);

    // Recovery should eventually happen after returning to baseline.
    assert!(status.recovered);
    assert!(!status.metastable);
    assert!(status.recovery_time.is_some());
}
