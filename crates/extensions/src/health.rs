//! Extension health: the last failure, three strikes and auto-disable
//! (#306, ADR 0014, #269 §3).
//!
//! What an extension author can expect: every failed call is recorded as the
//! extension's last error, with its [`FailureKind`], message and time, and
//! the Extensions screen shows it as failing until the next successful call.
//! Three CPU-limit or memory-limit kills in a row, with no successful call in
//! between, turn the extension off (`auto_disabled`) until the user
//! re-enables it. Wall timeouts, request limits, script errors and blocked
//! requests are recorded but never count as strikes. A call that is
//! cancelled, by the user or by the 10 s search deadline, is not recorded.
//!
//! `auto_disabled` and the last error are stored on the extension's row and
//! survive a restart. The strike count and the last traceback live in memory
//! only, so a restart starts the count over.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
    time::SystemTime,
};

use db::{Db, InstalledExtension, StoredHealth};
use domain::{ExtensionFailure, ExtensionHealth, ExtensionId, FailureKind};

use crate::{error::ExtensionError, package::PackageError};

/// How many CPU or memory kills in a row auto-disable an extension.
pub const STRIKES: u8 = 3;

/// The kind of failure `error` records, or `None` when it is not the
/// extension's failure: a cancelled call, a stopped actor or an unknown id.
#[must_use]
pub fn failure_kind(error: &ExtensionError) -> Option<FailureKind> {
    match error {
        ExtensionError::CpuLimit => Some(FailureKind::CpuLimit),
        ExtensionError::WallTimeout => Some(FailureKind::WallTimeout),
        ExtensionError::MemoryLimit => Some(FailureKind::MemoryLimit),
        ExtensionError::RequestLimit => Some(FailureKind::RequestLimit),
        ExtensionError::NetworkBlocked(_) => Some(FailureKind::NetworkBlocked),
        ExtensionError::InvalidProvider(_)
        | ExtensionError::InvalidResult(_)
        | ExtensionError::Runtime { .. }
        | ExtensionError::Vm(_) => Some(FailureKind::RuntimeError),
        ExtensionError::Cancelled
        | ExtensionError::Stopped(_)
        | ExtensionError::UnknownExtension(_) => None,
    }
}

/// A change in one extension's health, to publish to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HealthChange {
    /// The extension's health now.
    pub health: ExtensionHealth,
    /// The kill that made the third strike, when this change auto-disabled
    /// the extension. Set exactly once per auto-disable.
    pub auto_disabled: Option<ExtensionFailure>,
}

/// Records call outcomes and turns them into health, stored in `autumn.db`.
///
/// Cloning is cheap and shares the in-memory strike counts.
#[derive(Debug, Clone)]
pub struct HealthMonitor {
    db: Db,
    memory: Arc<Mutex<HashMap<ExtensionId, Memory>>>,
}

/// What is kept only until the app quits.
#[derive(Debug, Default)]
struct Memory {
    strikes: u8,
    traceback: Option<String>,
}

impl HealthMonitor {
    /// A monitor over the extension rows in `db`.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self {
            db,
            memory: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Records one call's outcome for `id` at `now`.
    ///
    /// A success resets the strikes and clears a recorded error. A failure is
    /// stored as the last error, logged at `warn`, and a CPU or memory kill
    /// adds a strike; the third in a row sets `auto_disabled`. The caller
    /// then drops the extension's lane. Returns the change, or `None` when
    /// the health is the same, the outcome is not recorded, or the extension
    /// was uninstalled meanwhile.
    ///
    /// # Errors
    ///
    /// [`PackageError::Db`] when the database fails.
    pub async fn record<T>(
        &self,
        id: &ExtensionId,
        outcome: &Result<T, ExtensionError>,
        now: SystemTime,
    ) -> Result<Option<HealthChange>, PackageError> {
        let Some(row) = self.db.extensions().get(id).await? else {
            return Ok(None);
        };
        let before = health_of(&row);
        let Err(error) = outcome else {
            self.reset(id);
            if row.auto_disabled {
                return Ok(None);
            }
            return self.store(id, now, &before, ExtensionHealth::Healthy).await;
        };
        let Some(kind) = failure_kind(error) else {
            return Ok(None);
        };
        let failure = ExtensionFailure::new(kind, error.to_string(), now);
        tracing::warn!(extension = %id, ?kind, message = %failure.message, "extension call failed");
        let tripped = self.strike(id, error, kind, &before);
        let auto_disabled = row.auto_disabled || tripped;
        if tripped {
            tracing::warn!(extension = %id, "auto-disabled after {STRIKES} CPU or memory kills in a row");
        }
        let health = if auto_disabled {
            ExtensionHealth::AutoDisabled(Some(failure.clone()))
        } else {
            ExtensionHealth::Failing(failure.clone())
        };
        let change = self.store(id, now, &before, health).await?;
        Ok(change.map(|change| HealthChange {
            auto_disabled: tripped.then_some(failure),
            ..change
        }))
    }

    /// Each installed extension's health, by id.
    ///
    /// # Errors
    ///
    /// [`PackageError::Db`] when the database fails.
    pub async fn all(&self) -> Result<HashMap<ExtensionId, ExtensionHealth>, PackageError> {
        let rows = self.db.extensions().list().await?;
        Ok(rows
            .iter()
            .map(|row| (row.package.id.clone(), health_of(row)))
            .collect())
    }

    /// The traceback of `id`'s last script error, until the next success,
    /// clear or restart.
    #[must_use]
    pub fn traceback(&self, id: &ExtensionId) -> Option<String> {
        self.lock()
            .get(id)
            .and_then(|memory| memory.traceback.clone())
    }

    /// Clear error: forgets the last error and traceback, keeping
    /// `auto_disabled` and the strike count.
    ///
    /// # Errors
    ///
    /// [`PackageError::NotInstalled`], or [`PackageError::Db`].
    pub async fn clear_error(
        &self,
        id: &ExtensionId,
        now: SystemTime,
    ) -> Result<Option<HealthChange>, PackageError> {
        let row = self.row(id).await?;
        if let Some(memory) = self.lock().get_mut(id) {
            memory.traceback = None;
        }
        let before = health_of(&row);
        let after = if row.auto_disabled {
            ExtensionHealth::AutoDisabled(None)
        } else {
            ExtensionHealth::Healthy
        };
        self.store(id, now, &before, after).await
    }

    /// Re-enable: clears `auto_disabled`, the last error and the strikes, so
    /// the extension's lane returns on the next search.
    ///
    /// # Errors
    ///
    /// [`PackageError::NotInstalled`], or [`PackageError::Db`].
    pub async fn re_enable(
        &self,
        id: &ExtensionId,
        now: SystemTime,
    ) -> Result<Option<HealthChange>, PackageError> {
        let row = self.row(id).await?;
        self.reset(id);
        let before = health_of(&row);
        self.store(id, now, &before, ExtensionHealth::Healthy).await
    }

    /// Forgets `id`'s strikes and traceback, on removal or update.
    pub fn forget(&self, id: &ExtensionId) {
        self.lock().remove(id);
    }

    async fn row(&self, id: &ExtensionId) -> Result<InstalledExtension, PackageError> {
        self.db
            .extensions()
            .get(id)
            .await?
            .ok_or_else(|| PackageError::NotInstalled(id.clone()))
    }

    /// Writes the row's health when it changed.
    async fn store(
        &self,
        id: &ExtensionId,
        now: SystemTime,
        before: &ExtensionHealth,
        after: ExtensionHealth,
    ) -> Result<Option<HealthChange>, PackageError> {
        if *before == after {
            return Ok(None);
        }
        let (auto_disabled, failure) = match &after {
            ExtensionHealth::AutoDisabled(failure) => (true, failure.as_ref()),
            ExtensionHealth::Failing(failure) => (false, Some(failure)),
            _ => (false, None),
        };
        let last_error = failure.map(encode);
        self.db
            .extensions()
            .set_health(
                id,
                StoredHealth {
                    auto_disabled,
                    last_error: last_error.as_deref(),
                },
                now,
            )
            .await?;
        Ok(Some(HealthChange {
            health: after,
            auto_disabled: None,
        }))
    }

    /// Counts a failure. Returns whether it made the third strike, which
    /// also resets the count.
    fn strike(
        &self,
        id: &ExtensionId,
        error: &ExtensionError,
        kind: FailureKind,
        before: &ExtensionHealth,
    ) -> bool {
        let mut memory = self.lock();
        let memory = memory.entry(id.clone()).or_default();
        memory.traceback = match error {
            ExtensionError::Runtime { traceback, .. } if !traceback.is_empty() => {
                Some(traceback.clone())
            }
            _ => None,
        };
        if !kind.strikes() || matches!(before, ExtensionHealth::AutoDisabled(_)) {
            return false;
        }
        memory.strikes = memory.strikes.saturating_add(1);
        if memory.strikes < STRIKES {
            return false;
        }
        memory.strikes = 0;
        true
    }

    fn reset(&self, id: &ExtensionId) {
        if let Some(memory) = self.lock().get_mut(id) {
            *memory = Memory::default();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ExtensionId, Memory>> {
        self.memory.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A row's health. A stored error this version can't read is dropped with a
/// warning.
fn health_of(row: &InstalledExtension) -> ExtensionHealth {
    let failure = row
        .last_error
        .as_deref()
        .and_then(|text| match serde_json::from_str(text) {
            Ok(failure) => Some(failure),
            Err(error) => {
                tracing::warn!(extension = %row.package.id, %error, "ignoring an unreadable stored extension error");
                None
            }
        });
    match (row.auto_disabled, failure) {
        (true, failure) => ExtensionHealth::AutoDisabled(failure),
        (false, Some(failure)) => ExtensionHealth::Failing(failure),
        (false, None) => ExtensionHealth::Healthy,
    }
}

fn encode(failure: &ExtensionFailure) -> String {
    serde_json::to_string(failure).unwrap_or_else(|error| {
        tracing::warn!(%error, "storing an extension error without its details");
        String::new()
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;
    use crate::Packages;

    const MANIFEST: &str = r#"{"id":"my-ext","name":"My ext","author":"someone","version":"1.0.0",
        "api_version":1,"type":"torrent-provider","runtime":"luau",
        "permissions":{"hosts":["example.org"]},"payload":"return {}"}"#;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn id() -> ExtensionId {
        ExtensionId::new("my-ext")
    }

    async fn monitor() -> (tempfile::TempDir, Db, HealthMonitor) {
        let dir = tempfile::tempdir().expect("a temp directory");
        let db = Db::open(&dir.path().join("autumn.db"))
            .await
            .expect("the database");
        Packages::new(&db)
            .install(MANIFEST, "https://example.org/index.json", at(1))
            .await
            .expect("the package installs");
        let monitor = HealthMonitor::new(db.clone());
        (dir, db, monitor)
    }

    fn fail(error: ExtensionError) -> Result<(), ExtensionError> {
        Err(error)
    }

    async fn health(monitor: &HealthMonitor) -> ExtensionHealth {
        monitor
            .all()
            .await
            .expect("the health list")
            .remove(&id())
            .expect("the extension is listed")
    }

    #[tokio::test]
    async fn three_kills_in_a_row_auto_disable_once() {
        let (_dir, db, monitor) = monitor().await;
        let kills = [
            ExtensionError::CpuLimit,
            ExtensionError::MemoryLimit,
            ExtensionError::CpuLimit,
        ];
        let mut changes = Vec::new();
        for (second, kill) in (10..).zip(kills) {
            changes.push(
                monitor
                    .record(&id(), &fail(kill), at(second))
                    .await
                    .expect("recorded"),
            );
        }
        let tripped: Vec<_> = changes
            .iter()
            .map(|change| {
                change
                    .as_ref()
                    .and_then(|change| change.auto_disabled.clone())
            })
            .collect();
        let third = ExtensionFailure::new(
            FailureKind::CpuLimit,
            "the extension used too much CPU time",
            at(12),
        );
        assert_eq!(tripped, [None, None, Some(third.clone())]);
        assert_eq!(
            health(&monitor).await,
            ExtensionHealth::AutoDisabled(Some(third))
        );
        let runnable = Packages::new(&db).runnable().await.expect("runnable");
        assert!(runnable.is_empty(), "the lane set skips it: {runnable:?}");

        let again = monitor
            .record(&id(), &fail(ExtensionError::CpuLimit), at(13))
            .await
            .expect("recorded");
        assert!(
            again.is_some_and(|change| change.auto_disabled.is_none()),
            "a late kill never fires a second auto-disable"
        );
    }

    #[tokio::test]
    async fn a_success_between_kills_resets_the_strikes() {
        let (_dir, _db, monitor) = monitor().await;
        for (second, outcome) in (10..).zip([
            fail(ExtensionError::CpuLimit),
            fail(ExtensionError::CpuLimit),
            Ok(()),
            fail(ExtensionError::CpuLimit),
            fail(ExtensionError::MemoryLimit),
        ]) {
            monitor
                .record(&id(), &outcome, at(second))
                .await
                .expect("recorded");
        }
        assert!(
            matches!(health(&monitor).await, ExtensionHealth::Failing(failure)
                if failure.kind == FailureKind::MemoryLimit),
            "two strikes since the success"
        );
    }

    #[tokio::test]
    async fn other_kinds_never_strike() {
        let (_dir, _db, monitor) = monitor().await;
        let errors = [
            ExtensionError::WallTimeout,
            ExtensionError::runtime("broken".to_owned(), "trace".to_owned()),
            ExtensionError::NetworkBlocked("host not declared".to_owned()),
            ExtensionError::RequestLimit,
            ExtensionError::InvalidResult("results: expected array".to_owned()),
        ];
        for (second, error) in (10..).zip(errors.into_iter().cycle().take(10)) {
            let change = monitor
                .record(&id(), &fail(error), at(second))
                .await
                .expect("recorded");
            assert!(
                change.is_some_and(|change| change.auto_disabled.is_none()),
                "recorded, never a strike"
            );
        }
        assert!(matches!(
            health(&monitor).await,
            ExtensionHealth::Failing(_)
        ));
    }

    #[tokio::test]
    async fn a_cancelled_call_leaves_health_unchanged() {
        let (_dir, _db, monitor) = monitor().await;
        monitor
            .record(&id(), &fail(ExtensionError::WallTimeout), at(10))
            .await
            .expect("recorded");
        let before = health(&monitor).await;
        for error in [
            ExtensionError::Cancelled,
            ExtensionError::Stopped(id()),
            ExtensionError::UnknownExtension(id()),
        ] {
            let change = monitor
                .record(&id(), &fail(error), at(20))
                .await
                .expect("recorded");
            assert_eq!(change, None);
        }
        assert_eq!(health(&monitor).await, before);
    }

    #[tokio::test]
    async fn changes_are_reported_once() {
        let (_dir, _db, monitor) = monitor().await;
        let (id, success) = (id(), Ok::<(), ExtensionError>(()));
        let ok = || monitor.record(&id, &success, at(5));
        assert_eq!(ok().await.expect("recorded"), None, "healthy stays healthy");

        let failed = monitor
            .record(&id, &fail(ExtensionError::WallTimeout), at(10))
            .await
            .expect("recorded");
        assert!(matches!(
            failed,
            Some(HealthChange {
                health: ExtensionHealth::Failing(_),
                auto_disabled: None
            })
        ));
        let cleared = ok().await.expect("recorded");
        assert_eq!(
            cleared.map(|change| change.health),
            Some(ExtensionHealth::Healthy)
        );
        assert_eq!(ok().await.expect("recorded"), None);
    }

    #[tokio::test]
    async fn health_survives_a_restart_but_the_traceback_does_not() {
        let (_dir, db, monitor) = monitor().await;
        let error = ExtensionError::runtime("broken".to_owned(), "my-ext:3".to_owned());
        monitor
            .record(&id(), &fail(error), at(10))
            .await
            .expect("recorded");
        assert_eq!(monitor.traceback(&id()).as_deref(), Some("my-ext:3"));

        let restarted = HealthMonitor::new(db);
        assert_eq!(
            health(&restarted).await,
            ExtensionHealth::Failing(ExtensionFailure::new(
                FailureKind::RuntimeError,
                "broken",
                at(10)
            ))
        );
        assert_eq!(restarted.traceback(&id()), None);
    }

    #[tokio::test]
    async fn clear_error_and_re_enable() {
        let (_dir, db, monitor) = monitor().await;
        for second in 10..13 {
            monitor
                .record(&id(), &fail(ExtensionError::MemoryLimit), at(second))
                .await
                .expect("recorded");
        }
        let cleared = monitor.clear_error(&id(), at(20)).await.expect("cleared");
        assert_eq!(
            cleared.map(|change| change.health),
            Some(ExtensionHealth::AutoDisabled(None)),
            "Clear error keeps the extension off"
        );

        let enabled = monitor.re_enable(&id(), at(30)).await.expect("re-enabled");
        assert_eq!(
            enabled.map(|change| change.health),
            Some(ExtensionHealth::Healthy)
        );
        assert_eq!(
            Packages::new(&db).runnable().await.expect("runnable").len(),
            1,
            "the lane returns"
        );
        for second in 40..42 {
            monitor
                .record(&id(), &fail(ExtensionError::CpuLimit), at(second))
                .await
                .expect("recorded");
        }
        assert!(
            matches!(health(&monitor).await, ExtensionHealth::Failing(_)),
            "re-enable reset the strikes"
        );

        let error = monitor
            .clear_error(&ExtensionId::new("missing"), at(50))
            .await
            .expect_err("not installed");
        assert_eq!(error.to_string(), "no installed extension has id missing");
    }
}
