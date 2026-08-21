use impulse_edge::benchmark::route_lookup::RouteLookupBench;

use crate::{benchmark::runner::run_case_aggregate, report::BenchCase};

pub fn benchmark_route_lookup(scale: usize) -> Vec<BenchCase> {
    let bench = RouteLookupBench::new(scale);
    assert_eq!(bench.indexed_hit() > 0, bench.linear_hit() > 0);

    vec![
        run_case_aggregate(
            "micro",
            "route_lookup_indexed_hit",
            scale,
            route_iterations(scale, false),
            || bench.indexed_hit(),
        ),
        run_case_aggregate(
            "micro",
            "route_lookup_linear_hit",
            scale,
            route_iterations(scale, true),
            || bench.linear_hit(),
        ),
        run_case_aggregate(
            "micro",
            "route_lookup_indexed_miss",
            scale,
            route_iterations(scale, false),
            || bench.indexed_miss(),
        ),
    ]
}

fn route_iterations(scale: usize, linear: bool) -> u64 {
    match (scale, linear) {
        (100, false) => 300_000,
        (1_000, false) => 200_000,
        (10_000, false) => 100_000,
        (100, true) => 200_000,
        (1_000, true) => 40_000,
        (10_000, true) => 4_000,
        _ => 20_000,
    }
}
