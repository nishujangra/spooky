use std::{fs, path::Path};

use crate::{
    cli::FailOn,
    regression::{RegressionIssue, RegressionSeverity, format_issue},
    report::BenchReport,
};

pub fn write_markdown(
    path: &Path,
    report: &BenchReport,
    issues: &[RegressionIssue],
    fail_on: FailOn,
) -> Result<(), String> {
    let mut lines = vec![
        "# Impulse Benchmark Report".to_string(),
        "".to_string(),
        format!("- Report kind: `{}`", report.report_kind),
        format!("- Profile: `{}`", report.profile),
        "".to_string(),
        "| kind | case | scale | ns/op | cpu% | ops/s | p50(ns) | p95(ns) | p99(ns) | alloc_calls | alloc_bytes | rss_delta_kb |".to_string(),
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |".to_string(),
    ];

    for case in &report.cases {
        lines.push(format!(
            "| {} | {} | {} | {:.2} | {:.2} | {:.2} | {:.0} | {:.0} | {:.0} | {} | {} | {} |",
            case.kind,
            case.name,
            case.scale,
            case.latency_ns_per_op,
            case.cpu_pct,
            case.throughput_ops_per_sec,
            case.latency_p50_ns,
            case.latency_p95_ns,
            case.latency_p99_ns,
            case.alloc_calls,
            case.alloc_bytes,
            case.rss_delta_kb
        ));
    }

    lines.push("".to_string());
    if issues.is_empty() {
        lines.push("No regressions detected against baseline.".to_string());
    } else {
        lines.push("## Regression Findings".to_string());
        lines.push(format!("- Fail mode: `{:?}`", fail_on));
        lines.push("".to_string());

        let mut severe = issues
            .iter()
            .filter(|issue| issue.severity == RegressionSeverity::Severe)
            .collect::<Vec<_>>();
        let mut warn = issues
            .iter()
            .filter(|issue| issue.severity == RegressionSeverity::Warn)
            .collect::<Vec<_>>();

        severe.sort_by_key(|issue| (&issue.kind, &issue.case, issue.scale, issue.metric));
        warn.sort_by_key(|issue| (&issue.kind, &issue.case, issue.scale, issue.metric));

        if !severe.is_empty() {
            lines.push("### Severe".to_string());
            for issue in severe {
                lines.push(format!("- {}", format_issue(issue)));
            }
            lines.push("".to_string());
        }

        if !warn.is_empty() {
            lines.push("### Warn".to_string());
            for issue in warn {
                lines.push(format!("- {}", format_issue(issue)));
            }
        }
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create markdown dir '{}': {err}",
                parent.display()
            )
        })?;
    }
    fs::write(path, lines.join("\n"))
        .map_err(|err| format!("failed to write markdown '{}': {err}", path.display()))
}
