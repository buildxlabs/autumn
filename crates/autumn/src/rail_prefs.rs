//! Maps the shell's rail preferences to and from the settings rows.

use std::cell::RefCell;

use anyhow::Context as _;
use db::Db;
use gpui_kit::{App, Task};
use settings::Settings;
use ui_screens::{RailMode, RailPrefs};
use ui_state::Route;

/// The settings row the rail preferences live in.
const APPEARANCE_TAB: &str = "appearance";

/// The rail preferences stored in `settings`.
pub(crate) fn from_settings(settings: &Settings) -> RailPrefs {
    let appearance = &settings.appearance;
    let mode = if appearance.sidebar_click_pin {
        RailMode::ClickPin
    } else {
        RailMode::Hover
    };
    let unpinned = appearance
        .sidebar_unpinned
        .iter()
        .filter_map(|key| Route::from_key(key))
        .collect();
    RailPrefs::new(mode, unpinned)
}

/// A save callback for the shell. It keeps `settings` current and writes the
/// Appearance tab, the only one the rail changes, to `db` on the Tokio bridge,
/// so it never overwrites a tab saved elsewhere. A failed save is logged; the shell
/// keeps the new preferences for this run.
pub(crate) fn saver(db: Db, settings: Settings) -> impl Fn(&RailPrefs, &mut App) -> Task<()> {
    let settings = RefCell::new(settings);
    move |prefs, cx| {
        let rows = {
            let mut settings = settings.borrow_mut();
            settings.appearance.sidebar_click_pin = prefs.mode == RailMode::ClickPin;
            settings.appearance.sidebar_unpinned = prefs
                .unpinned
                .iter()
                .map(|route| route.key().to_owned())
                .collect();
            settings.to_rows().map(|rows| {
                rows.into_iter()
                    .filter(|(tab, _)| tab == APPEARANCE_TAB)
                    .collect::<Vec<_>>()
            })
        };
        let db = db.clone();
        let save = ui_state::bridge::run(cx, async move {
            db.settings()
                .save(&rows.context("could not encode the settings")?)
                .await
                .context("could not save the settings")
        });
        cx.background_executor().spawn(async move {
            let result = match save.await {
                Ok(saved) => saved,
                Err(err) => Err(err.into()),
            };
            if let Err(err) = result {
                tracing::warn!(error = %format_args!("{err:#}"), "rail preferences not saved");
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use db::Db;
    use gpui_kit::TestAppContext;
    use settings::Settings;
    use tokio::runtime::{Builder, Runtime};
    use ui_screens::{RailMode, RailPrefs};
    use ui_state::Route;

    use super::{from_settings, saver};

    fn runtime() -> Runtime {
        Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap()
    }

    #[gpui_kit::test]
    async fn rail_prefs_saved_by_the_shell_survive_a_restart(cx: &mut TestAppContext) {
        let runtime = runtime();
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autumn.db");
        let db = runtime.block_on(Db::open(&path)).unwrap();
        cx.update(|cx| ui_state::bridge::init(cx, runtime.handle().clone()));
        let save = saver(db.clone(), Settings::default());

        let prefs = RailPrefs::new(RailMode::ClickPin, vec![Route::Manga, Route::Discover]);
        cx.update(|cx| save(&prefs, cx)).await;
        drop(db);

        let reopened = runtime.block_on(Db::open(&path)).unwrap();
        let rows = runtime.block_on(reopened.settings().rows()).unwrap();
        let (settings, errors) = Settings::from_rows(rows);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            from_settings(&settings),
            prefs,
            "click-pin and the unpinned items, in order, after reopening the database"
        );
    }

    #[test]
    fn unknown_unpinned_keys_are_skipped() {
        let mut settings = Settings::default();
        settings.appearance.sidebar_unpinned = vec!["retired".to_owned(), "manga".to_owned()];

        assert_eq!(
            from_settings(&settings),
            RailPrefs::new(RailMode::Hover, vec![Route::Manga])
        );
    }
}
