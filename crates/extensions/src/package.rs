//! Installed packages: validated manifests stored in `autumn.db` (ADR 0012).
//!
//! [`Packages`] installs a manifest that was already fetched (with its
//! payload inlined) together with the `origin_url` it came from, and turns
//! enabled rows back into runnable providers for [`crate::ExtensionHost`].

use std::time::SystemTime;

use db::{Db, DbError, InstalledExtension, NewExtension};
use domain::ExtensionId;
use url::Url;

use crate::{
    actor::ProviderSource,
    manifest::{Manifest, ManifestError, Payload},
};

/// Why installing or loading a package failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PackageError {
    /// The manifest breaks a rule.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// `origin_url` is not an HTTPS URL.
    #[error("origin_url: must be an https URL: {0}")]
    OriginUrl(String),
    /// An extension with this id is already installed.
    #[error("an extension with id {0} is already installed")]
    AlreadyInstalled(ExtensionId),
    /// No installed extension has this id.
    #[error("no installed extension has id {0}")]
    NotInstalled(ExtensionId),
    /// The database failed.
    #[error("the extension database failed")]
    Db(#[source] DbError),
}

impl From<DbError> for PackageError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::ExtensionExists(id) => Self::AlreadyInstalled(id),
            DbError::UnknownExtension(id) => Self::NotInstalled(id),
            other => Self::Db(other),
        }
    }
}

/// One installed extension: its manifest and state.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    /// The stored manifest, checked again on load.
    pub manifest: Manifest,
    /// Where it was installed from.
    pub origin_url: String,
    /// The user's toggle.
    pub enabled: bool,
    /// Set after repeated CPU or memory kills (#269).
    pub auto_disabled: bool,
}

/// The installed extensions in `autumn.db`.
#[derive(Debug, Clone, Copy)]
pub struct Packages<'db> {
    db: &'db Db,
}

impl<'db> Packages<'db> {
    /// Packages stored in `db`.
    #[must_use]
    pub fn new(db: &'db Db) -> Self {
        Self { db }
    }

    /// Validates `manifest_json` and installs it, enabled, in one
    /// transaction.
    ///
    /// # Errors
    ///
    /// - [`PackageError::Manifest`] when the manifest breaks a rule.
    /// - [`PackageError::OriginUrl`] when `origin_url` is not HTTPS.
    /// - [`PackageError::AlreadyInstalled`] when the id is taken.
    /// - [`PackageError::Db`] when the database fails.
    pub async fn install(
        &self,
        manifest_json: &str,
        origin_url: &str,
        now: SystemTime,
    ) -> Result<Manifest, PackageError> {
        let manifest = Manifest::parse(manifest_json)?;
        let origin =
            Url::parse(origin_url).map_err(|error| PackageError::OriginUrl(error.to_string()))?;
        if origin.scheme() != "https" {
            return Err(PackageError::OriginUrl(format!("got {}:", origin.scheme())));
        }
        let package = NewExtension {
            id: manifest.id.clone(),
            origin_url: origin.into(),
            version: manifest.version.to_string(),
            kind: manifest.kind.as_str().to_owned(),
            runtime: manifest.payload.runtime().to_owned(),
            manifest_json: manifest.descriptor(),
            payload: manifest.payload.to_stored(),
        };
        self.db.extensions().install(&package, now).await?;
        Ok(manifest)
    }

    /// Deletes the extension's row in one transaction.
    ///
    /// # Errors
    ///
    /// [`PackageError::NotInstalled`], or [`PackageError::Db`].
    pub async fn uninstall(&self, id: &ExtensionId) -> Result<(), PackageError> {
        Ok(self.db.extensions().uninstall(id).await?)
    }

    /// Turns the extension on.
    ///
    /// # Errors
    ///
    /// [`PackageError::NotInstalled`], or [`PackageError::Db`].
    pub async fn enable(&self, id: &ExtensionId, now: SystemTime) -> Result<(), PackageError> {
        Ok(self.db.extensions().enable(id, now).await?)
    }

    /// Turns the extension off, keeping its row.
    ///
    /// # Errors
    ///
    /// [`PackageError::NotInstalled`], or [`PackageError::Db`].
    pub async fn disable(&self, id: &ExtensionId, now: SystemTime) -> Result<(), PackageError> {
        Ok(self.db.extensions().disable(id, now).await?)
    }

    /// Every installed extension whose manifest this version still accepts,
    /// by id. A row that no longer validates is skipped with a warning.
    ///
    /// # Errors
    ///
    /// [`PackageError::Db`] when the database fails.
    pub async fn installed(&self) -> Result<Vec<Package>, PackageError> {
        let rows = self.db.extensions().list().await?;
        Ok(rows.into_iter().filter_map(package).collect())
    }

    /// The Luau providers to run: enabled, not auto-disabled rows. A
    /// declarative row is stored but not runnable yet (ADR 0016).
    ///
    /// # Errors
    ///
    /// [`PackageError::Db`] when the database fails.
    pub async fn runnable(&self) -> Result<Vec<ProviderSource>, PackageError> {
        Ok(self
            .installed()
            .await?
            .into_iter()
            .filter(|package| package.enabled && !package.auto_disabled)
            .filter_map(|package| {
                let Manifest {
                    id,
                    name,
                    version,
                    payload,
                    ..
                } = package.manifest;
                match payload {
                    Payload::Luau(source) => Some(ProviderSource {
                        id,
                        name,
                        version: version.to_string(),
                        source,
                    }),
                    Payload::Declarative(_) => None,
                }
            })
            .collect())
    }
}

fn package(row: InstalledExtension) -> Option<Package> {
    let stored = &row.package;
    match Manifest::from_stored(&stored.manifest_json, &stored.payload) {
        Ok(manifest) => Some(Package {
            manifest,
            origin_url: row.package.origin_url,
            enabled: row.enabled,
            auto_disabled: row.auto_disabled,
        }),
        Err(error) => {
            tracing::warn!(extension = %stored.id, %error, "skipping an installed extension this version can't load");
            None
        }
    }
}
