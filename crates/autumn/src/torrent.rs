//! Wires the torrent controls on Settings → Storage to the composition root
//! (#229) and the engine's torrent session (#217).

use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc};

use anyhow::Context as _;
use engine::TorrentStreams;
use settings::Settings;
use torrent::TorrentCache;
use ui_screens::{TorrentCacheAction, TorrentServices};

/// The settings row the torrent controls live in.
const TORRENT_TAB: &str = "torrent";

/// Builds the save callback and the cache request behind the Storage tab.
///
/// The cache request reads the folder from the latest saved settings, so a
/// new download folder is measured and cleared without a relaunch. A save
/// restarts the torrent session for the new folder or upload toggle once no
/// stream plays, and Clear never deletes a live stream's files.
pub(crate) fn services(
    db: db::Db,
    settings: Settings,
    default_folder: PathBuf,
    streams: Arc<TorrentStreams>,
) -> TorrentServices {
    let settings = Rc::new(RefCell::new(settings));
    let saved = Rc::clone(&settings);
    let cache_default = default_folder.clone();
    let save_default = default_folder.clone();
    let save_streams = Arc::clone(&streams);
    TorrentServices::new(
        move |torrent, cx| {
            let rows = {
                let mut settings = saved.borrow_mut();
                settings.torrent = torrent.clone();
                settings.to_rows().map(|rows| {
                    rows.into_iter()
                        .filter(|(tab, _)| tab == TORRENT_TAB)
                        .collect::<Vec<_>>()
                })
            };
            let db = db.clone();
            let (streams, default_root) = (Arc::clone(&save_streams), save_default.clone());
            let torrent = torrent.clone();
            let save = ui_state::bridge::run(cx, async move {
                db.settings()
                    .save(&rows.context("could not encode the torrent settings")?)
                    .await
                    .context("could not save the torrent settings")?;
                streams.apply_storage(&torrent, default_root).await;
                Ok::<_, anyhow::Error>(())
            });
            cx.background_executor().spawn(async move {
                if let Err(error) = save.await {
                    tracing::warn!(error = %format_args!("{error:#}"), "torrent settings not saved");
                }
            })
        },
        move |action, cx| {
            let folder = settings
                .borrow()
                .torrent
                .download_folder
                .clone()
                .unwrap_or_else(|| cache_default.clone());
            let cache = TorrentCache::new(folder);
            let streams = Arc::clone(&streams);
            let task = ui_state::bridge::run(cx, async move {
                if action == TorrentCacheAction::Clear {
                    cache.clear(&streams.active()).await?;
                }
                cache.size_bytes().await
            });
            cx.background_executor().spawn(async move {
                let result = task
                    .await
                    .map_err(anyhow::Error::from)
                    .and_then(|result| result.map_err(Into::into));
                if let Err(error) = &result {
                    tracing::warn!(?action, %error, "torrent cache request failed");
                }
                result
            })
        },
        default_folder,
    )
}
