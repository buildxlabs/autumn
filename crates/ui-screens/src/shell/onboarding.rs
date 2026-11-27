//! The first-run wizard (#138, ROADMAP Phase 2).
//!
//! The wizard holds only the current step. Every choice is written through
//! the same callbacks the Settings tabs use, so its starting values are the
//! saved settings and Skip simply marks them complete.

use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    TestSupportExt,
    component::{ActiveTheme, button::Button, label::Label},
    div,
};
use settings::PlayerChoice;
use ui_state::{Route, SettingsTab};

use super::{AppShell, PlayerOption};

/// The three steps, in the order the wizard shows them.
const STEPS: [&str; 3] = ["Library folders", "Account", "Player"];

impl AppShell {
    /// Saves the General tab after the wizard or a Settings change.
    pub(super) fn write_general(&mut self, cx: &mut Context<'_, Self>) {
        let task = self.general_services.save(&self.general, cx);
        self.saving = Some(cx.spawn(async move |_, _| {
            task.await;
        }));
        cx.notify();
    }

    /// Completes the wizard without changing any other setting. A fresh
    /// install therefore lands on Home with all the Settings defaults.
    pub(super) fn complete_onboarding(&mut self, cx: &mut Context<'_, Self>) {
        self.general.complete();
        self.write_general(cx);
        self.navigate(Route::Home, cx);
    }

    /// Settings → General: reopen the wizard with the current choices shown.
    pub(super) fn reopen_onboarding(&mut self, cx: &mut Context<'_, Self>) {
        self.general.reopen();
        self.write_general(cx);
        self.onboarding_step = 0;
        self.navigate(Route::Onboarding, cx);
    }

    pub(super) fn onboarding_next(&mut self, cx: &mut Context<'_, Self>) {
        self.onboarding_step = (self.onboarding_step + 1).min(STEPS.len());
        cx.notify();
    }

    pub(super) fn onboarding_back(&mut self, cx: &mut Context<'_, Self>) {
        self.onboarding_step = self.onboarding_step.saturating_sub(1);
        cx.notify();
    }

    /// Picks folders and starts the first scan once they have been saved.
    pub(super) fn onboarding_pick_folders(&mut self, cx: &mut Context<'_, Self>) {
        self.pick_library_folders(true, cx);
    }

    /// The Playback tab's Auto choice is the suggested default.
    pub(super) fn onboarding_auto_player(&mut self, cx: &mut Context<'_, Self>) {
        self.update_playback(|settings| settings.player = PlayerChoice::Auto, cx);
    }

    pub(super) fn onboarding_player(&mut self, option: &PlayerOption, cx: &mut Context<'_, Self>) {
        let kind = option.kind;
        self.update_playback(
            |settings| {
                settings.player = PlayerChoice::Installed { kind };
            },
            cx,
        );
    }

    /// Account settings own the login flow; the wizard links to that tab.
    pub(super) fn onboarding_anilist(&mut self, cx: &mut Context<'_, Self>) {
        self.navigate(
            Route::Settings {
                tab: SettingsTab::Account,
            },
            cx,
        );
    }
}

/// Renders the current step. `render` stays pure: clicks update the shell.
pub(super) fn render(shell: &AppShell, cx: &mut Context<'_, AppShell>) -> impl IntoElement {
    let step = shell.onboarding_step.min(STEPS.len() - 1);
    let shell_handle = cx.entity().downgrade();
    let body = match step {
        0 => library_step(shell, &shell_handle),
        1 => account_step(
            shell.account.read(cx).is_logged_in(),
            shell.keychain_available,
            &shell_handle,
        ),
        _ => player_step(shell, &shell_handle),
    };
    div()
        .id("onboarding-view")
        .test_support()
        .flex()
        .flex_col()
        .gap_6()
        .p_8()
        .child(div().text_2xl().child("Welcome to Autumn"))
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().colors.muted_foreground)
                .child(format!(
                    "Step {} of 3 — {}",
                    step + 1,
                    STEPS.get(step).copied().unwrap_or("Setup")
                )),
        )
        .child(body)
        .child(
            div()
                .flex()
                .gap_3()
                .child(Button::new("onboarding-back").label("Back").on_click({
                    let shell = shell_handle.clone();
                    move |_, _, cx| {
                        let _ = shell.update(cx, AppShell::onboarding_back);
                    }
                }))
                .child(Button::new("onboarding-skip").label("Skip").on_click({
                    let shell = shell_handle.clone();
                    move |_, _, cx| {
                        let _ = shell.update(cx, AppShell::complete_onboarding);
                    }
                }))
                .child(
                    Button::new("onboarding-next")
                        .label(if step == STEPS.len() - 1 {
                            "Finish"
                        } else {
                            "Continue"
                        })
                        .on_click({
                            move |_, _, cx| {
                                let _ = shell_handle.update(cx, |shell, cx| {
                                    if shell.onboarding_step == STEPS.len() - 1 {
                                        shell.complete_onboarding(cx);
                                    } else {
                                        shell.onboarding_next(cx);
                                    }
                                });
                            }
                        }),
                ),
        )
}

/// Folders are written by the same picker the Library tab uses.
fn library_step(shell: &AppShell, handle: &gpui_kit::WeakEntity<AppShell>) -> AnyElement {
    let folders: Vec<String> = shell
        .library
        .folders
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(Label::new(
            "Choose the folders Autumn should scan for anime.",
        ))
        .child(div().text_sm().child(if folders.is_empty() {
            "No folders chosen yet.".to_owned()
        } else {
            format!("Chosen: {}", folders.join(", "))
        }))
        .child(
            Button::new("onboarding-choose-folders")
                .label("Choose folders")
                .on_click({
                    let shell = handle.clone();
                    move |_, _, cx| {
                        let _ = shell.update(cx, AppShell::onboarding_pick_folders);
                    }
                }),
        )
        .into_any_element()
}

/// Local is the default; AniList uses the Account tab's existing login.
#[expect(
    clippy::fn_params_excessive_bools,
    reason = "the two flags are read from the account store and startup, not alternative modes"
)]
fn account_step(
    logged_in: bool,
    keychain_available: bool,
    handle: &gpui_kit::WeakEntity<AppShell>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(Label::new(
            "Track shows locally, or connect your AniList account.",
        ))
        .child(div().text_sm().child(if logged_in {
            "AniList is connected. You can keep it or stay account-free."
        } else {
            "Local (no account) is the default and works fully offline."
        }))
        .child(
            Button::new("onboarding-anilist")
                .label("Connect AniList")
                .on_click({
                    let shell = handle.clone();
                    move |_, _, cx| {
                        let _ = shell.update(cx, AppShell::onboarding_anilist);
                    }
                }),
        )
        .child(if keychain_available {
            div().into_any_element()
        } else {
            div()
                .text_sm()
                .child("AniList login needs an OS keychain; none is available here.")
                .into_any_element()
        })
        .into_any_element()
}

/// Detected players, the suggested Auto choice, or a custom executable.
fn player_step(shell: &AppShell, handle: &gpui_kit::WeakEntity<AppShell>) -> AnyElement {
    let auto = matches!(shell.playback.player, PlayerChoice::Auto);
    let mut column = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(Label::new("Choose the player Autumn should launch."))
        .child(
            Button::new("onboarding-auto-player")
                .label(if auto {
                    "✓ Suggested default (Auto)"
                } else {
                    "Suggested default (Auto)"
                })
                .on_click({
                    let shell = handle.clone();
                    move |_, _, cx| {
                        let _ = shell.update(cx, AppShell::onboarding_auto_player);
                    }
                }),
        );
    for (index, option) in shell.players.iter().enumerate() {
        let selected = shell.playback.player == PlayerChoice::Installed { kind: option.kind };
        let label: SharedString =
            format!("{} {}", if selected { "✓" } else { " " }, option.label).into();
        let option = option.clone();
        column = column.child(
            Button::new(SharedString::from(format!("onboarding-player-{index}")))
                .label(label)
                .on_click({
                    let shell = handle.clone();
                    move |_, _, cx| {
                        let _ = shell.update(cx, |shell, cx| shell.onboarding_player(&option, cx));
                    }
                }),
        );
    }
    column = column.child(
        Button::new("onboarding-custom-player")
            .label("Choose a player executable")
            .on_click({
                let shell = handle.clone();
                move |_, _, cx| {
                    let _ = shell.update(cx, AppShell::pick_custom_player);
                }
            }),
    );
    if shell.players.is_empty() {
        column = column
            .child(
                div()
                    .text_sm()
                    .child("No player was detected. You can install mpv and continue anyway."),
            )
            .child(
                Button::new("onboarding-mpv-download")
                    .label("Download mpv")
                    .on_click(|_, _, cx| {
                        cx.open_url("https://mpv.io/installation/");
                    }),
            );
    }
    column.into_any_element()
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, sync::Arc};

    use gpui_kit::{
        App, AppContext, Bounds, TestAppContext, WindowBounds, WindowOptions, base::Root, point,
        px, size, test::TestWindowExt,
    };
    use settings::{GeneralSettings, LibrarySettings, PlaybackSettings};
    use tokio::runtime::Builder;
    use ui_state::{
        LibraryExplorerStore, LibraryStore, Route, ScanSummariesStore,
        account::AccountStore,
        bridge::{self, BridgeError},
        entry_store::EntryStore,
        fetch_status::FetchStatus,
        home_store::HomeStore,
        playback_store::PlaybackStore,
        test_support::pending_services,
    };

    use crate::{
        DiscoverUpdate,
        shell::{AppShell, GeneralServices, LibraryServices, PlaybackServices, RailPrefs},
    };

    use super::*;

    #[test]
    fn steps_are_in_setup_order() {
        assert_eq!(STEPS, ["Library folders", "Account", "Player"]);
    }

    #[gpui_kit::test]
    #[expect(
        clippy::too_many_lines,
        clippy::unused_async,
        clippy::indexing_slicing,
        reason = "one screen test builds the shell and checks the wizard, Skip and Run setup again"
    )]
    async fn first_run_shows_the_wizard_and_skip_writes_only_the_flag(cx: &mut TestAppContext) {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a test runtime");
        let _guard = runtime.enter();
        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let saved: Rc<RefCell<Vec<GeneralSettings>>> = Rc::new(RefCell::new(Vec::new()));
        let recordings = Rc::clone(&saved);
        let general_services = GeneralServices::new(move |general, _| {
            recordings.borrow_mut().push(general.clone());
            gpui_kit::Task::ready(())
        });
        let services = pending_services();
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::init(cx);
                ui_theme::init(cx);
                cx.set_reduce_motion(true);
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(1280.), px(800.)),
                    })),
                    ..WindowOptions::default()
                };
                gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| {
                        AppShell::new(
                            RailPrefs::default(),
                            PlaybackSettings::default(),
                            PlaybackServices::inert(),
                            crate::shell::NetworkSettings::default(),
                            crate::shell::NetworkServices::inert(),
                            LibrarySettings::default(),
                            LibraryServices::inert(),
                            cx.new(|_| LibraryStore::new(Arc::clone(&services))),
                            cx.new(|_| ScanSummariesStore::new(Arc::clone(&services))),
                            cx.new(|_| LibraryExplorerStore::new(Arc::clone(&services))),
                            cx.new(|_| FetchStatus::default()),
                            cx.new(|_| AccountStore::default()),
                            "https://example.test/login".to_owned(),
                            |_, _| gpui_kit::Task::ready(()),
                            |_, _| gpui_kit::Task::ready(Ok(0)),
                            |_, _| gpui_kit::Task::ready(()),
                            Rc::new(|_: Option<String>, _: &App| {
                                gpui_kit::Task::ready(Ok::<_, BridgeError>(
                                    DiscoverUpdate::default(),
                                ))
                            }),
                            cx.new(|_| EntryStore::new(Arc::clone(&services))),
                            cx.new(|_| ui_state::EntryLibraryStore::new(Arc::clone(&services))),
                            cx.new(|_| HomeStore::new(Arc::clone(&services))),
                            cx.new(|_| PlaybackStore::new(Arc::clone(&services))),
                            GeneralSettings::default(),
                            general_services,
                            false,
                            window,
                            cx,
                        )
                    })
                })
            })
            .expect("a test window");
        let window = window.downcast::<Root>().expect("the root window");
        cx.run_until_parked();

        assert_eq!(
            view.read_with(cx, |shell, _| shell.route()),
            Route::Onboarding,
            "a first run starts at the wizard"
        );
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("onboarding-view").is_some());
            assert!(window.try_find("home-view").is_none());
            window.click("onboarding-skip", cx);
        })
        .expect("the wizard");
        cx.run_until_parked();

        assert_eq!(
            view.read_with(cx, |shell, _| shell.route()),
            Route::Home,
            "Skip lands on Home"
        );
        let writes = saved.borrow().clone();
        assert_eq!(
            writes.len(),
            1,
            "the wizard wrote the General tab once: {writes:?}"
        );
        assert!(
            writes[0].onboarding_completed,
            "Skip only marks the wizard done"
        );
        assert_eq!(
            view.read_with(cx, |shell, _| (
                shell.library.clone(),
                shell.playback.clone()
            )),
            (LibrarySettings::default(), PlaybackSettings::default()),
            "the other tabs stay at their prefilled defaults"
        );

        cx.update(|cx| {
            view.update(cx, AppShell::reopen_onboarding);
        });
        assert_eq!(
            view.read_with(cx, |shell, _| shell.route()),
            Route::Onboarding,
            "Run setup again reopens the wizard"
        );
        assert_eq!(
            saved
                .borrow()
                .last()
                .map(|general| general.onboarding_completed),
            Some(false),
            "the flag is cleared through the same save callback"
        );
    }
}
