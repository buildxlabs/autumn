//! `autumn.log` and `print`: extension output into `tracing` (#267 §5).
//!
//! Lines go to target `extension` with field `extension = <id>`. Each line is
//! cut to 4 KiB, a call logs at most 200 lines, and the values in the call's
//! secret set are replaced with `<redacted>` before anything is emitted.

use std::sync::{Arc, Mutex, PoisonError};

use domain::ExtensionId;
use mlua::{Function, Lua, Table, Value, Variadic};

use super::Builder;
use crate::error::truncate;

/// The longest line kept, in bytes.
const MAX_LINE_BYTES: usize = 4 * 1024;
/// The most lines one call may log.
const MAX_LINES_PER_CALL: usize = 200;
/// Shorter secrets are too likely to match ordinary text.
const MIN_SECRET_CHARS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

/// One VM's log state. Cloning shares it.
#[derive(Debug, Clone)]
pub(crate) struct CallLog {
    id: ExtensionId,
    state: Arc<Mutex<State>>,
}

#[derive(Debug, Default)]
struct State {
    emitted: usize,
    dropped: usize,
    /// Longest first, so a secret containing another is replaced whole.
    secrets: Vec<String>,
}

impl CallLog {
    pub(crate) fn new(id: ExtensionId) -> Self {
        Self {
            id,
            state: Arc::default(),
        }
    }

    /// Replaces the values redacted from every line (the extension's current
    /// `secret` settings); values under 4 characters are ignored.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the settings ticket (#304) fills the secret set")
    )]
    pub(crate) fn set_secrets(&self, secrets: impl IntoIterator<Item = String>) {
        let mut secrets: Vec<String> = secrets
            .into_iter()
            .filter(|secret| secret.chars().count() >= MIN_SECRET_CHARS)
            .collect();
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        self.lock().secrets = secrets;
    }

    /// Starts a call's line budget. The returned guard writes the
    /// `… N lines dropped` note when the call ends, however it ends.
    pub(crate) fn begin_call(&self) -> CallGuard {
        let mut state = self.lock();
        state.emitted = 0;
        state.dropped = 0;
        CallGuard(self.clone())
    }

    /// `autumn.log.debug/info/warn/error`.
    pub(super) fn module(&self, build: &Builder<'_>) -> mlua::Result<Table> {
        build.table([
            (
                "debug",
                self.function(build, "autumn.log.debug", Level::Debug)?,
            ),
            (
                "info",
                self.function(build, "autumn.log.info", Level::Info)?,
            ),
            (
                "warn",
                self.function(build, "autumn.log.warn", Level::Warn)?,
            ),
            (
                "error",
                self.function(build, "autumn.log.error", Level::Error)?,
            ),
        ])
    }

    /// `print`, which logs like `autumn.log.info`.
    pub(super) fn print(&self, build: &Builder<'_>) -> mlua::Result<Function> {
        self.function(build, "print", Level::Info)
    }

    fn function(
        &self,
        build: &Builder<'_>,
        name: &'static str,
        level: Level,
    ) -> mlua::Result<Function> {
        let log = self.clone();
        build.function(name, move |_: &Lua, args: Variadic<Value>| {
            let parts = args
                .iter()
                .map(Value::to_string)
                .collect::<mlua::Result<Vec<_>>>()?;
            log.line(level, parts.join(" "));
            Ok(Value::Nil)
        })
    }

    fn line(&self, level: Level, mut line: String) {
        let mut state = self.lock();
        if state.emitted >= MAX_LINES_PER_CALL {
            state.dropped += 1;
            return;
        }
        state.emitted += 1;
        for secret in &state.secrets {
            line = line.replace(secret.as_str(), "<redacted>");
        }
        drop(state);
        truncate(&mut line, MAX_LINE_BYTES);
        let id = &self.id;
        match level {
            Level::Debug => tracing::debug!(target: "extension", extension = %id, "{line}"),
            Level::Info => tracing::info!(target: "extension", extension = %id, "{line}"),
            Level::Warn => tracing::warn!(target: "extension", extension = %id, "{line}"),
            Level::Error => tracing::error!(target: "extension", extension = %id, "{line}"),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Ends a call's log budget when dropped.
#[derive(Debug)]
pub(crate) struct CallGuard(CallLog);

impl Drop for CallGuard {
    fn drop(&mut self) {
        let dropped = std::mem::take(&mut self.0.lock().dropped);
        if dropped > 0 {
            let id = &self.0.id;
            tracing::warn!(target: "extension", extension = %id, "… {dropped} lines dropped");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    use crate::host::test_support::vm;

    /// Formatted `tracing` output collected in memory.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Runs `source` as one call and returns the log lines it produced.
    fn logged(source: &str, secrets: &[&str]) -> Vec<String> {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::DEBUG)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let (lua, log) = vm();
            log.set_secrets(secrets.iter().map(|secret| (*secret).to_owned()));
            let call = log.begin_call();
            lua.load(source).exec().unwrap();
            drop(call);
        });
        let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn levels_map_and_arguments_join_with_spaces() {
        let lines = logged(
            "autumn.log.debug('d', 1) autumn.log.info('i', true) autumn.log.warn('w') autumn.log.error('e', nil) print('p', 2)",
            &[],
        );
        assert_eq!(
            lines,
            [
                "DEBUG extension: d 1 extension=t",
                " INFO extension: i true extension=t",
                " WARN extension: w extension=t",
                "ERROR extension: e nil extension=t",
                " INFO extension: p 2 extension=t",
            ]
        );
    }

    #[test]
    fn long_lines_are_cut_to_4_kib() {
        let lines = logged("autumn.log.info(string.rep('x', 10000))", &[]);
        let line = lines.first().unwrap();
        assert!(
            line.contains(&"x".repeat(4096)) && !line.contains(&"x".repeat(4097)),
            "{} bytes",
            line.len()
        );
    }

    #[test]
    fn a_call_logs_200_lines_then_notes_the_rest() {
        let lines = logged("for i = 1, 250 do autumn.log.info(i) end", &[]);
        assert_eq!(lines.len(), 201);
        assert_eq!(lines.get(199).unwrap(), " INFO extension: 200 extension=t");
        assert_eq!(
            lines.last().unwrap(),
            " WARN extension: … 50 lines dropped extension=t"
        );
    }

    #[test]
    fn secret_values_are_redacted_but_short_ones_are_not() {
        let lines = logged(
            "autumn.log.info('key=hunter22 & hunter2 & abc')",
            &["hunter2", "hunter22", "abc"],
        );
        assert_eq!(
            lines,
            [" INFO extension: key=<redacted> & <redacted> & abc extension=t"]
        );
    }
}
