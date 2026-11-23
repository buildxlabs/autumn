//! Engine-side account commands: call the auth service, switch the active
//! platform and publish the result as UI events. No token reaches a log or a
//! store other than the in-memory AniList platform.

use std::time::SystemTime;

use auth::AniListAuth;
use db::Db;
use domain::{AniListAccountState, AppEvent};
use gpui_kit::{App, Task};
use metadata::AniList;
use tracker::AniListPlatform;
use ui_state::{PlatformGateway, account::AccountCommand, events::Sender};

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
/// AniList the active platform; sign-out restores Local with its entries
/// untouched (ADR 0004).
pub(crate) fn request(
    auth: AniListAuth,
    gateway: PlatformGateway,
    db: Db,
    anilist: AniList,
    events: Sender,
) -> impl Fn(AccountCommand, &mut App) -> Task<()> {
    move |command, cx| {
        let auth = auth.clone();
        let gateway = gateway.clone();
        let db = db.clone();
        let anilist = anilist.clone();
        let events = events.clone();
        let task = ui_state::bridge::run(cx, async move {
            match command {
                AccountCommand::Login(token) => {
                    let state = auth.login(&token, SystemTime::now()).await?;
                    if matches!(state, AniListAccountState::LoggedOut) {
                        gateway.sign_out();
                    } else if let Ok(Some(connected_token)) = auth.token().await {
                        gateway.connect_anilist(AniListPlatform::new(
                            db.clone(),
                            anilist.clone(),
                            connected_token,
                            name(&state),
                        ));
                    }
                    Ok(Some(state))
                }
                AccountCommand::SignOut => {
                    auth.sign_out().await?;
                    gateway.sign_out();
                    Ok(None)
                }
                _ => Ok::<_, auth::AuthError>(None),
            }
        });
        cx.background_executor().spawn(async move {
            match task.await {
                Ok(Ok(Some(state))) => {
                    let _ = events.send(AppEvent::AccountChanged(state));
                }
                Ok(Ok(None)) => {
                    let _ = events.send(AppEvent::AccountChanged(AniListAccountState::LoggedOut));
                }
                Ok(Err(error)) => {
                    // AuthError's display never includes the token.
                    let _ = events.send(AppEvent::AccountError(error.to_string()));
                }
                Err(_) => {
                    // A newer command replaces this one; no event for a cancelled task.
                }
            }
        })
    }
}
