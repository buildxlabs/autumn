//! Autumn's engine: the composition root of the Tokio-side services (#58).
//!
//! The engine owns the database, the shared HTTP client, the AniList
//! catalog (with its stale-first disk cache) and the active tracking
//! platform (the Local, account-free platform). `autumn` builds the Tokio
//! runtime and starts the engine with [`Engine::new`]; the ui-state stores
//! call its services through [`bridge::run`] (ARCHITECTURE §4). It grows
//! into ARCHITECTURE's engine in Phase 2.

mod settings_store;

use std::{future::Future, pin::Pin};

use db::Db;
use domain::{ExternalId, ListEntry, ListStatus, MediaId, MediaKind};
use metadata::{AniList, CatalogCache, HomeLoad, Load, MediaDetail};
use net::{Client, HostKind, PosterCache};
use settings::{AppPaths, Settings};
use tokio::runtime::Runtime;
use tracker::{Local, Platform, PlatformError};

/// A boxed engine-service call: object-safe, `Send` and `'static`.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Why an engine service call failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    /// The database could not be opened.
    #[error("the database could not be opened: {0}")]
    Db(#[from] db::DbError),
    /// The settings could not be loaded, or a stored row is invalid.
    #[error("the settings could not be loaded: {0}")]
    Settings(#[from] settings::SettingsError),
    /// The shared HTTP client could not be built.
    #[error("the HTTP client could not be built: {0}")]
    Client(#[from] net::Error),
    /// A metadata request failed.
    #[error("the metadata request failed: {0}")]
    Metadata(#[from] metadata::Error),
    /// A platform call failed.
    #[error("the platform call failed: {0}")]
    Platform(#[from] PlatformError),
}

/// The engine's services: what the Anime Entry and Home screens call through
/// [`bridge::run`]. Boxed futures make the trait object-safe: stores hold
/// `Arc<dyn EngineServices>`, and tests fake it.
pub trait EngineServices: Send + Sync + 'static {
    /// The Entry page's detail, stale-first (24 h TTL).
    fn detail(&self, anilist_id: u32) -> BoxFuture<Result<Load<MediaDetail>, EngineError>>;
    /// The media identity behind the Entry: never mints.
    fn local_media(
        &self,
        kind: MediaKind,
        ids: &[ExternalId],
    ) -> BoxFuture<Result<Option<MediaId>, EngineError>>;
    /// The media's entry on the active platform, if it has one.
    fn local_entry(&self, media: MediaId) -> BoxFuture<Result<Option<ListEntry>, EngineError>>;
    /// Saves the editor draft on the active platform.
    fn save_entry(&self, entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>>;
    /// Finds or mints the media identity: call when the user first records a fact.
    fn mint(&self, kind: MediaKind, ids: &[ExternalId]) -> BoxFuture<Result<MediaId, EngineError>>;
    /// The Home catalog load, stale-first (6 h TTL).
    fn home(&self) -> BoxFuture<Result<Load<HomeLoad>, EngineError>>;
    /// The active platform's entries for `kind`, most recently updated first.
    fn collection(&self, kind: MediaKind) -> BoxFuture<Result<Vec<ListEntry>, EngineError>>;
    /// The active platform's completed entries: the account-free missed-sequels feed.
    fn missed_sequels(&self, kind: MediaKind) -> BoxFuture<Result<Vec<ListEntry>, EngineError>>;
}

/// The engine: owns the database, the shared client, the catalog cache and
/// the active tracking platform.
#[derive(Debug, Clone)]
pub struct Engine {
    db: Db,
    client: Client,
    poster_cache: PosterCache,
    catalog: CatalogCache,
    platform: Local,
    settings: Settings,
}

/// The loopback host of debug fetches; `fetch_status::observe` watches its rate limits.
pub const DEBUG_HOST: &str = "127.0.0.1";

impl Engine {
    /// Opens the database at `paths.database_file()`, imports the Phase 0
    /// `settings.json` once and loads the settings, builds the shared client
    /// (with the debug host registered), and starts the Local platform.
    ///
    /// # Errors
    ///
    /// - [`EngineError::Db`] when the database can't be opened.
    /// - [`EngineError::Settings`] when the settings can't be loaded or a row is invalid.
    /// - [`EngineError::Client`] when the client can't be built.
    pub fn new(runtime: &Runtime, paths: &AppPaths) -> Result<Self, EngineError> {
        let db = runtime.block_on(Db::open(&paths.database_file()))?;
        let settings = runtime.block_on(settings_store::load(&db, &paths.settings_file()))?;
        let mut builder = Client::builder().anilist();
        #[cfg(debug_assertions)]
        {
            builder = builder.host(DEBUG_HOST, 25, 5, HostKind::AniList);
        }
        let client = builder.build()?;
        let poster_cache = PosterCache::new(paths.cache_dir())?;
        let catalog = CatalogCache::new(AniList::new(client.clone()), poster_cache.clone());
        let platform = Local::new(db.clone());
        Ok(Self {
            db,
            client,
            poster_cache,
            catalog,
            platform,
            settings,
        })
    }

    /// The app database.
    #[must_use]
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// The settings, loaded when the engine started.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The shared HTTP client.
    #[must_use]
    pub fn client(&self) -> Client {
        self.client.clone()
    }

    /// The on-disk poster cache.
    #[must_use]
    pub fn poster_cache(&self) -> PosterCache {
        self.poster_cache.clone()
    }
}

impl EngineServices for Engine {
    fn detail(&self, anilist_id: u32) -> BoxFuture<Result<Load<MediaDetail>, EngineError>> {
        let catalog = self.catalog.clone();
        Box::pin(async move { catalog.media(anilist_id).await.map_err(EngineError::from) })
    }

    fn local_media(
        &self,
        kind: MediaKind,
        ids: &[ExternalId],
    ) -> BoxFuture<Result<Option<MediaId>, EngineError>> {
        let db = self.db.clone();
        let ids = ids.to_vec();
        Box::pin(async move { db.media().find(kind, &ids).await.map_err(EngineError::from) })
    }

    fn local_entry(&self, media: MediaId) -> BoxFuture<Result<Option<ListEntry>, EngineError>> {
        let db = self.db.clone();
        Box::pin(async move {
            db.local_list()
                .entry(media)
                .await
                .map_err(EngineError::from)
        })
    }

    fn save_entry(&self, entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>> {
        let platform = self.platform.clone();
        let entry = entry.clone();
        Box::pin(async move { platform.save_entry(&entry).await.map_err(EngineError::from) })
    }

    fn mint(&self, kind: MediaKind, ids: &[ExternalId]) -> BoxFuture<Result<MediaId, EngineError>> {
        let db = self.db.clone();
        let ids = ids.to_vec();
        Box::pin(async move {
            db.media()
                .find_or_mint(kind, &ids)
                .await
                .map(|minted| minted.media)
                .map_err(EngineError::from)
        })
    }

    fn home(&self) -> BoxFuture<Result<Load<HomeLoad>, EngineError>> {
        let catalog = self.catalog.clone();
        Box::pin(async move { catalog.home().await.map_err(EngineError::from) })
    }

    fn collection(&self, kind: MediaKind) -> BoxFuture<Result<Vec<ListEntry>, EngineError>> {
        let platform = self.platform.clone();
        Box::pin(async move {
            let collection = platform.collection(kind).await?;
            Ok(collection
                .lists
                .into_iter()
                .flat_map(|list| list.entries)
                .collect())
        })
    }

    fn missed_sequels(&self, kind: MediaKind) -> BoxFuture<Result<Vec<ListEntry>, EngineError>> {
        let platform = self.platform.clone();
        Box::pin(async move {
            let collection = platform.collection(kind).await?;
            Ok(collection
                .lists
                .into_iter()
                .flat_map(|list| list.entries)
                .filter(|entry| entry.status == ListStatus::Completed)
                .collect())
        })
    }
}
