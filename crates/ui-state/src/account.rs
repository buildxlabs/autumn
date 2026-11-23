//! The AniList account state received through the app event pump.

use std::time::SystemTime;

use domain::{ANILIST_RELOGIN_WINDOW, AniListAccountState, AppEvent};
use gpui_kit::{Context, EventEmitter, Task};

/// A command from the Account settings tab to the owner of the token.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AccountCommand {
    /// Store this pasted token and connect AniList.
    Login(String),
    /// Delete the token and return to the account-free platform.
    SignOut,
}

/// What the shell knows about the AniList account. It never holds a token.
#[derive(Debug)]
pub struct AccountStore {
    state: AniListAccountState,
    error: Option<String>,
    /// Moves an active account into the prompt and read-only states on time.
    expiry_task: Option<Task<()>>,
}

impl AccountStore {
    /// A store already holding `state`, for restored accounts and tests.
    #[must_use]
    pub fn from_state(state: AniListAccountState) -> Self {
        Self {
            state,
            error: None,
            expiry_task: None,
        }
    }
}

impl Default for AccountStore {
    fn default() -> Self {
        Self {
            state: AniListAccountState::LoggedOut,
            error: None,
            expiry_task: None,
        }
    }
}

impl EventEmitter<AccountCommand> for AccountStore {}

impl AccountStore {
    /// The account state to render.
    #[must_use]
    pub fn state(&self) -> &AniListAccountState {
        &self.state
    }

    /// The last user-facing error, if an account action failed.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The account name, when AniList supplied one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match &self.state {
            AniListAccountState::Active { name, .. }
            | AniListAccountState::Expired {
                name: Some(name), ..
            } => Some(name.as_str()),
            _ => None,
        }
    }

    /// The avatar URL to display, when there is one.
    #[must_use]
    pub fn avatar(&self) -> Option<&str> {
        match &self.state {
            AniListAccountState::Active { avatar, .. }
            | AniListAccountState::Expired { avatar, .. } => avatar.as_deref(),
            _ => None,
        }
    }

    /// Whether the re-login prompt should be shown.
    #[must_use]
    pub fn expiring(&self) -> bool {
        matches!(
            self.state,
            AniListAccountState::Expired { .. }
                | AniListAccountState::Active { expiring: true, .. }
        )
    }

    /// Whether the token is past its lifetime.
    #[must_use]
    pub fn expired(&self) -> bool {
        matches!(self.state, AniListAccountState::Expired { .. })
    }

    /// Whether an account is connected.
    #[must_use]
    pub fn is_logged_in(&self) -> bool {
        !matches!(self.state, AniListAccountState::LoggedOut)
    }

    /// Forward a command to the owner of the token.
    pub fn request(command: AccountCommand, cx: &mut Context<'_, Self>) {
        cx.emit(command);
    }

    /// Apply an event from the foreground pump and notify observing views.
    pub fn apply(&mut self, event: AppEvent, cx: &mut Context<'_, Self>) {
        match event {
            AppEvent::AccountChanged(state) => {
                if let AniListAccountState::Active { expires_at, .. } = state {
                    self.schedule_expiry(expires_at, cx);
                } else {
                    self.expiry_task = None;
                }
                self.state = state;
                self.error = None;
                cx.notify();
            }
            AppEvent::AccountError(message) => {
                self.error = Some(message);
                cx.notify();
            }
            _ => {}
        }
    }

    /// Prompts seven days before expiry, then switches to read-only when the
    /// token runs out, even if the app stays open for a year.
    fn schedule_expiry(&mut self, expires_at: SystemTime, cx: &mut Context<'_, Self>) {
        self.expiry_task = None;
        let store = cx.entity().downgrade();
        self.expiry_task = Some(cx.spawn(async move |_, cx| {
            let prompt_at = expires_at
                .checked_sub(ANILIST_RELOGIN_WINDOW)
                .unwrap_or(expires_at);
            wait_until(prompt_at, cx).await;
            let _ = store.update(cx, |store, cx| {
                if let AniListAccountState::Active { expiring, .. } = &mut store.state {
                    *expiring = true;
                    cx.notify();
                }
            });
            wait_until(expires_at, cx).await;
            let _ = store.update(cx, |store, cx| {
                if let AniListAccountState::Active { name, avatar, .. } = &store.state {
                    let (name, avatar) = (name.clone(), avatar.clone());
                    store.state = AniListAccountState::Expired {
                        name: Some(name),
                        avatar,
                    };
                    cx.notify();
                }
            });
        }));
    }
}

async fn wait_until(deadline: SystemTime, cx: &mut gpui_kit::AsyncApp) {
    if let Ok(left) = deadline.duration_since(SystemTime::now()) {
        cx.background_executor().timer(left).await;
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use domain::{AniListAccountState, AppEvent};
    use gpui_kit::{AppContext, TestAppContext};

    use super::AccountStore;

    #[gpui_kit::test]
    fn account_events_replace_the_account_and_clear_old_errors(cx: &mut TestAppContext) {
        let store = cx.new(|_| AccountStore::default());
        let notified = Rc::new(Cell::new(0));
        cx.update(|cx| {
            let notified = Rc::clone(&notified);
            cx.observe(&store, move |_, _| notified.set(notified.get() + 1))
                .detach();
        });

        store.update(cx, |store, cx| {
            store.apply(AppEvent::AccountError("no keychain".to_owned()), cx);
        });
        assert_eq!(
            store.read_with(cx, |store, _| store.error().map(str::to_owned)),
            Some("no keychain".to_owned())
        );

        store.update(cx, |store, cx| {
            store.apply(AppEvent::AccountChanged(AniListAccountState::LoggedOut), cx);
        });
        assert_eq!(
            store.read_with(cx, |store, _| store.state().clone()),
            AniListAccountState::LoggedOut
        );
        assert_eq!(
            store.read_with(cx, |store, _| store.error().map(str::to_owned)),
            None
        );
        assert_eq!(notified.get(), 2);
    }
}
