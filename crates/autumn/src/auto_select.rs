//! The composition-root side of Settings → Torrents (#228).
//!
//! Saving writes only the `auto_select` row; no other tab is touched.

use std::cell::RefCell;

use anyhow::Context as _;
use db::Db;
use settings::Settings;
use ui_screens::AutoSelectServices;

/// The settings row the Torrents tab owns.
const AUTO_SELECT_TAB: &str = "auto_select";

/// A save callback for the Torrents tab. It writes only the auto-select row,
/// so a save here never overwrites another tab.
pub(crate) fn saver(db: Db, settings: Settings) -> AutoSelectServices {
    let settings = RefCell::new(settings);
    AutoSelectServices::new(move |auto_select, cx| {
        let rows = {
            let mut settings = settings.borrow_mut();
            settings.auto_select = auto_select.clone();
            settings.to_rows().map(|rows| {
                rows.into_iter()
                    .filter(|(tab, _)| tab == AUTO_SELECT_TAB)
                    .collect::<Vec<_>>()
            })
        };
        let db = db.clone();
        let save = ui_state::bridge::run(cx, async move {
            db.settings()
                .save(&rows.context("could not encode the auto-select settings")?)
                .await
                .context("could not save the auto-select settings")
        });
        cx.background_executor().spawn(async move {
            match save.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(error = %format_args!("{error:#}"), "auto-select settings not saved");
                }
                Err(error) => {
                    tracing::warn!(%error, "auto-select settings not saved");
                }
            }
        })
    })
}
