//! Maps the General tab's flag to its settings row.

use std::cell::RefCell;

use anyhow::Context as _;
use db::Db;
use settings::Settings;
use ui_screens::GeneralServices;

/// The settings row the General tab owns.
const GENERAL_TAB: &str = "general";

/// A save callback for the wizard and Settings → General. It writes only the
/// General row, so a save here never overwrites another tab.
pub(crate) fn saver(db: Db, settings: Settings) -> GeneralServices {
    let settings = RefCell::new(settings);
    GeneralServices::new(move |general, cx| {
        let rows = {
            let mut settings = settings.borrow_mut();
            settings.general = general.clone();
            settings.to_rows().map(|rows| {
                rows.into_iter()
                    .filter(|(tab, _)| tab == GENERAL_TAB)
                    .collect::<Vec<_>>()
            })
        };
        let db = db.clone();
        let save = ui_state::bridge::run(cx, async move {
            db.settings()
                .save(&rows.context("could not encode the setting")?)
                .await
                .context("could not save the setting")
        });
        cx.background_executor().spawn(async move {
            if let Err(error) = save.await {
                tracing::warn!(error = %format_args!("{error:#}"), "General settings not saved");
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use db::Db;
    use gpui_kit::TestAppContext;
    use settings::{GeneralSettings, Settings};
    use tokio::runtime::{Builder, Runtime};

    use super::saver;

    fn runtime() -> Runtime {
        Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap()
    }

    #[gpui_kit::test]
    async fn completing_onboarding_survives_a_restart(cx: &mut TestAppContext) {
        let runtime = runtime();
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autumn.db");
        let db = runtime.block_on(Db::open(&path)).unwrap();
        cx.update(|cx| ui_state::bridge::init(cx, runtime.handle().clone()));
        let save = saver(db.clone(), Settings::default());

        let mut general = GeneralSettings::default();
        general.complete();
        cx.update(|cx| save.save(&general, cx)).await;
        drop(db);

        let reopened = runtime.block_on(Db::open(&path)).unwrap();
        let rows = runtime.block_on(reopened.settings().rows()).unwrap();
        let (settings, errors) = Settings::from_rows(rows);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            settings.general.onboarding_completed,
            "the next launch skips the wizard"
        );
    }
}
