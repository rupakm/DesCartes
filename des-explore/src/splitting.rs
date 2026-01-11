use std::sync::{Arc, Mutex};

use des_core::{SimTime, Simulation, SimulationConfig};

use crate::harness::{HarnessContext, HarnessError};
use crate::monitor::{Monitor, MonitorStatus};
use crate::trace::{Trace, TraceMeta, TraceRecorder};

/// Configuration for multilevel splitting bug finding.
///
/// This is a skeleton implementation intended to find counterexample traces
/// (e.g., metastable behavior) rather than produce unbiased probability
/// estimates.
#[derive(Debug, Clone)]
pub struct SplittingConfig {
    /// Score thresholds in increasing order.
    pub levels: Vec<f64>,

    /// Number of continuation branches spawned per successful particle.
    pub branch_factor: usize,

    /// Max number of particles kept per level (bounds work).
    pub max_particles_per_level: usize,

    /// Simulation end time (hard horizon for each run).
    pub end_time: SimTime,

    /// Whether to install the `des-tokio` runtime.
    pub install_tokio: bool,
}

impl Default for SplittingConfig {
    fn default() -> Self {
        Self {
            levels: vec![50.0, 150.0, 300.0],
            branch_factor: 2,
            max_particles_per_level: 32,
            end_time: SimTime::from_secs(60),
            install_tokio: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FoundBug {
    pub trace: Trace,
    pub status: MonitorStatus,
}

#[derive(Debug, thiserror::Error)]
pub enum SplittingError {
    #[error(transparent)]
    Harness(#[from] HarnessError),
}

#[derive(Clone)]
struct Particle {
    /// Trace prefix to replay before diverging.
    prefix: Option<Trace>,

    /// Seed used for post-prefix randomness.
    continuation_seed: u64,
}

fn run_until_threshold_or_end(
    sim: &mut Simulation,
    monitor: &Arc<Mutex<Monitor>>,
    end_time: SimTime,
    threshold: f64,
    recorder: &Arc<Mutex<TraceRecorder>>,
    captured_prefix: &mut Option<Trace>,
) -> MonitorStatus {
    while sim.peek_next_event_time().is_some() && sim.time() < end_time {
        sim.step();
        let now = sim.time();

        let status = {
            let mut m = monitor.lock().unwrap();
            m.flush_up_to(now);
            m.status()
        };

        if status.metastable {
            return status;
        }

        if captured_prefix.is_none() && status.score >= threshold {
            *captured_prefix = Some(recorder.lock().unwrap().snapshot());
            // We deliberately keep running until end_time so that a single run
            // can also directly discover metastability.
        }
    }

    // Final flush.
    let mut m = monitor.lock().unwrap();
    m.flush_up_to(end_time);
    m.status()
}

/// Find a trace that triggers a terminal predicate (e.g., metastability) using
/// multilevel splitting.
///
/// This implementation uses rerun+replay:
/// - to branch at a checkpoint, we record a trace prefix up to the first time
///   the score crosses the level threshold;
/// - continuation particles replay that prefix and then draw fresh randomness.
///
/// `setup` must build the simulation and wire a randomness provider.
///
/// For multi-distribution models, prefer `ctx.shared_branching_provider(prefix, continuation_seed)`
/// so that all randomness draws are consumed from a single globally-ordered stream.
pub fn find_with_splitting(
    cfg: SplittingConfig,
    base_sim_config: SimulationConfig,
    scenario: String,
    setup: impl Fn(
            SimulationConfig,
            &HarnessContext,
            Option<&Trace>,
            u64,
        ) -> (Simulation, Arc<Mutex<Monitor>>)
        + Copy,
) -> Result<Option<FoundBug>, SplittingError> {
    let mut particles = vec![Particle {
        prefix: None,
        continuation_seed: base_sim_config.seed,
    }];

    for threshold in cfg.levels.iter().copied() {
        let mut next_level: Vec<Particle> = Vec::new();

        for (p_index, p) in particles.iter().enumerate() {
            let recorder = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
                seed: base_sim_config.seed,
                scenario: scenario.clone(),
            })));

            let ctx = HarnessContext::new(recorder.clone());

            let (mut sim, monitor) = setup(
                base_sim_config.clone(),
                &ctx,
                p.prefix.as_ref(),
                p.continuation_seed,
            );

            if cfg.install_tokio {
                #[cfg(feature = "tokio")]
                {
                    des_tokio::runtime::install(&mut sim);
                }

                #[cfg(not(feature = "tokio"))]
                {
                    return Err(SplittingError::Harness(HarnessError::TokioFeatureDisabled));
                }
            }

            let mut prefix_at_threshold: Option<Trace> = None;
            let status = run_until_threshold_or_end(
                &mut sim,
                &monitor,
                cfg.end_time,
                threshold,
                &recorder,
                &mut prefix_at_threshold,
            );

            if status.metastable {
                let trace = recorder.lock().unwrap().snapshot();
                return Ok(Some(FoundBug { trace, status }));
            }

            let Some(prefix) = prefix_at_threshold else {
                continue;
            };

            // Spawn continuation particles for the next level.
            for i in 0..cfg.branch_factor {
                if next_level.len() >= cfg.max_particles_per_level {
                    break;
                }

                // Derive a deterministic but distinct continuation seed.
                let seed = p.continuation_seed
                    ^ ((threshold.to_bits().wrapping_mul(0x9E37_79B9)) as u64)
                    ^ ((p_index as u64) << 32)
                    ^ (i as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);

                next_level.push(Particle {
                    prefix: Some(prefix.clone()),
                    continuation_seed: seed,
                });
            }
        }

        if next_level.is_empty() {
            // No one reached this level; stop early.
            return Ok(None);
        }

        particles = next_level;
    }

    Ok(None)
}
