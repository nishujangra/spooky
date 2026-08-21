use clap::Parser;
use impulse_bench::{
    benchmark::{macro_bench::run_macro_suite, micro_bench::run_micro_suite},
    cli::{Args, BenchSuite, FailOn},
    io::{load_release_index, load_report, merge_reports, resolve_baseline_paths, write_report},
    manifest::load_manifest,
    markdown::write_markdown,
    promotion::run_promotion,
    regression::{RegressionSeverity, compare_reports, format_issue, resolve_gate_config},
    report::BenchReport,
    utils::{print_summary, suite_label, unix_now},
};

fn main() -> Result<(), String> {
    let args = Args::parse();

    if args.promote_release.is_some() {
        return run_promotion(&args);
    }

    let manifest = load_manifest(&args.manifest)?;
    let profile = manifest
        .profiles
        .get(&args.profile)
        .ok_or_else(|| format!("profile '{}' missing in manifest", args.profile))?;

    let mut cases = Vec::new();
    match args.suite {
        BenchSuite::Micro => {
            cases.extend(run_micro_suite(profile, &manifest.micro)?);
        }
        BenchSuite::Macro => {
            cases.extend(run_macro_suite(profile, &manifest.macro_suite)?);
        }
        BenchSuite::All => {
            cases.extend(run_micro_suite(profile, &manifest.micro)?);
            cases.extend(run_macro_suite(profile, &manifest.macro_suite)?);
        }
    }

    cases.sort_by(|left, right| {
        (&left.kind, &left.name, left.scale).cmp(&(&right.kind, &right.name, right.scale))
    });

    let report = BenchReport {
        suite: "impulse-performance-regression".to_string(),
        report_kind: suite_label(args.suite).to_string(),
        profile: args.profile.clone(),
        generated_unix_secs: unix_now(),
        cpu_threshold: args.cpu_threshold.unwrap_or(manifest.gates.cpu.warn_pct),
        mem_threshold: args.mem_threshold.unwrap_or(manifest.gates.memory.warn_pct),
        cases,
    };

    print_summary(&report);
    write_report(&args.output, &report)?;

    let mut issues = Vec::new();
    if args.check_baseline {
        let release_index = load_release_index(&args.baseline_index)?;
        let baseline_paths = resolve_baseline_paths(&args, &release_index)?;
        let mut baseline_reports = Vec::with_capacity(baseline_paths.len());
        for path in &baseline_paths {
            baseline_reports.push(load_report(path)?);
        }
        let baseline = merge_reports(baseline_reports);

        let gates = resolve_gate_config(manifest.gates.clone(), &args);
        issues = compare_reports(&report, &baseline, &gates);
    }

    if let Some(markdown) = &args.markdown_out {
        write_markdown(markdown, &report, &issues, args.fail_on)?;
    }

    if !issues.is_empty() {
        let severe_count = issues
            .iter()
            .filter(|issue| issue.severity == RegressionSeverity::Severe)
            .count();
        let warn_count = issues.len().saturating_sub(severe_count);

        for issue in &issues {
            eprintln!("{}", format_issue(issue));
        }

        let fail = match args.fail_on {
            FailOn::Severe => severe_count > 0,
            FailOn::Any => !issues.is_empty(),
        };

        if fail {
            return Err(format!(
                "benchmark regression gate failed (severe={severe_count}, warn={warn_count}, mode={:?})",
                args.fail_on
            ));
        }
    }

    Ok(())
}
