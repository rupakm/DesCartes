use std::sync::{Arc, Mutex};

use descartes_core::{SimTime, Simulation, SimulationConfig};

use crate::harness::{HarnessContext, HarnessError};
use crate::monitor::{Monitor, MonitorStatus};
use crate::stats::{wilson_interval, z_for_confidence};
use crate::trace::{Trace, TraceMeta, TraceRecorder};

#[derive(Debug, Clone)]
pub struct BernoulliEstimate {
    pub trials: u64,
    pub successes: u64,
    pub p_hat: f64,
    pub ci_low: Option<f64>,
    pub ci_high: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct MonteCarloConfig {
    pub trials: u64,
    pub end_time: SimTime,
    pub install_tokio: bool,
    pub confidence: f64,
}

impl Default for MonteCarloConfig {
    fn default() -> Self {
        Self {
            trials: 1_000,
            end_time: SimTime::from_secs(60),
            install_tokio: true,
            confidence: 0.95,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SplittingEstimate {
    /// Estimated probability of satisfying the terminal predicate.
    pub p_hat: f64,

    /// Approximate confidence interval low bound.
    pub ci_low: Option<f64>,

    /// Approximate confidence interval high bound.
    pub ci_high: Option<f64>,

    /// Total particles per level.
    pub particles: usize,

    /// Success counts for each level, plus the terminal stage as the last entry.
    pub stage_successes: Vec<usize>,

    /// Conditional success probabilities per stage.
    pub stage_probs: Vec<f64>,

    /// One example counterexample trace when a success occurs.
    pub example: Option<FoundCounterexample>,
}

#[derive(Debug, Clone)]
pub struct FoundCounterexample {
    pub trace: Trace,
    pub status: MonitorStatus,
}

#[derive(Debug, Clone)]
pub struct SplittingEstimateConfig {
    /// Score thresholds in increasing order.
    pub levels: Vec<f64>,

    /// Number of particles per stage.
    pub particles: usize,

    /// Simulation end time (hard horizon for each run).
    pub end_time: SimTime,

    /// Whether to install the `des-tokio` runtime.
    pub install_tokio: bool,

    /// Confidence level used for the (approximate) log-normal CI.
    pub confidence: f64,
}

impl Default for SplittingEstimateConfig {
    fn default() -> Self {
        Self {
            levels: vec![50.0, 150.0, 300.0],
            particles: 64,
            end_time: SimTime::from_secs(60),
            install_tokio: true,
            confidence: 0.95,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EstimateError {
    #[error(transparent)]
    Harness(#[from] HarnessError),

    #[error("trials must be > 0")]
    ZeroTrials,

    #[error("splitting requires particles >= 1")]
    ZeroParticles,
}

#[derive(Clone)]
struct Particle {
    prefix: Option<Trace>,
    seed: u64,
}

fn derive_seed(base: u64, i: u64) -> u64 {
    // SplitMix64.
    let mut x = base.wrapping_add(i.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn install_tokio_if_requested(
    sim: &mut Simulation,
    install_tokio: bool,
) -> Result<(), HarnessError> {
    if !install_tokio {
        return Ok(());
    }

    #[cfg(feature = "tokio")]
    {
        descartes_tokio::runtime::install(sim);
        Ok(())
    }

    #[cfg(not(feature = "tokio"))]
    {
        let _ = sim;
        Err(HarnessError::TokioFeatureDisabled)
    }
}

fn run_until_end_or(
    sim: &mut Simulation,
    monitor: &Arc<Mutex<Monitor>>,
    end_time: SimTime,
    mut stop: impl FnMut(&MonitorStatus) -> bool,
) -> MonitorStatus {
    while sim.peek_next_event_time().is_some() && sim.time() < end_time {
        sim.step();
        let now = sim.time();
        let status = {
            let mut m = monitor.lock().unwrap();
            m.flush_up_to(now);
            m.status()
        };

        if stop(&status) {
            return status;
        }
    }

    let mut m = monitor.lock().unwrap();
    m.flush_up_to(end_time);
    m.status()
}

/// Naïve Monte Carlo estimate for a terminal predicate.
///
/// `setup` must build the simulation and wire any trace-aware randomness provider.
///
/// The predicate is evaluated on the evolving [`MonitorStatus`] and may short-circuit.
pub fn estimate_monte_carlo(
    cfg: MonteCarloConfig,
    base_sim_config: SimulationConfig,
    scenario: String,
    setup: impl Fn(
            SimulationConfig,
            &HarnessContext,
            Option<&Trace>,
            u64,
        ) -> (Simulation, Arc<Mutex<Monitor>>)
        + Copy,
    predicate: impl Fn(&MonitorStatus) -> bool,
) -> Result<BernoulliEstimate, EstimateError> {
    if cfg.trials == 0 {
        return Err(EstimateError::ZeroTrials);
    }

    let mut successes = 0u64;

    for i in 0..cfg.trials {
        let seed = derive_seed(base_sim_config.seed, i);
        let sim_config = SimulationConfig { seed };

        let recorder = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
            seed,
            scenario: scenario.clone(),
        })));
        let ctx = HarnessContext::new(recorder);

        let (mut sim, monitor) = setup(sim_config, &ctx, None, seed);
        install_tokio_if_requested(&mut sim, cfg.install_tokio)?;

        let status = run_until_end_or(&mut sim, &monitor, cfg.end_time, &predicate);

        if predicate(&status) {
            successes += 1;
        }
    }

    let p_hat = successes as f64 / cfg.trials as f64;
    let ci = wilson_interval(successes, cfg.trials, cfg.confidence);

    Ok(BernoulliEstimate {
        trials: cfg.trials,
        successes,
        p_hat,
        ci_low: ci.map(|(l, _)| l),
        ci_high: ci.map(|(_, h)| h),
    })
}

/// Multilevel splitting estimate for a terminal predicate.
///
/// This is intended for *rare* terminal properties.
///
/// - Each stage `i` uses a score threshold `levels[i]`.
/// - Particles that reach the threshold are checkpointed by storing a trace prefix.
/// - The next stage resamples successful prefixes (with replacement) and continues.
///
/// The returned confidence interval is an approximate log-normal CI using the usual
/// binomial-stage variance approximation.
pub fn estimate_with_splitting(
    cfg: SplittingEstimateConfig,
    base_sim_config: SimulationConfig,
    scenario: String,
    setup: impl Fn(
            SimulationConfig,
            &HarnessContext,
            Option<&Trace>,
            u64,
        ) -> (Simulation, Arc<Mutex<Monitor>>)
        + Copy,
    terminal: impl Fn(&MonitorStatus) -> bool,
) -> Result<SplittingEstimate, EstimateError> {
    if cfg.particles == 0 {
        return Err(EstimateError::ZeroParticles);
    }

    let mut particles: Vec<Particle> = (0..cfg.particles)
        .map(|i| {
            let seed = derive_seed(base_sim_config.seed, i as u64);
            Particle { prefix: None, seed }
        })
        .collect();

    let mut stage_successes: Vec<usize> = Vec::with_capacity(cfg.levels.len() + 1);
    let mut stage_probs: Vec<f64> = Vec::with_capacity(cfg.levels.len() + 1);

    let mut example: Option<FoundCounterexample> = None;

    for (level_index, &threshold) in cfg.levels.iter().enumerate() {
        let mut successful: Vec<Trace> = Vec::new();

        for p in particles.iter() {
            let recorder = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
                seed: p.seed,
                scenario: scenario.clone(),
            })));
            let ctx = HarnessContext::new(recorder.clone());

            let sim_config = SimulationConfig { seed: p.seed };
            let (mut sim, monitor) = setup(sim_config, &ctx, p.prefix.as_ref(), p.seed);
            install_tokio_if_requested(&mut sim, cfg.install_tokio)?;

            let mut prefix_at_threshold: Option<Trace> = None;
            let status = run_until_end_or(&mut sim, &monitor, cfg.end_time, |status| {
                if terminal(status) {
                    return true;
                }

                if prefix_at_threshold.is_none() && status.score >= threshold {
                    prefix_at_threshold = Some(recorder.lock().unwrap().snapshot());
                }

                false
            });

            if terminal(&status) && example.is_none() {
                example = Some(FoundCounterexample {
                    trace: recorder.lock().unwrap().snapshot(),
                    status: status.clone(),
                });
            }

            if let Some(prefix) = prefix_at_threshold {
                successful.push(prefix);
            }
        }

        stage_successes.push(successful.len());
        let p_i = successful.len() as f64 / cfg.particles as f64;
        stage_probs.push(p_i);

        if successful.is_empty() {
            return Ok(SplittingEstimate {
                p_hat: 0.0,
                ci_low: Some(0.0),
                ci_high: Some(0.0),
                particles: cfg.particles,
                stage_successes,
                stage_probs,
                example,
            });
        }

        // Resample N particles for the next level.
        let mut new_particles = Vec::with_capacity(cfg.particles);
        for j in 0..cfg.particles {
            let pick = derive_seed(base_sim_config.seed ^ (level_index as u64), j as u64) as usize
                % successful.len();
            let seed = derive_seed(
                base_sim_config.seed ^ 0xD1B5_4A32_D192_ED03,
                (level_index as u64) << 32 | j as u64,
            );
            new_particles.push(Particle {
                prefix: Some(successful[pick].clone()),
                seed,
            });
        }

        particles = new_particles;
    }

    // Terminal stage: run from last-level prefixes and check the terminal predicate.
    let mut terminal_successes = 0usize;

    for p in particles.iter() {
        let recorder = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
            seed: p.seed,
            scenario: scenario.clone(),
        })));
        let ctx = HarnessContext::new(recorder.clone());

        let sim_config = SimulationConfig { seed: p.seed };
        let (mut sim, monitor) = setup(sim_config, &ctx, p.prefix.as_ref(), p.seed);
        install_tokio_if_requested(&mut sim, cfg.install_tokio)?;

        let status = run_until_end_or(&mut sim, &monitor, cfg.end_time, &terminal);
        if terminal(&status) {
            terminal_successes += 1;
            if example.is_none() {
                example = Some(FoundCounterexample {
                    trace: recorder.lock().unwrap().snapshot(),
                    status,
                });
            }
        }
    }

    stage_successes.push(terminal_successes);
    let p_last = terminal_successes as f64 / cfg.particles as f64;
    stage_probs.push(p_last);

    let p_hat = stage_probs.iter().product::<f64>();

    let (ci_low, ci_high) = splitting_log_ci(&stage_probs, cfg.particles, cfg.confidence)
        .map(|(lo, hi)| (Some(lo), Some(hi)))
        .unwrap_or((None, None));

    Ok(SplittingEstimate {
        p_hat,
        ci_low,
        ci_high,
        particles: cfg.particles,
        stage_successes,
        stage_probs,
        example,
    })
}

fn splitting_log_ci(stage_probs: &[f64], particles: usize, confidence: f64) -> Option<(f64, f64)> {
    if stage_probs.iter().any(|&p| p <= 0.0) {
        return Some((0.0, 0.0));
    }

    let n = particles as f64;

    // Approximate Var(log p̂) ≈ Σ (1 - p_i) / (n * p_i)
    let var_log: f64 = stage_probs.iter().map(|&p| (1.0 - p) / (n * p)).sum();

    let se_log = var_log.sqrt();
    let z = z_for_confidence(confidence);

    let log_p = stage_probs.iter().map(|&p| p.ln()).sum::<f64>();
    let lo = (log_p - z * se_log).exp();
    let hi = (log_p + z * se_log).exp();
    Some((lo.clamp(0.0, 1.0), hi.clamp(0.0, 1.0)))
}
