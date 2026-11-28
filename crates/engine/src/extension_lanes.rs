//! Installed torrent-provider extensions as release-search lanes, their
//! lifecycle and their health (#304, #306, ADR 0011, ADR 0012, ADR 0014).
//!
//! The engine builds an [`ExtensionHost`] from the runnable rows in
//! `autumn.db` when it starts, and [`ExtensionLanes`] hands it to `search` as
//! the provider set. Enabling, disabling, re-enabling or uninstalling an
//! extension rebuilds the host from the database, stops the changed
//! extension's actor and purges its cached search rows, so a disabled
//! extension's rows never come back within the 60 s cache window.
//!
//! Every finished provider call is recorded in the extension's health. A
//! health change is published as [`AppEvent::ExtensionHealthChanged`]; the
//! third CPU or memory kill in a row also publishes
//! [`AppEvent::ExtensionAutoDisabled`] once and drops the lane.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, PoisonError, RwLock},
    time::SystemTime,
};

use db::Db;
use domain::{AppEvent, ExtensionHealth, ExtensionId};
use extensions::{
    ExtensionError, ExtensionHost, HealthChange, HealthMonitor, PackageError, Packages,
    ProviderRequest, ProviderTorrent,
};
use flume::Sender;
use search::{ExtensionProviders, ProviderMeta, ProviderSet};

/// One installed extension, as the Extensions screen lists it.
///
/// Every installed row is listed, including disabled ones. A row this
/// version of Autumn can no longer load (its stored manifest fails today's
/// rules) is left out, with a warning in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExtensionSummary {
    /// The manifest slug, unique among installed extensions.
    pub id: ExtensionId,
    /// The manifest's display name.
    pub name: String,
    /// The manifest's semantic version, as written.
    pub version: String,
    /// The manifest's `runtime`: `luau` or `declarative`.
    pub runtime: String,
    /// The user's toggle. A disabled extension keeps its row and settings but
    /// gets no search calls.
    pub enabled: bool,
    /// Healthy, failing with its last error, or auto-disabled. Survives a
    /// restart.
    pub health: ExtensionHealth,
    /// The traceback of the last script error, kept in memory until the next
    /// success, Clear error or restart.
    pub traceback: Option<String>,
}

/// The extension lanes `search` runs: the current host, swapped whole when an
/// extension's lifecycle changes.
///
/// A search reads the lanes once when it starts, so one already running
/// finishes on the host it began with. Cloning shares everything.
#[derive(Debug, Clone)]
pub(crate) struct ExtensionLanes {
    current: Arc<RwLock<Lanes>>,
    /// Serializes rebuilds so two never swap out of order.
    lifecycle: Arc<tokio::sync::Mutex<()>>,
    db: Db,
    health: HealthMonitor,
    events: Sender<AppEvent>,
}

#[derive(Debug, Clone)]
struct Lanes {
    host: ExtensionHost,
    providers: ExtensionProviders,
}

impl Lanes {
    fn new(host: ExtensionHost, hosts: HashMap<ExtensionId, Vec<String>>) -> Self {
        Self {
            providers: ExtensionProviders::new(host.clone(), hosts),
            host,
        }
    }
}

/// What a lifecycle change does to the extension's row.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Change {
    Enable,
    Disable,
    /// Clears `auto_disabled`, the last error and the strikes.
    Reenable,
    Uninstall,
}

impl ExtensionLanes {
    /// Lanes for the runnable Luau providers in `db`, publishing health on
    /// `events`. A database failure is logged and leaves no extension lanes,
    /// so search behaves as with nothing installed.
    ///
    /// Must run inside a Tokio runtime: the host spawns one actor per
    /// provider.
    pub(crate) async fn load(db: &Db, events: Sender<AppEvent>) -> Self {
        let lanes = match runnable(db).await {
            Ok(lanes) => lanes,
            Err(error) => {
                tracing::warn!(%error, "starting without extension lanes");
                Lanes::new(ExtensionHost::start(Vec::new()), HashMap::new())
            }
        };
        Self {
            current: Arc::new(RwLock::new(lanes)),
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            db: db.clone(),
            health: HealthMonitor::new(db.clone()),
            events,
        }
    }

    /// Applies `change` to `id`'s row, rebuilds the host from the database
    /// and stops `id`'s old actor, killing a call in flight. The caller then
    /// purges `id`'s cached search rows.
    pub(crate) async fn apply(&self, id: &ExtensionId, change: Change) -> Result<(), PackageError> {
        let _serial = self.lifecycle.lock().await;
        let packages = Packages::new(&self.db);
        let now = SystemTime::now();
        match change {
            Change::Enable => packages.enable(id, now).await?,
            Change::Disable => packages.disable(id, now).await?,
            Change::Reenable => {
                let change = self.health.re_enable(id, now).await?;
                self.publish(id, change).await;
            }
            Change::Uninstall => {
                packages.uninstall(id).await?;
                self.health.forget(id);
            }
        }
        self.rebuild(id).await
    }

    /// Clear error: forgets `id`'s last error and traceback. An
    /// auto-disabled extension stays off.
    pub(crate) async fn clear_error(&self, id: &ExtensionId) -> Result<(), PackageError> {
        let change = self.health.clear_error(id, SystemTime::now()).await?;
        self.publish(id, change).await;
        Ok(())
    }

    /// Every installed extension `db` can still load, by id, with its health.
    pub(crate) async fn installed(&self) -> Result<Vec<ExtensionSummary>, PackageError> {
        let mut health = self.health.all().await?;
        Ok(Packages::new(&self.db)
            .installed()
            .await?
            .into_iter()
            .map(|package| ExtensionSummary {
                runtime: package.manifest.payload.runtime().to_owned(),
                version: package.manifest.version.to_string(),
                health: health.remove(&package.manifest.id).unwrap_or_default(),
                traceback: self.health.traceback(&package.manifest.id),
                id: package.manifest.id,
                name: package.manifest.name,
                enabled: package.enabled,
            })
            .collect())
    }

    /// Records a finished call's outcome; an auto-disable drops the lane.
    async fn observe(
        &self,
        id: &ExtensionId,
        outcome: &Result<Vec<ProviderTorrent>, ExtensionError>,
    ) {
        let change = match self.health.record(id, outcome, SystemTime::now()).await {
            Ok(change) => change,
            Err(error) => {
                tracing::warn!(extension = %id, %error, "extension health was not recorded");
                return;
            }
        };
        let tripped = change
            .as_ref()
            .is_some_and(|change| change.auto_disabled.is_some());
        self.publish(id, change).await;
        if tripped {
            let _serial = self.lifecycle.lock().await;
            if let Err(error) = self.rebuild(id).await {
                tracing::warn!(extension = %id, %error, "the auto-disabled lane was not dropped");
            }
        }
    }

    /// Sends `change` to the UI: the health, then the one-shot auto-disable.
    async fn publish(&self, id: &ExtensionId, change: Option<HealthChange>) {
        let Some(change) = change else {
            return;
        };
        let mut events = vec![AppEvent::ExtensionHealthChanged {
            id: id.clone(),
            health: change.health,
        }];
        if let Some(failure) = change.auto_disabled {
            events.push(AppEvent::ExtensionAutoDisabled {
                id: id.clone(),
                failure,
            });
        }
        for event in events {
            // No receiver means the UI has gone; nothing is left to tell.
            let _ = self.events.send_async(event).await;
        }
    }

    /// Swaps in a host built from the database and stops `id`'s old actor.
    /// Callers hold `lifecycle`.
    async fn rebuild(&self, id: &ExtensionId) -> Result<(), PackageError> {
        let lanes = runnable(&self.db).await?;
        let old = std::mem::replace(&mut *self.write(), lanes);
        if old.host.providers().any(|info| &info.id == id)
            && let Err(error) = old.host.stop(id).await
        {
            tracing::debug!(extension = %id, %error, "the old extension actor was already gone");
        }
        Ok(())
    }

    fn read(&self) -> Lanes {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Lanes> {
        self.current.write().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ProviderSet for ExtensionLanes {
    fn providers(&self) -> Vec<ProviderMeta> {
        self.read().providers.providers()
    }

    /// Runs the call and records its outcome. When search drops this future
    /// at its deadline, nothing is recorded.
    fn search(
        &self,
        id: ExtensionId,
        request: ProviderRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ProviderTorrent>, ExtensionError>> + Send + 'static>>
    {
        let call = self.read().providers.search(id.clone(), request);
        let lanes = self.clone();
        Box::pin(async move {
            let outcome = call.await;
            lanes.observe(&id, &outcome).await;
            outcome
        })
    }
}

/// A host over `db`'s runnable providers, with each one's declared hosts.
async fn runnable(db: &Db) -> Result<Lanes, PackageError> {
    let packages = Packages::new(db);
    let sources = packages.runnable().await?;
    let hosts = packages
        .installed()
        .await?
        .into_iter()
        .map(|package| {
            let hosts = package
                .manifest
                .hosts
                .iter()
                .map(ToString::to_string)
                .collect();
            (package.manifest.id, hosts)
        })
        .collect();
    Ok(Lanes::new(ExtensionHost::start(sources), hosts))
}

#[cfg(test)]
mod tests {
    use std::{future::Future, sync::Arc};

    use db::Db;
    use domain::{AppEvent, ExtensionHealth, ExtensionId, FailureKind};
    use extensions::Packages;
    use search::{Indexer, Query, SearchOutcome, Searcher, Torrent, Transport};
    use settings::AppPaths;
    use tempfile::TempDir;
    use tokio::runtime::{Builder, Runtime};
    use torrent::TorrentFile;

    use super::ExtensionLanes;
    use crate::{
        AutoSelector, BoxFuture, Engine, EngineServices, EpisodeIds, ExtensionSummary,
        FileListError, FileLister,
    };

    const ORIGIN: &str = "https://example.org/index.json";
    const NYAA_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const FIXED_HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A Luau provider that makes no request and returns one magnet row
    /// named `release`.
    fn fixed_provider(id: &str, release: &str) -> String {
        manifest(
            id,
            &format!(
                r#"return {{ search = function(request)
                    return {{ {{
                        name = "{release}",
                        infohash = "{FIXED_HASH}",
                        magnet_url = "magnet:?xt=urn:btih:{FIXED_HASH}",
                        seeders = 12,
                    }} }}
                end }}"#
            ),
        )
    }

    /// A Luau provider that runs out of its 32 MiB memory cap on every call.
    fn memory_hog(id: &str) -> String {
        manifest(
            id,
            r#"return { search = function(request)
                local big = string.rep("x", 64 * 1024 * 1024)
                return { { name = big } }
            end }"#,
        )
    }

    /// A Luau provider whose `search` raises.
    fn broken_provider(id: &str) -> String {
        manifest(
            id,
            r#"return { search = function(request) error("the site changed") end }"#,
        )
    }

    fn manifest(id: &str, payload: &str) -> String {
        serde_json::json!({
            "id": id,
            "name": format!("{id} name"),
            "author": "someone",
            "version": "1.0.0",
            "api_version": 1,
            "type": "torrent-provider",
            "runtime": "luau",
            "permissions": { "hosts": ["example.org"] },
            "payload": payload,
        })
        .to_string()
    }

    /// Nyaa answers with one release; `AnimeTosho` with none.
    #[derive(Clone)]
    struct FixtureNyaa;

    impl Transport for FixtureNyaa {
        fn fetch(
            &self,
            query: &Query,
        ) -> impl Future<Output = Result<Vec<u8>, search::Error>> + Send {
            let body = if matches!(query, Query::Nyaa { .. }) {
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                     <rss version=\"2.0\" xmlns:nyaa=\"https://nyaa.si/xmlns/nyaa\"><channel>\
                     <title>Nyaa.si</title><item>\
                     <title>[SubsPlease] Sousou no Frieren - 05 (1080p) [5E1A0C05]</title>\
                     <link>https://nyaa.si/download/1.torrent</link>\
                     <guid>https://nyaa.si/view/1</guid>\
                     <pubDate>Fri, 27 Nov 2026 16:00:00 +0000</pubDate>\
                     <nyaa:seeders>150</nyaa:seeders><nyaa:leechers>4</nyaa:leechers>\
                     <nyaa:downloads>900</nyaa:downloads><nyaa:infoHash>{NYAA_HASH}</nyaa:infoHash>\
                     <nyaa:categoryId>1_2</nyaa:categoryId>\
                     <nyaa:category>Anime - English-translated</nyaa:category>\
                     <nyaa:size>96.0 MiB</nyaa:size><nyaa:comments>0</nyaa:comments>\
                     <nyaa:trusted>Yes</nyaa:trusted><nyaa:remake>No</nyaa:remake></item>\
                     </channel></rss>"
                )
            } else {
                "[]".to_owned()
            };
            std::future::ready(Ok(body.into_bytes()))
        }
    }

    struct NoFiles;

    impl FileLister for NoFiles {
        fn files(&self, _torrent: Torrent) -> BoxFuture<Result<Vec<TorrentFile>, FileListError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    struct NoIds;

    impl EpisodeIds for NoIds {
        fn anidb_eid(&self, _anilist_id: u32, _episode_key: String) -> BoxFuture<Option<u32>> {
            Box::pin(async { None })
        }
    }

    struct Fixture {
        _dir: TempDir,
        paths: AppPaths,
        runtime: Runtime,
        engine: Engine,
        db: Db,
        events: flume::Receiver<AppEvent>,
    }

    impl Fixture {
        /// Installs `manifests` into a fresh `autumn.db`, starts the engine
        /// on it and points its searcher at fixture Nyaa.
        fn start(manifests: &[String]) -> Self {
            let dir = tempfile::tempdir().expect("a temp directory");
            let paths = AppPaths::new(
                dir.path().join("config"),
                dir.path().join("data"),
                dir.path().join("cache"),
            );
            paths.create_dirs().expect("the test directories");
            let runtime = Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("a test runtime");
            let db = runtime
                .block_on(Db::open(&paths.database_file()))
                .expect("the database");
            for manifest in manifests {
                runtime
                    .block_on(Packages::new(&db).install(
                        manifest,
                        ORIGIN,
                        std::time::SystemTime::now(),
                    ))
                    .expect("the package installs");
            }
            let (engine, events) = fixture_engine(&runtime, &paths);
            Self {
                _dir: dir,
                paths,
                runtime,
                engine,
                db,
                events,
            }
        }

        /// Drops the engine and starts a new one on the same `autumn.db`.
        fn restart(&mut self) {
            let (engine, events) = fixture_engine(&self.runtime, &self.paths);
            self.engine = engine;
            self.events = events;
        }

        /// The health events sent so far, in order.
        fn health_events(&self) -> Vec<AppEvent> {
            self.events.drain().collect()
        }

        fn search(&self) -> SearchOutcome {
            self.runtime
                .block_on(self.engine.search_releases("frieren".to_owned(), false))
                .expect("the search answers")
        }

        fn installed(&self) -> Vec<ExtensionSummary> {
            self.runtime
                .block_on(self.engine.installed_extensions())
                .expect("the extensions list")
        }
    }

    fn fixture_engine(runtime: &Runtime, paths: &AppPaths) -> (Engine, flume::Receiver<AppEvent>) {
        let (events, receiver) = flume::unbounded();
        let engine = Engine::new(runtime, paths, events).expect("the engine");
        let lanes: Arc<dyn search::ProviderSet> = Arc::<ExtensionLanes>::clone(&engine.extensions);
        let searcher = Searcher::new(FixtureNyaa).with_providers(lanes);
        let selector = AutoSelector::new(
            Arc::new(Arc::new(searcher)),
            Arc::new(NoFiles),
            Arc::new(NoIds),
        );
        *engine
            .auto_select
            .write()
            .expect("the selector lock is not poisoned") = selector;
        // Only health events matter here; the torrent sampler's are noise.
        let (health, filtered) = flume::unbounded();
        std::thread::spawn(move || {
            for event in receiver {
                if matches!(
                    event,
                    AppEvent::ExtensionHealthChanged { .. }
                        | AppEvent::ExtensionAutoDisabled { .. }
                ) && health.send(event).is_err()
                {
                    return;
                }
            }
        });
        (engine, filtered)
    }

    fn summary<'a>(summaries: &'a [ExtensionSummary], id: &str) -> &'a ExtensionSummary {
        summaries
            .iter()
            .find(|summary| summary.id.as_str() == id)
            .expect("the extension is listed")
    }

    fn names(outcome: &SearchOutcome) -> Vec<(String, String)> {
        outcome
            .torrents
            .iter()
            .map(|torrent| (format!("{:?}", torrent.indexer), torrent.name.clone()))
            .collect()
    }

    fn extension_rows(outcome: &SearchOutcome) -> Vec<&Torrent> {
        outcome
            .torrents
            .iter()
            .filter(|torrent| matches!(torrent.indexer, Indexer::Extension { .. }))
            .collect()
    }

    #[test]
    fn an_installed_provider_answers_next_to_nyaa() {
        let fixture = Fixture::start(&[fixed_provider("fixed-ext", "[Ext] Frieren - 05")]);

        let outcome = fixture.search();
        assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
        assert_eq!(outcome.torrents.len(), 2, "{:?}", names(&outcome));
        assert!(
            outcome
                .torrents
                .iter()
                .any(|torrent| torrent.indexer == Indexer::Nyaa && torrent.infohash == NYAA_HASH),
            "{:?}",
            names(&outcome)
        );
        let rows = extension_rows(&outcome);
        assert_eq!(rows.len(), 1, "{:?}", names(&outcome));
        let row = rows.first().expect("one provider row");
        assert!(
            matches!(&row.indexer, Indexer::Extension { id, name }
                if id.as_str() == "fixed-ext" && name.as_ref() == "fixed-ext name"),
            "{:?}",
            row.indexer
        );
        assert_eq!(row.infohash, FIXED_HASH);
        assert_eq!(
            fixture.installed(),
            [ExtensionSummary {
                id: ExtensionId::new("fixed-ext"),
                name: "fixed-ext name".to_owned(),
                version: "1.0.0".to_owned(),
                runtime: "luau".to_owned(),
                enabled: true,
                health: ExtensionHealth::Healthy,
                traceback: None,
            }]
        );
    }

    #[test]
    fn disable_enable_and_uninstall_change_the_next_search() {
        let fixture = Fixture::start(&[fixed_provider("fixed-ext", "[Ext] Frieren - 05")]);
        let id = ExtensionId::new("fixed-ext");
        assert_eq!(extension_rows(&fixture.search()).len(), 1);

        fixture
            .runtime
            .block_on(fixture.engine.disable_extension(id.clone()))
            .expect("disable");
        let outcome = fixture.search();
        assert!(extension_rows(&outcome).is_empty(), "{:?}", names(&outcome));
        assert_eq!(outcome.torrents.len(), 1, "Nyaa still answers");
        assert!(
            fixture.installed().iter().all(|summary| !summary.enabled),
            "the row is kept, disabled"
        );

        fixture
            .runtime
            .block_on(fixture.engine.enable_extension(id.clone()))
            .expect("enable");
        assert_eq!(extension_rows(&fixture.search()).len(), 1);

        fixture
            .runtime
            .block_on(fixture.engine.uninstall_extension(id.clone()))
            .expect("uninstall");
        let outcome = fixture.search();
        assert!(extension_rows(&outcome).is_empty(), "{:?}", names(&outcome));
        assert!(fixture.installed().is_empty(), "the row is deleted");

        // Reinstalled under the same id and version, the provider must run
        // again: the cached rows of the uninstalled one are gone.
        fixture
            .runtime
            .block_on(Packages::new(&fixture.db).install(
                &fixed_provider("fixed-ext", "[Ext] Frieren - 05 v2"),
                ORIGIN,
                std::time::SystemTime::now(),
            ))
            .expect("reinstall");
        fixture
            .runtime
            .block_on(fixture.engine.enable_extension(id))
            .expect("enable reloads the lanes");
        let outcome = fixture.search();
        assert_eq!(
            extension_rows(&outcome)
                .iter()
                .map(|torrent| torrent.name.as_str())
                .collect::<Vec<_>>(),
            ["[Ext] Frieren - 05 v2"],
            "no stale cache hit"
        );
    }

    #[test]
    fn a_raising_provider_is_one_failure_while_the_rest_answer() {
        let fixture = Fixture::start(&[
            fixed_provider("fixed-ext", "[Ext] Frieren - 05"),
            broken_provider("broken-ext"),
        ]);

        let outcome = fixture.search();
        let failed: Vec<_> = outcome
            .failures
            .iter()
            .map(|failure| failure.indexer.clone())
            .collect();
        assert!(
            matches!(failed.as_slice(), [Indexer::Extension { id, .. }] if id.as_str() == "broken-ext"),
            "{:?}",
            outcome.failures
        );
        assert_eq!(outcome.torrents.len(), 2, "{:?}", names(&outcome));
    }

    #[test]
    fn with_nothing_installed_only_the_built_ins_answer() {
        let fixture = Fixture::start(&[]);

        let outcome = fixture.search();
        assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
        assert_eq!(names(&outcome).len(), 1);
        assert_eq!(fixture.installed(), Vec::<ExtensionSummary>::new());
    }

    #[test]
    fn changing_an_unknown_extension_names_the_id() {
        let fixture = Fixture::start(&[]);

        let error = fixture
            .runtime
            .block_on(
                fixture
                    .engine
                    .disable_extension(ExtensionId::new("missing-ext")),
            )
            .expect_err("nothing is installed");
        assert_eq!(
            error.to_string(),
            "the extension change failed: no installed extension has id missing-ext"
        );
    }

    #[test]
    fn three_memory_kills_auto_disable_the_lane_once_and_survive_a_restart() {
        let mut fixture = Fixture::start(&[
            fixed_provider("fixed-ext", "[Ext] Frieren - 05"),
            memory_hog("hog-ext"),
        ]);
        let hog = ExtensionId::new("hog-ext");
        let failed = |outcome: &SearchOutcome| {
            outcome.failures.iter().any(
                |failure| matches!(&failure.indexer, Indexer::Extension { id, .. } if *id == hog),
            )
        };

        for _ in 0..3 {
            let outcome = fixture.search();
            assert!(failed(&outcome), "{:?}", outcome.failures);
            assert_eq!(outcome.torrents.len(), 2, "Nyaa and fixed-ext still answer");
        }
        let outcome = fixture.search();
        assert!(
            !failed(&outcome),
            "the lane is dropped: {:?}",
            outcome.failures
        );
        assert_eq!(outcome.torrents.len(), 2);

        // Sent before each search returns, but they pass the filter thread.
        let mut events = Vec::new();
        while events.len() < 4 {
            events.push(
                fixture
                    .events
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("a health event"),
            );
        }
        events.extend(fixture.health_events());
        let kinds: Vec<&str> = events
            .iter()
            .map(|event| match event {
                AppEvent::ExtensionHealthChanged {
                    health: ExtensionHealth::Failing(_),
                    ..
                } => "failing",
                AppEvent::ExtensionHealthChanged {
                    health: ExtensionHealth::AutoDisabled(_),
                    ..
                } => "auto-disabled",
                AppEvent::ExtensionAutoDisabled { .. } => "toast",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["failing", "failing", "auto-disabled", "toast"]);

        let listed = fixture.installed();
        let health = summary(&listed, "hog-ext").health.clone();
        assert!(
            matches!(&health, ExtensionHealth::AutoDisabled(Some(failure))
                if failure.kind == FailureKind::MemoryLimit),
            "{health:?}"
        );
        assert!(
            summary(&listed, "hog-ext").enabled,
            "the user's toggle is untouched"
        );

        fixture.restart();
        assert_eq!(summary(&fixture.installed(), "hog-ext").health, health);
        assert!(!failed(&fixture.search()), "still off after a restart");

        fixture
            .runtime
            .block_on(fixture.engine.reenable_extension(hog.clone()))
            .expect("re-enable");
        assert_eq!(
            summary(&fixture.installed(), "hog-ext").health,
            ExtensionHealth::Healthy
        );
        assert!(failed(&fixture.search()), "the lane is back");
    }

    #[test]
    fn a_script_error_keeps_its_traceback_until_cleared() {
        let fixture = Fixture::start(&[broken_provider("broken-ext")]);
        fixture.search();

        let listed = fixture.installed();
        let broken = summary(&listed, "broken-ext");
        assert!(
            matches!(&broken.health, ExtensionHealth::Failing(failure)
                if failure.kind == FailureKind::RuntimeError
                    && failure.message.contains("the site changed")),
            "{:?}",
            broken.health
        );
        assert!(
            broken.traceback.is_some(),
            "the traceback is kept in memory"
        );

        fixture
            .runtime
            .block_on(
                fixture
                    .engine
                    .clear_extension_error(ExtensionId::new("broken-ext")),
            )
            .expect("clear error");
        let listed = fixture.installed();
        let broken = summary(&listed, "broken-ext");
        assert_eq!(broken.health, ExtensionHealth::Healthy);
        assert_eq!(broken.traceback, None);
    }
}
