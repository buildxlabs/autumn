//! The `tracing` setup: daily log files under `<data>/logs/` plus stderr
//! (ARCHITECTURE §7).

use std::{fs, path::Path};

use anyhow::Context as _;
use tracing::{Subscriber, level_filters::LevelFilter};
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    EnvFilter, Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

/// Log files are `autumn.YYYY-MM-DD.log` (UTC).
const FILE_PREFIX: &str = "autumn";
const FILE_SUFFIX: &str = "log";
/// One week of daily files; older ones are deleted on rotation (decision #17).
const MAX_LOG_FILES: usize = 7;

/// Installs the global subscriber, and routes `log` records (GPUI logs
/// through `log`) into it. Keep the returned guard alive until exit: dropping
/// it flushes the file writer.
///
/// `RUST_LOG`, when set, is the filter for both the file and stderr.
/// Otherwise the file gets INFO and stderr gets WARN.
pub(crate) fn init(log_dir: &Path) -> anyhow::Result<WorkerGuard> {
    let rust_log = std::env::var("RUST_LOG").ok();
    let (subscriber, guard) = subscriber(log_dir, rust_log.as_deref())?;
    subscriber
        .try_init()
        .context("a global logger was already installed")?;
    Ok(guard)
}

/// Builds the file + stderr subscriber without installing it.
fn subscriber(
    log_dir: &Path,
    rust_log: Option<&str>,
) -> anyhow::Result<(impl Subscriber + Send + Sync + use<>, WorkerGuard)> {
    // The appender prunes old files before its first write and complains on
    // stderr when the directory is missing, so create it first.
    fs::create_dir_all(log_dir)
        .with_context(|| format!("could not create {}", log_dir.display()))?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(FILE_PREFIX)
        .filename_suffix(FILE_SUFFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(log_dir)
        .with_context(|| format!("could not open a log file in {}", log_dir.display()))?;
    let (file_writer, guard) = tracing_appender::non_blocking(appender);

    let file = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(file_writer)
        .with_filter(filter(rust_log, LevelFilter::INFO));
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(filter(rust_log, LevelFilter::WARN));

    Ok((
        tracing_subscriber::registry().with(file).with(stderr),
        guard,
    ))
}

/// `RUST_LOG` when given (invalid directives are skipped), else `default`.
fn filter(rust_log: Option<&str>, default: LevelFilter) -> EnvFilter {
    let builder = EnvFilter::builder().with_default_directive(default.into());
    match rust_log {
        Some(directives) => builder.parse_lossy(directives),
        None => builder.parse_lossy(""),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::subscriber;

    /// Logs one INFO and one DEBUG line into a fresh dir and returns the
    /// names of the files created and the text written.
    fn log_lines(rust_log: Option<&str>) -> (Vec<String>, String) {
        let dir = tempfile::tempdir().unwrap();
        let (subscriber, guard) = subscriber(dir.path(), rust_log).unwrap();
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("info line");
            tracing::debug!("debug line");
        });
        drop(guard);

        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let text = names
            .iter()
            .map(|name| fs::read_to_string(dir.path().join(name)).unwrap())
            .collect();
        (names, text)
    }

    #[test]
    fn writes_a_dated_file_at_info_by_default() {
        let (names, text) = log_lines(None);

        assert_eq!(names.len(), 1, "one log file: {names:?}");
        let name = names.first().unwrap();
        let date = name
            .strip_prefix("autumn.")
            .and_then(|rest| rest.strip_suffix(".log"))
            .unwrap_or_default();
        assert_eq!(date.len(), "YYYY-MM-DD".len(), "dated file name: {name}");
        assert!(text.contains("info line"), "INFO reaches the file");
        assert!(!text.contains("debug line"), "DEBUG is filtered by default");
    }

    #[test]
    fn rust_log_debug_puts_debug_lines_in_the_file() {
        let (_, text) = log_lines(Some("debug"));

        assert!(
            text.contains("debug line"),
            "RUST_LOG=debug reaches the file"
        );
    }

    #[test]
    fn missing_log_dir_is_created() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("logs");

        let (_subscriber, _guard) = subscriber(&dir, None).unwrap();

        assert!(Path::new(&dir).is_dir(), "the appender creates logs/");
    }
}
