use impulse_edge::benchmark::connection_lookup::ConnectionLookupBench;

use crate::{benchmark::runner::run_case_aggregate, report::BenchCase};

pub fn benchmark_connection_lookup(scale: usize) -> Vec<BenchCase> {
    let bench = ConnectionLookupBench::new(scale);
    assert!(bench.peer_map_hit() > 0);

    vec![
        run_case_aggregate(
            "micro",
            "connection_exact_lookup",
            scale,
            fast_iterations(scale),
            || bench.exact_lookup(),
        ),
        run_case_aggregate(
            "micro",
            "connection_alias_lookup",
            scale,
            fast_iterations(scale),
            || bench.alias_lookup(),
        ),
        run_case_aggregate(
            "micro",
            "connection_prefix_scan_miss_lookup",
            scale,
            scan_iterations(scale),
            || bench.prefix_scan_miss_lookup(),
        ),
        run_case_aggregate(
            "micro",
            "connection_peer_scan_miss",
            scale,
            scan_iterations(scale),
            || bench.peer_scan_miss(),
        ),
        run_case_aggregate(
            "micro",
            "connection_peer_map_hit",
            scale,
            fast_iterations(scale),
            || bench.peer_map_hit(),
        ),
        run_case_aggregate(
            "micro",
            "connection_peer_map_miss",
            scale,
            fast_iterations(scale),
            || bench.peer_map_miss(),
        ),
    ]
}

pub fn fast_iterations(scale: usize) -> u64 {
    match scale {
        100 => 300_000,
        1_000 => 200_000,
        10_000 => 80_000,
        _ => 50_000,
    }
}

fn scan_iterations(scale: usize) -> u64 {
    match scale {
        100 => 150_000,
        1_000 => 30_000,
        10_000 => 3_000,
        _ => 10_000,
    }
}
