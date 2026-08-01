use std::{
    fs::{OpenOptions, create_dir_all},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use env_logger::{Builder, Target};
use log::LevelFilter;

use crate::logger::{
    errors::{build_create_log_dir_error, build_open_log_file_error},
    formatter::{build_json_payload, should_passthrough_raw_json_target},
};

static LOGGER_INIT_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
static LOGGER_INITIALIZED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoggerInitStatus {
    Initialized,
    AlreadyInitialized,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogLevelError {
    level: String,
}

impl LogLevelError {
    fn new(level: &str) -> Self {
        Self {
            level: level.to_string(),
        }
    }
}

impl std::fmt::Display for LogLevelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid log level '{}'", self.level)
    }
}

impl std::error::Error for LogLevelError {}

pub fn init_logger(log_level: &str, log_enabled: bool, log_file: &str, json: bool) {
    let _ = try_init_logger(log_level, log_enabled, log_file, json);
}

pub fn try_init_logger(
    log_level: &str,
    log_enabled: bool,
    log_file: &str,
    json: bool,
) -> LoggerInitStatus {
    let mutex = LOGGER_INIT_MUTEX.get_or_init(|| Mutex::new(()));
    let _guard = mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if LOGGER_INITIALIZED.load(Ordering::Acquire) {
        return LoggerInitStatus::AlreadyInitialized;
    }

    let status = configure_and_init_logger(log_level, log_enabled, log_file, json);
    LOGGER_INITIALIZED.store(true, Ordering::Release);
    status
}

fn configure_and_init_logger(
    log_level: &str,
    log_enabled: bool,
    log_file: &str,
    json: bool,
) -> LoggerInitStatus {
    let level = parse_log_level_filter(log_level).unwrap_or_else(|_| {
        eprintln!(
            "Invalid log level '{}', defaulting to 'spooky' (info)",
            log_level
        );
        LevelFilter::Info
    });

    let mut builder = Builder::new();
    // Keep env_logger's internal filter fully open and use log::set_max_level
    // as the single effective runtime gate so live level raises work too.
    builder.filter_level(LevelFilter::Trace);

    builder.format(move |buf, record| {
        if should_passthrough_raw_json_target(record.target()) {
            return writeln!(buf, "{}", record.args());
        }

        if json {
            let message = record.args().to_string();
            let payload = build_json_payload(
                &buf.timestamp_seconds().to_string(),
                &record.level().as_str().to_ascii_lowercase(),
                record.target(),
                &message,
            );

            writeln!(buf, "{payload}")
        } else {
            writeln!(
                buf,
                "{} {} [{}] {}",
                buf.timestamp_seconds(),
                record.level(),
                record.target(),
                record.args()
            )
        }
    });

    // only write to file if enabled
    if log_enabled {
        if let Some(parent) = Path::new(log_file).parent()
            && let Err(err) = create_dir_all(parent)
        {
            eprintln!("{}", build_create_log_dir_error(log_file, parent, &err));
            return try_init_builder(builder, level);
        }

        // Restrict newly-created log files (0o640, not world-readable) and
        // refuse to follow a symlinked path — the log is opened as root before
        // privilege drop, so a symlink there would be a root-write primitive.
        let file = match OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o640)
            .custom_flags(libc::O_NOFOLLOW)
            .open(log_file)
        {
            Ok(file) => file,
            Err(err) => {
                eprintln!("{}", build_open_log_file_error(log_file, &err));
                return try_init_builder(builder, level);
            }
        };

        builder.target(Target::Pipe(Box::new(file)));
    }
    // else → default (stderr)

    try_init_builder(builder, level)
}

fn try_init_builder(mut builder: Builder, effective_level: LevelFilter) -> LoggerInitStatus {
    match builder.try_init() {
        Ok(()) => {
            log::set_max_level(effective_level);
            LoggerInitStatus::Initialized
        }
        Err(_) => LoggerInitStatus::AlreadyInitialized,
    }
}

pub fn set_log_level(level: &str) -> Result<(), LogLevelError> {
    let level = parse_log_level_filter(level)?;
    log::set_max_level(level);
    Ok(())
}

fn parse_log_level_filter(level: &str) -> Result<LevelFilter, LogLevelError> {
    match level.to_ascii_lowercase().as_str() {
        "whisper" | "trace" => Ok(LevelFilter::Trace),
        "haunt" | "debug" => Ok(LevelFilter::Debug),
        "spooky" | "info" => Ok(LevelFilter::Info),
        "scream" | "warn" => Ok(LevelFilter::Warn),
        "poltergeist" | "error" => Ok(LevelFilter::Error),
        "silence" | "off" => Ok(LevelFilter::Off),
        _ => Err(LogLevelError::new(level)),
    }
}
