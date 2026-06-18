# Benchmarking

Spooky includes a dedicated benchmark crate and helper scripts for repeatable performance work.

## Components

- `crates/bench/` provides the benchmark CLI and report generation
- `scripts/bench-micro.sh` runs the micro suite
- `scripts/bench-macro.sh` runs the macro suite
- `scripts/bench-gate.sh` compares current runs to baseline reports
- `scripts/bench-promote-baseline.sh` promotes a report into the stored baseline set

## What The Benchmark Suite Covers

- route lookup behavior
- load-balancer selection behavior
- connection lookup behavior
- header collection behavior
- macro traffic-mix workloads
- long-lived stream workload models

## Why It Matters

This project has a large hot path in the edge runtime. Benchmarking is part of the quality bar for changes that affect:

- routing
- connection lookup
- balancing
- buffering
- stream lifecycle

## Operational Use

- use micro benchmarks for algorithmic regressions
- use macro benchmarks for end-to-end hot-path behavior
- use baseline gating for release confidence rather than one-off headline numbers
