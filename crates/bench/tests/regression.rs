use impulse_bench::{
    manifest::GateMetric,
    regression::{RegressionSeverity, classify_regression},
};

fn gate(
    warn_pct: f64,
    severe_pct: f64,
    zero_baseline_limit: f64,
    min_delta_abs: f64,
) -> GateMetric {
    GateMetric {
        warn_pct,
        severe_pct,
        zero_baseline_limit,
        min_delta_abs,
    }
}

#[test]
fn min_delta_abs_suppresses_small_absolute_memory_drift() {
    let memory_gate = gate(0.20, 0.40, 128.0, 256.0);
    let regression = classify_regression(320.0, 200.0, &memory_gate, 128.0);
    assert!(regression.is_none());
}

#[test]
fn min_delta_abs_still_allows_large_absolute_memory_regressions() {
    let memory_gate = gate(0.20, 0.40, 128.0, 256.0);
    let regression = classify_regression(520.0, 200.0, &memory_gate, 128.0);
    assert!(matches!(
        regression,
        Some((RegressionSeverity::Severe, _, _))
    ));
}
