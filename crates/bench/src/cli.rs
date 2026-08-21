use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Impulse benchmark suite (micro + macro + regression gates)"
)]
pub struct Args {
    #[arg(long, default_value = "bench/latest.json")]
    pub output: PathBuf,

    #[arg(long)]
    pub markdown_out: Option<PathBuf>,

    #[arg(long)]
    pub baseline: Option<PathBuf>,

    #[arg(long, default_value_t = false)]
    pub check_baseline: bool,

    #[arg(long, value_enum, default_value_t = BenchSuite::Micro)]
    pub suite: BenchSuite,

    #[arg(long, default_value = "full")]
    pub profile: String,

    #[arg(long, default_value = "bench/manifest.yaml")]
    pub manifest: PathBuf,

    #[arg(long, default_value = "bench/baselines/releases.json")]
    pub baseline_index: PathBuf,

    #[arg(long)]
    pub baseline_release: Option<String>,

    #[arg(long, value_enum, default_value_t = FailOn::Severe)]
    pub fail_on: FailOn,

    #[arg(long)]
    pub cpu_threshold: Option<f64>,

    #[arg(long)]
    pub mem_threshold: Option<f64>,

    #[arg(long)]
    pub promote_release: Option<String>,

    #[arg(long, default_value = "bench/latest.json")]
    pub promote_micro_report: PathBuf,

    #[arg(long, default_value = "bench/macro/latest.json")]
    pub promote_macro_report: PathBuf,

    #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
    pub set_current_release: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum BenchSuite {
    Micro,
    Macro,
    All,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum FailOn {
    Severe,
    Any,
}
