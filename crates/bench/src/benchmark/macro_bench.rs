use impulse_edge::benchmark::{
    connection_lookup::ConnectionLookupBench, route_lookup::RouteLookupBench,
};
use impulse_lb::upstream_pool::UpstreamPool;

use crate::{
    benchmark::{headers::synth_h3_headers, lb::build_lb_pool, runner::run_case_with_latencies},
    manifest::{BenchProfile, MacroSuiteConfig},
    report::BenchCase,
};

fn benchmark_macro_traffic_mix(
    scale: usize,
    iterations: u64,
    mut rr_pool: UpstreamPool,
    mut random_pool: UpstreamPool,
    mut ch_pool: UpstreamPool,
) -> BenchCase {
    let route = RouteLookupBench::new(scale);
    let conn = ConnectionLookupBench::new(scale);
    let mut traffic_counter = 0usize;

    let mut headers_small = synth_h3_headers(8);
    let mut headers_large = synth_h3_headers(24);

    run_case_with_latencies("macro", "macro_traffic_mix", scale, iterations, move || {
        let bucket = traffic_counter % 100;
        traffic_counter = traffic_counter.wrapping_add(1);

        if bucket < 55 {
            let route_hit = route.indexed_hit();
            let lb = rr_pool.pick("mix-core").unwrap_or(0);
            let conn_hit = conn.peer_map_hit();
            route_hit ^ lb ^ conn_hit
        } else if bucket < 75 {
            let route_miss = route.indexed_miss();
            let lb = random_pool.pick("mix-random").unwrap_or(0);
            let conn_miss = conn.peer_map_miss();
            route_miss ^ lb ^ conn_miss
        } else if bucket < 90 {
            let route_hit = route.linear_hit();
            let key = if bucket & 1 == 0 {
                "tenant:a"
            } else {
                "tenant:b"
            };
            let lb = ch_pool.pick(key).unwrap_or(0);
            let alias = conn.alias_lookup();
            route_hit ^ lb ^ alias
        } else {
            let mut collected = 0usize;
            if bucket & 1 == 0 {
                headers_small.rotate_left(1);
                for (name, value) in &headers_small {
                    collected ^= name.len() ^ value.len();
                }
            } else {
                headers_large.rotate_left(1);
                for (name, value) in &headers_large {
                    collected ^= name.len() ^ value.len();
                }
            }
            collected ^ conn.prefix_scan_miss_lookup()
        }
    })
}

fn benchmark_macro_long_lived_stream(
    scale: usize,
    iterations: u64,
    chunks_per_stream: usize,
    chunk_bytes: usize,
    mut ch_pool: UpstreamPool,
) -> BenchCase {
    let route = RouteLookupBench::new(scale);
    let conn = ConnectionLookupBench::new(scale);

    let payload = vec![0xAB_u8; chunk_bytes.max(1)];
    let chunks = chunks_per_stream.max(1);
    let mut stream_counter = 0usize;

    run_case_with_latencies(
        "macro",
        "macro_long_lived_stream",
        scale,
        iterations,
        move || {
            let stream_id = stream_counter;
            stream_counter = stream_counter.wrapping_add(1);

            let mut checksum = route.indexed_hit() ^ conn.peer_map_hit();
            let lb = ch_pool
                .pick(if stream_id & 1 == 0 {
                    "stream-even"
                } else {
                    "stream-odd"
                })
                .unwrap_or(0);
            checksum ^= lb;

            for chunk_idx in 0..chunks {
                let offset = (chunk_idx * 31 + stream_id) % payload.len();
                checksum ^= payload[offset] as usize;
                if chunk_idx % 8 == 0 {
                    checksum ^= route.indexed_hit();
                }
                if chunk_idx % 11 == 0 {
                    checksum ^= conn.alias_lookup();
                }
            }

            checksum
        },
    )
}

fn effective_macro_scales(profile: &BenchProfile) -> Vec<usize> {
    if !profile.macro_scales.is_empty() {
        return profile.macro_scales.clone();
    }
    if let Some(first) = profile.scales.first() {
        return vec![*first];
    }
    vec![100]
}

pub fn run_macro_suite(
    profile: &BenchProfile,
    config: &MacroSuiteConfig,
) -> Result<Vec<BenchCase>, String> {
    let mut cases = Vec::new();
    for scale in effective_macro_scales(profile) {
        if config.include_traffic_mix {
            let rr = build_lb_pool(scale, "round-robin")?;
            let random = build_lb_pool(scale, "random")?;
            let ch = build_lb_pool(scale, "consistent-hash")?;
            cases.push(benchmark_macro_traffic_mix(
                scale,
                profile.macro_traffic_mix_iterations,
                rr,
                random,
                ch,
            ));
        }

        if config.include_long_lived_stream {
            let ch = build_lb_pool(scale, "consistent-hash")?;
            cases.push(benchmark_macro_long_lived_stream(
                scale,
                profile.macro_long_lived_stream_iterations,
                profile.macro_long_lived_stream_chunks,
                profile.macro_long_lived_stream_chunk_bytes,
                ch,
            ));
        }
    }
    Ok(cases)
}
