/// Inverse CDF (quantile) of the standard normal distribution.
///
/// Uses the Peter J. Acklam rational approximation.
///
/// # Panics
/// Panics if `p` is not in `(0, 1)`.
pub fn inv_norm_cdf(p: f64) -> f64 {
    assert!((0.0..1.0).contains(&p), "p must be in (0, 1)");

    // Coefficients in rational approximations.
    // See: https://web.archive.org/web/20150910044729/http://home.online.no/~pjacklam/notes/invnorm/
    const A: [f64; 6] = [
        -3.969_683_028_665_376e+01,
        2.209_460_984_245_205e+02,
        -2.759_285_104_469_687e+02,
        1.383_577_518_672_690e+02,
        -3.066_479_806_614_716e+01,
        2.506_628_277_459_239e+00,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e+01,
        1.615_858_368_580_409e+02,
        -1.556_989_798_598_866e+02,
        6.680_131_188_771_972e+01,
        -1.328_068_155_288_572e+01,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-03,
        -3.223_964_580_411_365e-01,
        -2.400_758_277_161_838e+00,
        -2.549_732_539_343_734e+00,
        4.374_664_141_464_968e+00,
        2.938_163_982_698_783e+00,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-03,
        3.224_671_290_700_398e-01,
        2.445_134_137_142_996e+00,
        3.754_408_661_907_416e+00,
    ];

    const P_LOW: f64 = 0.02425;
    const P_HIGH: f64 = 1.0 - P_LOW;

    if p < P_LOW {
        // Rational approximation for lower region.
        let q = (-2.0 * p.ln()).sqrt();
        let num = ((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5];
        let den = (((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0;
        -num / den
    } else if p > P_HIGH {
        // Rational approximation for upper region.
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        let num = ((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5];
        let den = (((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0;
        num / den
    } else {
        // Rational approximation for central region.
        let q = p - 0.5;
        let r = q * q;
        let num = (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q;
        let den = ((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0;
        num / den
    }
}

/// z-value for a symmetric confidence interval under a normal approximation.
///
/// For example, `confidence = 0.95` returns ~1.96.
pub fn z_for_confidence(confidence: f64) -> f64 {
    assert!(
        (0.0..1.0).contains(&confidence),
        "confidence must be in (0, 1)"
    );
    let p = 0.5 + confidence / 2.0;
    inv_norm_cdf(p)
}

/// Wilson score interval for a Bernoulli proportion.
///
/// Returns `(low, high)`.
pub fn wilson_interval(successes: u64, trials: u64, confidence: f64) -> Option<(f64, f64)> {
    if trials == 0 {
        return None;
    }

    let n = trials as f64;
    let phat = successes as f64 / n;
    let z = z_for_confidence(confidence);
    let z2 = z * z;

    let denom = 1.0 + z2 / n;
    let center = (phat + z2 / (2.0 * n)) / denom;
    let radius = (z / denom) * ((phat * (1.0 - phat) / n) + (z2 / (4.0 * n * n))).sqrt();

    let lo = (center - radius).clamp(0.0, 1.0);
    let hi = (center + radius).clamp(0.0, 1.0);
    Some((lo, hi))
}

#[derive(Debug, Clone)]
pub struct BatchMeansEstimate {
    pub batches: usize,
    pub batch_size: usize,
    pub mean: f64,
    pub std_err: f64,
    pub ci_low: f64,
    pub ci_high: f64,
}

/// Batch means estimator for a steady-state mean with an approximate normal CI.
///
/// `batches` should be reasonably large (>= 10) for the approximation to behave.
pub fn batch_means(samples: &[f64], batches: usize, confidence: f64) -> Option<BatchMeansEstimate> {
    if batches == 0 {
        return None;
    }

    let batch_size = samples.len() / batches;
    if batch_size == 0 {
        return None;
    }

    let mut batch_means = Vec::with_capacity(batches);
    for b in 0..batches {
        let start = b * batch_size;
        let end = start + batch_size;
        if end > samples.len() {
            break;
        }
        let m = mean(&samples[start..end])?;
        batch_means.push(m);
    }

    let k = batch_means.len();
    if k < 2 {
        return None;
    }

    let overall_mean = mean(&batch_means)?;
    let var = sample_variance(&batch_means)?;

    let std_err = (var / k as f64).sqrt();
    let z = z_for_confidence(confidence);
    let ci_low = overall_mean - z * std_err;
    let ci_high = overall_mean + z * std_err;

    Some(BatchMeansEstimate {
        batches: k,
        batch_size,
        mean: overall_mean,
        std_err,
        ci_low,
        ci_high,
    })
}

pub fn mean(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    Some(xs.iter().sum::<f64>() / xs.len() as f64)
}

pub fn sample_variance(xs: &[f64]) -> Option<f64> {
    if xs.len() < 2 {
        return None;
    }
    let m = mean(xs)?;
    let mut acc = 0.0;
    for &x in xs {
        let d = x - m;
        acc += d * d;
    }
    Some(acc / (xs.len() as f64 - 1.0))
}

/// Convenience for converting a probability estimate into an information-theoretic
/// surprise (aka self-information) in nats.
pub fn surprisal_nats(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::INFINITY;
    }
    -p.ln()
}

/// Convenience for converting a probability estimate into bits.
pub fn surprisal_bits(p: f64) -> f64 {
    // -log2(p) = -ln(p) / ln(2)
    surprisal_nats(p) / std::f64::consts::LN_2
}
