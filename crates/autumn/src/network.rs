//! The composition-root side of Settings → Network (#210 / ADR-0010).
//!
//! Saving writes only the `network` row; live reload and torrent restart are
//! applied by the engine (ARCHITECTURE §6).

use std::cell::RefCell;

use anyhow::Context as _;
use db::Db;
use settings::Settings;
use ui_screens::NetworkServices;

/// The settings row the Network tab owns.
const NETWORK_TAB: &str = "network";

/// A save callback for the Network tab. It writes only the Network row, so a
/// save here never overwrites another tab.
pub(crate) fn saver(db: Db, settings: Settings) -> NetworkServices {
    let settings = RefCell::new(settings);
    NetworkServices::new(move |network, cx| {
        let rows = {
            let mut settings = settings.borrow_mut();
            settings.network = network.clone();
            settings.to_rows().map(|rows| {
                rows.into_iter()
                    .filter(|(tab, _)| tab == NETWORK_TAB)
                    .collect::<Vec<_>>()
            })
        };
        let db = db.clone();
        let save = ui_state::bridge::run(cx, async move {
            db.settings()
                .save(&rows.context("could not encode the Network settings")?)
                .await
                .context("could not save the Network settings")
        });
        cx.background_executor().spawn(async move {
            match save.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(error = %format_args!("{error:#}"), "Network settings not saved");
                }
                Err(error) => {
                    tracing::warn!(%error, "Network settings not saved");
                }
            }
        })
    })
}
