//! Installed extension packages (ADR 0012, #265 §4).
//!
//! Each row holds one manifest that `extensions` already validated, and its
//! inlined payload. This module stores and loads rows; it doesn't read the
//! manifest.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use domain::ExtensionId;
use sqlx::{FromRow, SqlitePool};

use crate::DbError;

/// A package to install, already validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewExtension {
    /// The manifest slug.
    pub id: ExtensionId,
    /// The repository index or manifest URL the package came from.
    pub origin_url: String,
    /// The manifest's semantic `version`.
    pub version: String,
    /// The manifest's `type`, such as `torrent-provider`.
    pub kind: String,
    /// The manifest's `runtime`, such as `luau`.
    pub runtime: String,
    /// The manifest without its payload, as JSON.
    pub manifest_json: String,
    /// The Luau source or declarative definition.
    pub payload: String,
}

/// An installed package and its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledExtension {
    /// The package as installed.
    pub package: NewExtension,
    /// The user's toggle; `true` on install.
    pub enabled: bool,
    /// Set after repeated CPU or memory kills (#269), independent of
    /// `enabled`.
    pub auto_disabled: bool,
    /// The last recorded failure, as the health ticket stores it.
    pub last_error: Option<String>,
    /// When the package was installed.
    pub installed_at: SystemTime,
    /// When the row last changed.
    pub updated_at: SystemTime,
}

/// An extension's stored health, apart from the user's toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StoredHealth<'a> {
    /// Set after repeated CPU or memory kills (#269).
    pub auto_disabled: bool,
    /// The last recorded failure, encoded by `extensions`.
    pub last_error: Option<&'a str>,
}

/// Storage for installed extensions.
#[derive(Debug, Clone, Copy)]
pub struct ExtensionRepo<'a> {
    pool: &'a SqlitePool,
}

#[derive(FromRow)]
struct Row {
    id: String,
    origin_url: String,
    version: String,
    #[sqlx(rename = "type")]
    kind: String,
    runtime: String,
    manifest_json: String,
    payload: String,
    enabled: i64,
    auto_disabled: i64,
    last_error: Option<String>,
    installed_at: i64,
    updated_at: i64,
}

impl Row {
    fn into_installed(self) -> Result<InstalledExtension, DbError> {
        Ok(InstalledExtension {
            package: NewExtension {
                id: ExtensionId::new(self.id),
                origin_url: self.origin_url,
                version: self.version,
                kind: self.kind,
                runtime: self.runtime,
                manifest_json: self.manifest_json,
                payload: self.payload,
            },
            enabled: self.enabled != 0,
            auto_disabled: self.auto_disabled != 0,
            last_error: self.last_error,
            installed_at: time(self.installed_at, "extensions.installed_at")?,
            updated_at: time(self.updated_at, "extensions.updated_at")?,
        })
    }
}

impl<'a> ExtensionRepo<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Installs `package`, enabled.
    ///
    /// # Errors
    ///
    /// [`DbError::ExtensionExists`] when an extension with that id is
    /// installed, or [`DbError::Sqlx`] when a query fails.
    pub async fn install(&self, package: &NewExtension, now: SystemTime) -> Result<(), DbError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let taken: Option<i64> = sqlx::query_scalar("SELECT 1 FROM extensions WHERE id = ?")
            .bind(package.id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
        if taken.is_some() {
            return Err(DbError::ExtensionExists(package.id.clone()));
        }
        sqlx::query(
            "INSERT INTO extensions (id, origin_url, version, type, runtime, manifest_json,
                                     payload, installed_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(package.id.as_str())
        .bind(&package.origin_url)
        .bind(&package.version)
        .bind(&package.kind)
        .bind(&package.runtime)
        .bind(&package.manifest_json)
        .bind(&package.payload)
        .bind(seconds(now))
        .bind(seconds(now))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Every installed extension, by id.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails, or [`DbError::InvalidRow`]
    /// when a stored time is negative.
    pub async fn list(&self) -> Result<Vec<InstalledExtension>, DbError> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, origin_url, version, type, runtime, manifest_json, payload,
                    enabled, auto_disabled, last_error, installed_at, updated_at
             FROM extensions ORDER BY id",
        )
        .fetch_all(self.pool)
        .await?;
        rows.into_iter().map(Row::into_installed).collect()
    }

    /// One installed extension, if any.
    ///
    /// # Errors
    ///
    /// As [`Self::list`].
    pub async fn get(&self, id: &ExtensionId) -> Result<Option<InstalledExtension>, DbError> {
        let row: Option<Row> = sqlx::query_as(
            "SELECT id, origin_url, version, type, runtime, manifest_json, payload,
                        enabled, auto_disabled, last_error, installed_at, updated_at
                 FROM extensions WHERE id = ?",
        )
        .bind(id.as_str())
        .fetch_optional(self.pool)
        .await?;
        row.map(Row::into_installed).transpose()
    }

    /// Turns the user's toggle on.
    ///
    /// # Errors
    ///
    /// [`DbError::UnknownExtension`] when nothing has `id`, or
    /// [`DbError::Sqlx`] when the query fails.
    pub async fn enable(&self, id: &ExtensionId, now: SystemTime) -> Result<(), DbError> {
        self.set_enabled(id, 1, now).await
    }

    /// Turns the user's toggle off; the row and its state are kept.
    ///
    /// # Errors
    ///
    /// As [`Self::enable`].
    pub async fn disable(&self, id: &ExtensionId, now: SystemTime) -> Result<(), DbError> {
        self.set_enabled(id, 0, now).await
    }

    async fn set_enabled(
        &self,
        id: &ExtensionId,
        enabled: i64,
        now: SystemTime,
    ) -> Result<(), DbError> {
        let changed = sqlx::query("UPDATE extensions SET enabled = ?, updated_at = ? WHERE id = ?")
            .bind(enabled)
            .bind(seconds(now))
            .bind(id.as_str())
            .execute(self.pool)
            .await?
            .rows_affected();
        if changed == 0 {
            return Err(DbError::UnknownExtension(id.clone()));
        }
        Ok(())
    }

    /// Stores the extension's health, leaving `enabled` alone.
    ///
    /// # Errors
    ///
    /// As [`Self::enable`].
    pub async fn set_health(
        &self,
        id: &ExtensionId,
        health: StoredHealth<'_>,
        now: SystemTime,
    ) -> Result<(), DbError> {
        let changed = sqlx::query(
            "UPDATE extensions SET auto_disabled = ?, last_error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(i64::from(health.auto_disabled))
        .bind(health.last_error)
        .bind(seconds(now))
        .bind(id.as_str())
        .execute(self.pool)
        .await?
        .rows_affected();
        if changed == 0 {
            return Err(DbError::UnknownExtension(id.clone()));
        }
        Ok(())
    }

    /// Deletes the extension's row.
    ///
    /// # Errors
    ///
    /// [`DbError::UnknownExtension`] when nothing has `id`, or
    /// [`DbError::Sqlx`] when the query fails.
    pub async fn uninstall(&self, id: &ExtensionId) -> Result<(), DbError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let deleted = sqlx::query("DELETE FROM extensions WHERE id = ?")
            .bind(id.as_str())
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if deleted == 0 {
            return Err(DbError::UnknownExtension(id.clone()));
        }
        tx.commit().await?;
        Ok(())
    }
}

/// Unix seconds; times before the epoch are stored as zero.
fn seconds(at: SystemTime) -> i64 {
    let elapsed = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    i64::try_from(elapsed).unwrap_or(i64::MAX)
}

fn time(seconds: i64, column: &'static str) -> Result<SystemTime, DbError> {
    let seconds = u64::try_from(seconds).map_err(|_| DbError::InvalidRow(column))?;
    Ok(UNIX_EPOCH + Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Db;

    async fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
        (dir, db)
    }

    fn package(id: &str) -> NewExtension {
        NewExtension {
            id: ExtensionId::new(id),
            origin_url: "https://example.org/index.json".to_owned(),
            version: "1.0.0".to_owned(),
            kind: "torrent-provider".to_owned(),
            runtime: "luau".to_owned(),
            manifest_json: "{}".to_owned(),
            payload: "return {}".to_owned(),
        }
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[tokio::test]
    async fn install_list_disable_and_uninstall() {
        let (_dir, db) = db().await;
        let repo = db.extensions();
        repo.install(&package("b-ext"), at(10)).await.unwrap();
        repo.install(&package("a-ext"), at(20)).await.unwrap();

        let listed = repo.list().await.unwrap();
        let ids: Vec<_> = listed.iter().map(|row| row.package.id.as_str()).collect();
        assert_eq!(ids, ["a-ext", "b-ext"]);
        let first = listed.first().unwrap();
        assert_eq!(first.package, package("a-ext"));
        assert!(first.enabled && !first.auto_disabled, "enabled on install");
        assert_eq!((first.installed_at, first.updated_at), (at(20), at(20)));
        assert_eq!(first.last_error, None);

        let id = ExtensionId::new("a-ext");
        repo.disable(&id, at(30)).await.unwrap();
        let disabled = repo.get(&id).await.unwrap().unwrap();
        assert!(!disabled.enabled, "disable keeps the row");
        assert_eq!(disabled.updated_at, at(30));
        repo.enable(&id, at(40)).await.unwrap();
        assert!(repo.get(&id).await.unwrap().unwrap().enabled);

        repo.uninstall(&id).await.unwrap();
        assert_eq!(repo.get(&id).await.unwrap(), None);
        assert_eq!(repo.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn health_is_stored_apart_from_the_toggle() {
        let (_dir, db) = db().await;
        let repo = db.extensions();
        repo.install(&package("my-ext"), at(10)).await.unwrap();
        let id = ExtensionId::new("my-ext");

        let failing = StoredHealth {
            auto_disabled: true,
            last_error: Some("boom"),
        };
        repo.set_health(&id, failing, at(20)).await.unwrap();
        let row = repo.get(&id).await.unwrap().unwrap();
        assert!(row.enabled && row.auto_disabled, "the toggle is untouched");
        assert_eq!(row.last_error.as_deref(), Some("boom"));
        assert_eq!(row.updated_at, at(20));

        repo.set_health(&id, StoredHealth::default(), at(30))
            .await
            .unwrap();
        let row = repo.get(&id).await.unwrap().unwrap();
        assert!(!row.auto_disabled);
        assert_eq!(row.last_error, None);
    }

    #[tokio::test]
    async fn a_duplicate_id_is_refused_and_leaves_the_first() {
        let (_dir, db) = db().await;
        let repo = db.extensions();
        repo.install(&package("my-ext"), at(10)).await.unwrap();
        let mut again = package("my-ext");
        again.version = "2.0.0".to_owned();
        let error = repo.install(&again, at(20)).await.unwrap_err();
        assert!(
            matches!(error, DbError::ExtensionExists(ref id) if id.as_str() == "my-ext"),
            "{error:?}"
        );
        let kept = repo
            .get(&ExtensionId::new("my-ext"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(kept.package.version, "1.0.0");
    }

    #[tokio::test]
    async fn unknown_ids_are_errors() {
        let (_dir, db) = db().await;
        let repo = db.extensions();
        let id = ExtensionId::new("missing");
        for error in [
            repo.disable(&id, at(1)).await.unwrap_err(),
            repo.enable(&id, at(1)).await.unwrap_err(),
            repo.uninstall(&id).await.unwrap_err(),
            repo.set_health(&id, StoredHealth::default(), at(1))
                .await
                .unwrap_err(),
        ] {
            assert!(matches!(error, DbError::UnknownExtension(_)), "{error:?}");
        }
    }
}
