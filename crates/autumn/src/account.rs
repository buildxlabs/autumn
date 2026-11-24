//! Engine-side account commands: call the auth service, switch the active
//! platform, review a list transfer and publish the result as UI events. No
//! token reaches a log or a store other than the in-memory AniList platform.

use std::{
    sync::{Arc, Mutex},
    time::SystemTime,
};

use auth::AniListAuth;
use db::Db;
use domain::{
    AniListAccountState, AppEvent, MediaId, TransferChoice, TransferDirection, TransferPreview,
    TransferProgress, TransferState,
};
use gpui_kit::{App, Task};
use metadata::AniList;
use ui_state::{AniListHandle, PlatformGateway, account::AccountCommand, events::Sender};

/// The account name to show for a connected account.
pub(crate) fn name(state: &AniListAccountState) -> String {
    match state {
        AniListAccountState::Active { name, .. }
        | AniListAccountState::Expired {
            name: Some(name), ..
        } => name.clone(),
        AniListAccountState::Expired { name: None, .. } | AniListAccountState::LoggedOut => {
            "AniList".to_owned()
        }
        _ => "AniList".to_owned(),
    }
}

/// Builds the Account tab's command handler.
///
/// Each command runs on the Tokio bridge; its result (or a user-facing
/// error) is sent back through the event pump. A successful login makes
/// AniList the active platform and offers a Local → AniList transfer. Sign-out
/// first offers the AniList → Local transfer while the token still works, and
/// only deletes the token once the user confirms or dismisses it (ADR 0004).
pub(crate) fn request(
    auth: AniListAuth,
    gateway: PlatformGateway,
    db: Db,
    anilist: AniList,
    events: Sender,
) -> impl Fn(AccountCommand, &mut App) -> Task<()> {
    // A sign-out waits for the transfer review. Remember it so confirming or
    // dismissing that one preview finishes the sign-out.
    let pending_sign_out = Arc::new(Mutex::new(None::<TransferDirection>));
    move |command, cx| {
        let auth = auth.clone();
        let gateway = gateway.clone();
        let db = db.clone();
        let anilist = anilist.clone();
        let events = events.clone();
        let pending_sign_out = Arc::clone(&pending_sign_out);
        let events_for_result = events.clone();
        let task = ui_state::bridge::run(cx, async move {
            match command {
                AccountCommand::Login(token) => {
                    login(
                        &auth,
                        &gateway,
                        &db,
                        &anilist,
                        &events,
                        &pending_sign_out,
                        token,
                    )
                    .await
                }
                AccountCommand::SignOut => {
                    sign_out(&auth, &gateway, &events, &pending_sign_out).await
                }
                AccountCommand::TransferPreview { direction } => {
                    guard_pending(&pending_sign_out, direction)?;
                    offer(&gateway, direction, &events).await?;
                    Ok(None)
                }
                AccountCommand::TransferConfirm { direction, choices } => {
                    guard_pending(&pending_sign_out, direction)?;
                    confirm(
                        &gateway,
                        &events,
                        &auth,
                        &pending_sign_out,
                        direction,
                        choices,
                    )
                    .await
                }
                AccountCommand::TransferCancel { direction } => {
                    guard_pending(&pending_sign_out, direction)?;
                    let finished =
                        finish_sign_out(direction, &auth, &gateway, &pending_sign_out).await?;
                    let _ = events.send(AppEvent::TransferPreview(None));
                    Ok(finished)
                }
                _ => Ok(None),
            }
        });
        cx.background_executor().spawn(async move {
            match task.await {
                Ok(Ok(Some(state))) => {
                    let _ = events_for_result.send(AppEvent::AccountChanged(state));
                }
                // A newer command replaced this one, or no account event is due.
                Ok(Ok(None)) | Err(_) => {}
                Ok(Err(message)) => {
                    // Auth and gateway errors never carry a token.
                    let _ = events_for_result.send(AppEvent::AccountError(message));
                }
            }
        })
    }
}

type AccountResult = Result<Option<AniListAccountState>, String>;

/// A sign-out that is waiting on its transfer review owns the preview. An
/// unrelated transfer must not replace it and leave the sign-out unfinished.
fn guard_pending(
    pending: &Mutex<Option<TransferDirection>>,
    direction: TransferDirection,
) -> Result<(), String> {
    let blocked = pending
        .lock()
        .map_err(|_| "account state unavailable")?
        .is_some_and(|waiting| waiting != direction);
    if blocked {
        return Err("finish the pending sign-out transfer first".to_owned());
    }
    Ok(())
}

async fn login(
    auth: &AniListAuth,
    gateway: &PlatformGateway,
    db: &Db,
    anilist: &AniList,
    events: &Sender,
    pending: &Mutex<Option<TransferDirection>>,
    token: String,
) -> AccountResult {
    let state = auth
        .login(&token, SystemTime::now())
        .await
        .map_err(|error| error.to_string())?;
    if matches!(state, AniListAccountState::LoggedOut) {
        gateway.sign_out();
    } else {
        // A keychain read failure must reach the UI, not leave the app on
        // Local with a supposedly connected account.
        match auth.token().await.map_err(|error| error.to_string())? {
            Some(connected_token) => gateway.connect_anilist(AniListHandle::new(
                db.clone(),
                anilist.clone(),
                connected_token,
                name(&state),
            )),
            None => gateway.sign_out(),
        }
    }
    *pending.lock().map_err(|_| "account state unavailable")? = None;
    // Publish the account before the preview: a failing preview must not
    // leave the connected account invisible.
    let _ = events.send(AppEvent::AccountChanged(state));
    offer(gateway, TransferDirection::ToAniList, events).await?;
    Ok(None)
}

async fn sign_out(
    auth: &AniListAuth,
    gateway: &PlatformGateway,
    events: &Sender,
    pending: &Mutex<Option<TransferDirection>>,
) -> AccountResult {
    // A failed preview must never strand the user on a signed-in account:
    // sign out without offering a transfer instead.
    let preview = if gateway.can_transfer() {
        match gateway.transfer_preview(TransferDirection::ToLocal).await {
            Ok(preview) => (!preview.is_empty()).then_some(preview),
            Err(error) => {
                let _ = events.send(AppEvent::AccountError(error.to_string()));
                None
            }
        }
    } else {
        None
    };
    if let Some(preview) = preview {
        *pending.lock().map_err(|_| "account state unavailable")? =
            Some(TransferDirection::ToLocal);
        let _ = events.send(AppEvent::TransferPreview(Some(preview)));
        return Ok(None);
    }
    auth.sign_out().await.map_err(|error| error.to_string())?;
    gateway.sign_out();
    Ok(None)
}

async fn confirm(
    gateway: &PlatformGateway,
    events: &Sender,
    auth: &AniListAuth,
    pending: &Mutex<Option<TransferDirection>>,
    direction: TransferDirection,
    choices: Vec<(MediaId, TransferChoice)>,
) -> AccountResult {
    // Rebuild against current platform state so an interrupted run continues
    // from its saved checkpoint with the reviewed decisions.
    let mut preview = gateway
        .transfer_preview(direction)
        .await
        .map_err(|error| error.to_string())?;
    apply_choices(&mut preview, &choices);
    let progress_events = events.clone();
    let failed = Arc::new(Mutex::new(None::<TransferProgress>));
    let seen = Arc::clone(&failed);
    let result = gateway
        .transfer_apply(direction, &preview, |update| {
            let _ = progress_events.send(AppEvent::TransferProgress(update.clone()));
            if let Ok(mut saved) = seen.lock() {
                *saved = Some(update);
            }
        })
        .await;
    if let Err(error) = result {
        let (done, total) = failed
            .lock()
            .ok()
            .and_then(|saved| {
                saved
                    .as_ref()
                    .map(|progress| (progress.done, progress.total))
            })
            .unwrap_or((0, 0));
        // The checkpoint already holds the completed entries, so a retry must
        // review only what is left rather than the stale plan on screen.
        if let Ok(mut remaining) = gateway.transfer_preview(direction).await {
            apply_choices(&mut remaining, &choices);
            let refreshed = (!remaining.is_empty()).then_some(remaining);
            let _ = events.send(AppEvent::TransferPreview(refreshed));
        }
        let _ = events.send(AppEvent::TransferProgress(TransferProgress {
            direction,
            done,
            total,
            state: TransferState::Failed(error.to_string()),
        }));
        return Err(error.to_string());
    }
    // Keep the preview until sign-out really succeeded: a keychain failure
    // must leave the user a way to finish or cancel it.
    let finished = finish_sign_out(direction, auth, gateway, pending).await?;
    let _ = events.send(AppEvent::TransferPreview(None));
    if let Some(last) = failed.lock().ok().and_then(|saved| saved.clone()) {
        let _ = events.send(AppEvent::TransferProgress(last));
    }
    Ok(finished)
}

fn apply_choices(preview: &mut TransferPreview, choices: &[(MediaId, TransferChoice)]) {
    for item in &mut preview.items {
        if let Some((_, choice)) = choices.iter().find(|(media, _)| *media == item.media) {
            item.choice = *choice;
        }
    }
}

/// Publishes a preview when there is something to review.
async fn offer(
    gateway: &PlatformGateway,
    direction: TransferDirection,
    events: &Sender,
) -> Result<(), String> {
    if !gateway.can_transfer() {
        return Ok(());
    }
    let preview = gateway
        .transfer_preview(direction)
        .await
        .map_err(|error| error.to_string())?;
    let preview = (!preview.is_empty()).then_some(preview);
    let _ = events.send(AppEvent::TransferPreview(preview));
    Ok(())
}

/// Completes a sign-out that was waiting on this transfer's review.
async fn finish_sign_out(
    direction: TransferDirection,
    auth: &AniListAuth,
    gateway: &PlatformGateway,
    pending: &Mutex<Option<TransferDirection>>,
) -> AccountResult {
    let waiting = pending
        .lock()
        .map_err(|_| "account state unavailable")?
        .is_some_and(|pending| pending == direction);
    if !waiting {
        return Ok(None);
    }
    auth.sign_out().await.map_err(|error| error.to_string())?;
    gateway.sign_out();
    *pending.lock().map_err(|_| "account state unavailable")? = None;
    Ok(Some(AniListAccountState::LoggedOut))
}
