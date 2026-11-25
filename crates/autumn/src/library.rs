//! Wires the engine's library module to the composition root (#127).
//!
//! The engine never imports ui-state: the active platform's entries and the
//! settings save callback are supplied here, after both sides are built.

use std::cell::RefCell;

use anyhow::Context as _;
use domain::{ListEntry, MediaKind};
use engine::{AnimeList, BoxFuture, Engine, EngineError};
use settings::Settings;
use ui_screens::LibraryServices;
use ui_state::PlatformGateway;

/// The settings row the Library tab lives in.
const LIBRARY_TAB: &str = "library";

/// The active platform's anime entries, read through the UI gateway.
#[derive(Debug, Clone)]
pub(crate) struct GatewayListSource(PlatformGateway);

impl GatewayListSource {
    /// Reads entries from `gateway`, whichever platform is active.
    pub(crate) fn new(gateway: PlatformGateway) -> Self {
        Self(gateway)
    }
}

impl AnimeList for GatewayListSource {
    fn anime_entries(&self) -> BoxFuture<Result<Vec<ListEntry>, EngineError>> {
        let gateway = self.0.clone();
        Box::pin(async move {
            gateway
                .entries(MediaKind::Anime)
                .await
                .map_err(|error| EngineError::ActivePlatform(error.message().to_owned()))
        })
    }
}

/// A save callback for the Library tab. It writes only the Library row and
/// restarts the watcher so a saved change takes effect without a restart.
pub(crate) fn saver(db: db::Db, engine: Engine, settings: Settings) -> LibraryServices {
    let settings = RefCell::new(settings);
    LibraryServices::new(move |library, cx| {
        let (rows, updated) = {
            let mut settings = settings.borrow_mut();
            settings.library = library.clone();
            let rows = settings.to_rows().map(|rows| {
                rows.into_iter()
                    .filter(|(tab, _)| tab == LIBRARY_TAB)
                    .collect::<Vec<_>>()
            });
            (rows, settings.clone())
        };
        let engine = engine.clone();
        let db = db.clone();
        let save = ui_state::bridge::run(cx, async move {
            db.settings()
                .save(&rows.context("could not encode the Library settings")?)
                .await
                .context("could not save the Library settings")?;
            if let Err(error) = engine.apply_library_settings(&updated) {
                tracing::warn!(%error, "the library watcher could not be reloaded");
            }
            Ok::<_, anyhow::Error>(())
        });
        cx.background_executor().spawn(async move {
            let result = match save.await {
                Ok(saved) => saved,
                Err(error) => Err(anyhow::Error::from(error)),
            };
            if let Err(error) = result {
                tracing::warn!(error = %format_args!("{error:#}"), "library settings not saved");
            }
        })
    })
}
