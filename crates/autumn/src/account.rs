//! Engine-side account commands: call the auth service and publish the
//! result as UI events. No token reaches a log or a store.

use std::time::SystemTime;

use auth::AniListAuth;
use domain::{AniListAccountState, AppEvent};
use gpui_kit::{App, Task};
use ui_state::{account::AccountCommand, events::Sender};

/// Builds the Account tab's command handler.
///
/// Each command runs on the Tokio bridge; its result (or a user-facing
/// error) is sent back through the event pump.
pub(crate) fn request(
    auth: AniListAuth,
    events: Sender,
) -> impl Fn(AccountCommand, &mut App) -> Task<()> {
    move |command, cx| {
        let auth = auth.clone();
        let events = events.clone();
        let task = ui_state::bridge::run(cx, async move {
            match command {
                AccountCommand::Login(token) => {
                    auth.login(&token, SystemTime::now()).await.map(Some)
                }
                AccountCommand::SignOut => auth.sign_out().await.map(|()| None),
                _ => Ok(None),
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
