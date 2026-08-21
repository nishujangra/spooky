use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    cli::{Args, BenchSuite},
    report::{BenchReport, ReleaseBaselineIndex},
    utils::unix_now,
};

pub fn write_report(path: &Path, report: &BenchReport) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create output dir '{}': {err}", parent.display()))?;
    }
    let json =
        serde_json::to_string_pretty(report).map_err(|err| format!("serialize report: {err}"))?;
    fs::write(path, json)
        .map_err(|err| format!("failed to write report '{}': {err}", path.display()))
}

pub fn load_report(path: &Path) -> Result<BenchReport, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("failed to read baseline '{}': {err}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|err| format!("failed to parse baseline '{}': {err}", path.display()))
}

pub fn load_release_index(path: &Path) -> Result<ReleaseBaselineIndex, String> {
    if !path.exists() {
        return Ok(ReleaseBaselineIndex::default());
    }
    let text = fs::read_to_string(path)
        .map_err(|err| format!("failed to read baseline index '{}': {err}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|err| format!("failed to parse baseline index '{}': {err}", path.display()))
}

pub fn write_release_index(path: &Path, index: &ReleaseBaselineIndex) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create baseline index dir '{}': {err}",
                parent.display()
            )
        })?;
    }
    let text = serde_json::to_string_pretty(index)
        .map_err(|err| format!("failed to serialize baseline index: {err}"))?;
    fs::write(path, text)
        .map_err(|err| format!("failed to write baseline index '{}': {err}", path.display()))
}

pub fn merge_reports(reports: Vec<BenchReport>) -> BenchReport {
    let mut merged = BenchReport {
        suite: "impulse-performance-baseline".to_string(),
        report_kind: "merged".to_string(),
        profile: "baseline".to_string(),
        generated_unix_secs: unix_now(),
        ..BenchReport::default()
    };

    for report in reports {
        merged.cases.extend(report.cases);
    }

    merged.cases.sort_by(|left, right| {
        (&left.kind, &left.name, left.scale).cmp(&(&right.kind, &right.name, right.scale))
    });
    merged
}

pub fn resolve_baseline_paths(
    args: &Args,
    release_index: &ReleaseBaselineIndex,
) -> Result<Vec<PathBuf>, String> {
    if let Some(path) = &args.baseline {
        return Ok(vec![path.clone()]);
    }

    let release = args
        .baseline_release
        .clone()
        .or_else(|| {
            (!release_index.current_release.is_empty()).then_some(release_index.current_release.clone())
        })
        .ok_or_else(|| {
            "baseline not specified; pass --baseline or configure --baseline-release / current_release in baseline index".to_string()
        })?;

    let entry = release_index
        .releases
        .get(&release)
        .ok_or_else(|| format!("release '{release}' missing from baseline index"))?;

    let paths = match args.suite {
        BenchSuite::Micro => vec![PathBuf::from(&entry.micro)],
        BenchSuite::Macro => vec![PathBuf::from(&entry.macro_report)],
        BenchSuite::All => vec![
            PathBuf::from(&entry.micro),
            PathBuf::from(&entry.macro_report),
        ],
    };

    Ok(paths)
}
