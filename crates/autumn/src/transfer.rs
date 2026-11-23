//! Engine-side list transfer: plan a reviewed preview and run it on the
//! Tokio bridge. Every AniList call goes through the shared client, so the
//! transfer waits behind the same rate limiter as the rest of the app.

use auth::AniListAuth;
use db::Db;
use domain::{AppEvent, MediaKind, TransferDirection, TransferPreview};
use gpui_kit::{App, Task};
use metadata::AniList;
use tracker::{AniListPlatform, Local, Platform, collection_entries, plan_transfer, run_transfer};
use ui_state::{bridge, events::Sender, transfer::TransferCommand};

/// Owns the two platforms a transfer needs and publishes its progress.
#[derive(Clone)]
pub(crate) struct Transfers {
    db: Db,
    anilist: AniList,
    auth: AniListAuth,
    events: Sender,
}

impl Transfers {
    /// A service sharing the app's database, AniList client and auth session.
    pub(crate) fn new(db: Db, anilist: AniList, auth: AniListAuth, events: Sender) -> Self {
        Self {
            db,
            anilist,
            auth,
            events,
        }
    }

    /// Runs one UI command in the background. A newer request cancels the
    /// older bridge task through the returned [`Task`].
    pub(crate) fn request(&self, command: TransferCommand, cx: &mut App) -> Task<()> {
        // Only a completed run clears the review UI; planning a preview must
        // leave the freshly opened dialog alone.
        let is_run = matches!(command, TransferCommand::Run(_));
        let service = self.clone();
        let task = bridge::run(cx, async move {
            match command {
                TransferCommand::Preview(direction) => {
                    service.publish_preview(direction, None).await
                }
                TransferCommand::Run(preview) => service.run(preview).await,
                _ => Ok(()),
            }
        });
        let service = self.clone();
        cx.background_executor().spawn(async move {
            match task.await {
                Ok(Ok(())) if is_run => {
                    let _ = service.events.send(AppEvent::TransferFinished);
                }
                Ok(Err(message)) => {
                    let _ = service.events.send(AppEvent::TransferError(message));
                }
                // A successful preview or a cancelled request needs no event.
                Ok(Ok(())) | Err(_) => {}
            }
        })
    }

    /// Reads both platforms and publishes a preview. `name` avoids a second
    /// Viewer request when the account state already has the user's name.
    ///
    /// # Errors
    ///
    /// A displayable message when the token or either collection cannot be
    /// read. No entries are written by this step.
    pub(crate) async fn publish_preview(
        &self,
        direction: TransferDirection,
        name: Option<String>,
    ) -> Result<(), String> {
        let (anilist, local) = self.platforms(name).await?;
        let kind = MediaKind::Anime;
        let (source, destination) = match direction {
            TransferDirection::LocalToAniList => (
                local
                    .collection(kind)
                    .await
                    .map_err(|error| error.to_string())?,
                anilist
                    .collection(kind)
                    .await
                    .map_err(|error| error.to_string())?,
            ),
            TransferDirection::AniListToLocal => (
                anilist
                    .collection(kind)
                    .await
                    .map_err(|error| error.to_string())?,
                local
                    .collection(kind)
                    .await
                    .map_err(|error| error.to_string())?,
            ),
            _ => return Err("unsupported transfer direction".to_owned()),
        };
        let preview = plan_transfer(
            kind,
            direction,
            &collection_entries(&source),
            &collection_entries(&destination),
        );
        let _ = self.events.send(AppEvent::TransferPreview(preview));
        Ok(())
    }

    /// Writes the reviewed preview, reporting one progress event per entry.
    ///
    /// # Errors
    ///
    /// A displayable message when the token or a write fails. Entries written
    /// before the failure stay on the destination; re-planning skips them.
    pub(crate) async fn run(&self, preview: TransferPreview) -> Result<(), String> {
        let events = self.events.clone();
        let progress = |step| {
            let _ = events.send(AppEvent::TransferProgress(step));
        };
        match preview.direction {
            TransferDirection::LocalToAniList => {
                // AniList calls share the net client's rate limiter.
                let (anilist, _) = self.platforms(None).await?;
                run_transfer(&preview, &anilist, progress).await
            }
            TransferDirection::AniListToLocal => {
                // The preview already carries the AniList entries. Sign-out
                // has removed the token, so only the Local platform is used.
                let local = Local::new(self.db.clone());
                run_transfer(&preview, &local, progress).await
            }
            _ => return Err("unsupported transfer direction".to_owned()),
        }
        .map_err(|error| error.to_string())
    }

    async fn platforms(&self, name: Option<String>) -> Result<(AniListPlatform, Local), String> {
        let token = self
            .auth
            .token()
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "AniList is not connected".to_owned())?;
        let name = match name {
            Some(name) => name,
            None => {
                self.anilist
                    .viewer(&token)
                    .await
                    .map_err(|error| error.to_string())?
                    .name
            }
        };
        Ok((
            AniListPlatform::new(self.db.clone(), self.anilist.clone(), token, name),
            Local::new(self.db.clone()),
        ))
    }
}
