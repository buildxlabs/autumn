//! Settings pages and their poster cache controls.

use std::{path::PathBuf, rc::Rc};

use gpui_kit::{
    App, ClickEvent, Context, Display, InteractiveElement, IntoElement, ObjectFit, ParentElement,
    PathPromptOptions, SharedString, StyleRefinement, Styled, StyledImage, Task, TestSupportExt,
    Window,
    component::{
        ActiveTheme, Disableable,
        button::Button,
        input::Input,
        label::Label,
        radio::{Radio, RadioGroup},
        setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
        switch::Switch,
        tab::{Tab, TabBar},
    },
    div, img,
    prelude::FluentBuilder,
    px,
};
use metadata::ODBL_CREDIT;
use settings::{PlaybackKind, PlaybackSettings, PlayerChoice};
use ui_state::{
    Route, SettingsTab,
    account::{AccountCommand, AccountStore},
};

use super::{AppShell, RailMode};

/// A player the Playback tab can offer, with its version in the label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerOption {
    /// The player family to store when this option is selected.
    pub kind: PlaybackKind,
    /// Display name, including the version when detection found one.
    pub label: SharedString,
    /// The executable or Flatpak id this option launches.
    pub path: PathBuf,
}

/// Why testing a player did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the Test button handles every failure it can distinguish"
)]
pub enum PlaybackTestError {
    /// The selected path does not name a usable player.
    Invalid(String),
    /// MPC-HC's web interface has to be turned on in its own options.
    WebInterfaceDisabled,
    /// The player did not open its control interface in time.
    Failed(String),
}

impl std::fmt::Display for PlaybackTestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::Failed(message) => f.write_str(message),
            Self::WebInterfaceDisabled => f.write_str(
                "MPC-HC's web interface is not enabled. Turn it on in MPC-HC's \
                 Options → Playback → Web Interface, then test again.",
            ),
        }
    }
}

type SavePlayback = Rc<dyn Fn(&PlaybackSettings, &mut App) -> Task<()>>;
type DetectPlayers = Rc<dyn Fn(&mut App) -> Task<Vec<PlayerOption>>>;
type ValidatePlayer = Rc<dyn Fn(PathBuf, &mut App) -> Task<Result<PlayerOption, String>>>;
type TestPlayer = Rc<dyn Fn(PlaybackSettings, &mut App) -> Task<Result<(), PlaybackTestError>>>;

/// The composition-root calls behind the Playback tab. Async work is run on
/// the Tokio bridge by the owner; the shell only stores the returned tasks.
#[derive(Clone)]
pub struct PlaybackServices {
    save: SavePlayback,
    detect: DetectPlayers,
    validate: ValidatePlayer,
    test: TestPlayer,
}

impl std::fmt::Debug for PlaybackServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackServices").finish_non_exhaustive()
    }
}

impl PlaybackServices {
    /// Creates the four callbacks the Playback tab needs.
    pub fn new(
        save: impl Fn(&PlaybackSettings, &mut App) -> Task<()> + 'static,
        detect: impl Fn(&mut App) -> Task<Vec<PlayerOption>> + 'static,
        validate: impl Fn(PathBuf, &mut App) -> Task<Result<PlayerOption, String>> + 'static,
        test: impl Fn(PlaybackSettings, &mut App) -> Task<Result<(), PlaybackTestError>> + 'static,
    ) -> Self {
        Self {
            save: Rc::new(save),
            detect: Rc::new(detect),
            validate: Rc::new(validate),
            test: Rc::new(test),
        }
    }

    /// Inert callbacks for embedding the shell without a player backend.
    #[must_use]
    pub fn inert() -> Self {
        Self::new(
            |_, _| Task::ready(()),
            |_| Task::ready(Vec::new()),
            |_, _| Task::ready(Err("no player backend is available".to_owned())),
            |_, _| {
                Task::ready(Err(PlaybackTestError::Failed(
                    "no player backend".to_owned(),
                )))
            },
        )
    }
}

/// The outcome of the last Test click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PlaybackTestStatus {
    Idle,
    Testing,
    Ok(String),
    Failed(String),
}

impl PlaybackTestStatus {
    fn label(&self) -> SharedString {
        match self {
            Self::Idle => "".into(),
            Self::Testing => "Testing the player…".into(),
            Self::Ok(message) | Self::Failed(message) => message.clone().into(),
        }
    }
}

/// A request from Settings → Storage for the on-disk poster cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PosterCacheAction {
    /// Read the current size.
    Measure,
    /// Clear the posters and then read the remaining size.
    Clear,
}

pub(super) type PosterCacheRequest =
    Box<dyn Fn(PosterCacheAction, &mut App) -> Task<gpui_kit::Result<u64>>>;

#[derive(Debug, Clone, Copy)]
pub(super) enum StorageStatus {
    Measuring,
    Clearing,
    Size(u64),
    Failed(PosterCacheAction),
}

impl StorageStatus {
    fn label(self) -> SharedString {
        match self {
            Self::Measuring => "Reading poster cache size…".into(),
            Self::Clearing => "Clearing poster cache…".into(),
            Self::Size(bytes) => size_label(bytes).into(),
            Self::Failed(PosterCacheAction::Measure) => {
                "Could not read the poster cache size.".into()
            }
            Self::Failed(PosterCacheAction::Clear) => "Could not clear the poster cache.".into(),
        }
    }

    fn busy(self) -> bool {
        matches!(self, Self::Measuring | Self::Clearing)
    }
}

impl AppShell {
    /// Detects installed players once on startup. A newer request replaces
    /// the one in flight, and only a completed read updates the choice list.
    pub(super) fn request_players(&mut self, cx: &mut Context<'_, Self>) {
        if self.players_task.is_some() {
            return;
        }
        let task = (self.playback_services.detect)(cx);
        let shell = cx.entity().downgrade();
        self.players_task = Some(cx.spawn(async move |_, cx| {
            let players = task.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.players = players;
                shell.players_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Opens the native file picker and validates the chosen executable
    /// before it becomes the custom player.
    pub(super) fn pick_custom_player(&mut self, cx: &mut Context<'_, Self>) {
        if self.player_pick_task.is_some() {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a player executable".into()),
        });
        let validate = Rc::clone(&self.playback_services.validate);
        let shell = cx.entity().downgrade();
        self.player_pick_error = None;
        self.player_pick_task = Some(cx.spawn(async move |_, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let result = match picked {
                Some(path) => Some(cx.update(|cx| validate(path, cx)).await),
                None => None,
            };
            let _ = shell.update(cx, |shell, cx| {
                shell.player_pick_task = None;
                match result {
                    Some(Ok(option)) => {
                        let path = option.path.clone();
                        shell.update_playback(
                            |settings| {
                                settings.player = PlayerChoice::Custom {
                                    kind: option.kind,
                                    path,
                                }
                            },
                            cx,
                        );
                    }
                    Some(Err(message)) => shell.player_pick_error = Some(message),
                    None => {}
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Launches the selected player on the bundled test clip and reports the
    /// result. MPC-HC's disabled-web-interface error is explained in full.
    pub(super) fn test_player(&mut self, cx: &mut Context<'_, Self>) {
        if matches!(self.playback_test_status, PlaybackTestStatus::Testing) {
            return;
        }
        self.playback_test_status = PlaybackTestStatus::Testing;
        let task = (self.playback_services.test)(self.playback.clone(), cx);
        let shell = cx.entity().downgrade();
        self.playback_test_task = Some(cx.spawn(async move |_, cx| {
            let result = task.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.playback_test_status = match result {
                    Ok(()) => PlaybackTestStatus::Ok(
                        "The player's control interface responded.".to_owned(),
                    ),
                    Err(error) => PlaybackTestStatus::Failed(error.to_string()),
                };
                shell.playback_test_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Applies one Playback change, validates it and saves it without waiting.
    fn update_playback(
        &mut self,
        change: impl FnOnce(&mut PlaybackSettings),
        cx: &mut Context<'_, Self>,
    ) {
        change(&mut self.playback);
        self.commit_playback(cx);
        cx.notify();
    }

    /// Persists the current Playback tab. A newer save cancels the older one,
    /// so the database always ends up with the latest choice.
    fn commit_playback(&mut self, cx: &mut Context<'_, Self>) {
        if let Err(error) = self.playback.validate() {
            self.playback_error = Some(error.to_string());
            return;
        }
        self.playback_error = None;
        let task = (self.playback_services.save)(&self.playback, cx);
        self.save_playback_task = Some(cx.spawn(async move |_, _| {
            task.await;
        }));
    }

    pub(super) fn request_poster_cache(
        &mut self,
        action: PosterCacheAction,
        cx: &mut Context<'_, Self>,
    ) {
        // A newer request replaces the one in flight, except that nothing
        // interrupts a clear halfway.
        if matches!(self.storage_status, StorageStatus::Clearing) && self.storage_task.is_some() {
            return;
        }
        self.storage_status = match action {
            PosterCacheAction::Measure => StorageStatus::Measuring,
            PosterCacheAction::Clear => StorageStatus::Clearing,
        };
        let task = (self.poster_cache)(action, cx);
        self.storage_task = Some(cx.spawn(async move |shell, cx| {
            let result = task.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.storage_status = match result {
                    Ok(bytes) => StorageStatus::Size(bytes),
                    Err(_) => StorageStatus::Failed(action),
                };
                shell.storage_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the settings tabs share one renderer and are short individually"
    )]
    pub(super) fn render_settings(
        &self,
        tab: SettingsTab,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        const TABS: [SettingsTab; 5] = [
            SettingsTab::Appearance,
            SettingsTab::Account,
            SettingsTab::Playback,
            SettingsTab::Storage,
            SettingsTab::About,
        ];
        let selected = match tab {
            SettingsTab::Appearance => 0,
            SettingsTab::Account => 1,
            SettingsTab::Playback => 2,
            SettingsTab::Storage => 3,
            SettingsTab::About => 4,
        };
        let tabs = TabBar::new("settings-tabs")
            .underline()
            .selected_index(selected)
            .child(settings_tab("settings-appearance", "Appearance"))
            .child(settings_tab("settings-account", "Account"))
            .child(settings_tab("settings-playback", "Playback"))
            .child(settings_tab("settings-storage", "Storage"))
            .child(settings_tab("settings-about", "About"))
            .on_click(cx.listener(|shell, index: &usize, _, cx| {
                if let Some(&tab) = TABS.get(*index) {
                    shell.navigate(Route::Settings { tab }, cx);
                }
            }));

        let (title, items) = match tab {
            SettingsTab::Appearance => {
                let checked = self.prefs.mode == RailMode::ClickPin;
                let shell = cx.entity().downgrade();
                (
                    "Appearance",
                    vec![SettingItem::render(move |_, _, _| {
                        let shell = shell.clone();
                        Switch::new("setting-click-pin")
                            .label("Pin the sidebar on click instead of opening it on hover")
                            .checked(checked)
                            .on_click(move |checked, _, cx| {
                                let _ = shell.update(cx, |shell, cx| {
                                    let mode = if *checked {
                                        RailMode::ClickPin
                                    } else {
                                        RailMode::Hover
                                    };
                                    shell.set_rail_mode(mode, cx);
                                });
                            })
                    })],
                )
            }
            SettingsTab::Account => {
                let account = self.account.clone();
                let signout_account = self.account.clone();
                let token_input = self.token_input.clone();
                let login_url = self.login_url.clone();
                let busy = self.account_busy;
                let logged_in = self.account.read(cx).is_logged_in();
                let expired = self.account.read(cx).expired();
                let avatar = self.account.read(cx).avatar().map(SharedString::from);
                let name = self
                    .account
                    .read(cx)
                    .name()
                    .unwrap_or(if expired {
                        "AniList (token expired)"
                    } else {
                        "Not connected"
                    })
                    .to_owned();
                let error = self.account.read(cx).error().map(str::to_owned);
                let expiring = self.account.read(cx).expiring();
                let avatar_bg = cx.theme().colors.accent;
                (
                    "Account",
                    vec![SettingItem::render(move |_, _, _| {
                        let account = account.clone();
                        let token = token_input.clone();
                        let login_url = login_url.clone();
                        div()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(avatar.clone().map_or_else(
                                        || div().w(px(48.)).h(px(48.)).rounded_full().bg(avatar_bg),
                                        |url| {
                                            div().child(
                                                img(url)
                                                    .w(px(48.))
                                                    .h(px(48.))
                                                    .rounded_full()
                                                    .object_fit(ObjectFit::Cover)
                                                    .id("account-avatar")
                                                    .test_support(),
                                            )
                                        },
                                    ))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(Label::new(name.clone()))
                                            .child(
                                                Label::new(if expired {
                                                    "Token expired — AniList is read-only until you log in again"
                                                } else if expiring {
                                                    "AniList token expires soon — log in again to keep syncing"
                                                } else if logged_in {
                                                    "Connected to AniList"
                                                } else {
                                                    "Account-free: local lists only"
                                                })
                                                .text_sm(),
                                            ),
                                    ),
                            )
                            .when_some(error.clone(), |column, message| {
                                column.child(Label::new(message).text_sm())
                            })
                            .child(
                                Input::new(&token).aria_label("AniList token"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_3()
                                    .child(
                                        Button::new("open-anilist")
                                            .label("Open AniList")
                                            .on_click(move |_, _, cx| cx.open_url(&login_url)),
                                    )
                                    .child(
                                        Button::new("connect-anilist")
                                            .label(if busy { "Connecting…" } else { "Connect" })
                                            .disabled(busy)
                                            .on_click(move |_, _, cx| {
                                                let pasted = token.read(cx).value().to_string();
                                                account.update(cx, |_, cx| {
                                                    AccountStore::request(
                                                        AccountCommand::Login(pasted),
                                                        cx,
                                                    );
                                                });
                                            }),
                                    )
                                    .when(logged_in, |buttons| {
                                        let account = signout_account.clone();
                                        buttons.child(
                                            Button::new("sign-out-anilist")
                                                .label("Sign out")
                                                .disabled(busy)
                                                .on_click(move |_, _, cx| {
                                                    account.update(cx, |_, cx| {
                                                        AccountStore::request(
                                                            AccountCommand::SignOut,
                                                            cx,
                                                        );
                                                    });
                                                }),
                                        )
                                    }),
                            )
                    })],
                )
            }
            SettingsTab::Playback => {
                let shell = cx.entity();
                let playback = self.playback.clone();
                let players = self.players.clone();
                let pick_busy = self.player_pick_task.is_some();
                let pick_error = self.player_pick_error.clone();
                let validation_error = self.playback_error.clone();
                let test_status = self.playback_test_status.clone();
                let custom_path = match &playback.player {
                    PlayerChoice::Custom { path, .. } => Some(path.clone()),
                    _ => None,
                };
                let mpchc_selected = match &playback.player {
                    PlayerChoice::Installed { kind } | PlayerChoice::Custom { kind, .. } => {
                        *kind == PlaybackKind::MpcHc
                    }
                    PlayerChoice::Auto => false,
                };

                let player_shell = shell.clone();
                let test_shell = shell;
                let mut items = vec![
                    SettingItem::render(move |_, _, _| {
                        let shell = player_shell.clone();
                        let mut group = RadioGroup::new("playback-player");
                        let auto_shell = shell.clone();
                        group = group.child(
                            Radio::new("player-auto")
                                .label("Auto — the preferred installed player")
                                .checked(playback.player == PlayerChoice::Auto)
                                .on_click(move |_, _, cx| {
                                    auto_shell.update(cx, |shell, cx| {
                                        shell.update_playback(
                                            |settings| settings.player = PlayerChoice::Auto,
                                            cx,
                                        );
                                    });
                                }),
                        );
                        for (index, option) in players.iter().enumerate() {
                            let shell = shell.clone();
                            let kind = option.kind;
                            group = group.child(
                                Radio::new(format!("player-installed-{index}"))
                                    .label(option.label.to_string())
                                    .checked(playback.player == PlayerChoice::Installed { kind })
                                    .on_click(move |_, _, cx| {
                                        shell.update(cx, |shell, cx| {
                                            shell.update_playback(
                                                |settings| {
                                                    settings.player =
                                                        PlayerChoice::Installed { kind }
                                                },
                                                cx,
                                            );
                                        });
                                    }),
                            );
                        }
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(Label::new("Player"))
                            .child(group)
                            .when_some(custom_path.clone(), |column, path| {
                                column.child(
                                    Label::new(format!("Custom player: {}", path.display()))
                                        .text_sm(),
                                )
                            })
                            .child(
                                Button::new("custom-player-path")
                                    .label(if pick_busy {
                                        "Choosing…"
                                    } else {
                                        "Custom path…"
                                    })
                                    .disabled(pick_busy)
                                    .on_click(move |_: &ClickEvent, _, cx| {
                                        shell.update(cx, AppShell::pick_custom_player);
                                    }),
                            )
                            .when_some(pick_error.clone(), |column, message| {
                                column.child(Label::new(message).text_sm())
                            })
                    }),
                    SettingItem::new(
                        "Watched threshold (%)",
                        SettingField::input(
                            {
                                let shell = cx.entity();
                                move |cx: &gpui_kit::App| {
                                    shell.read(cx).playback.watched_threshold.to_string().into()
                                }
                            },
                            {
                                let shell = cx.entity();
                                move |value: SharedString, cx: &mut gpui_kit::App| {
                                    if let Ok(parsed) = value.parse::<u8>() {
                                        shell.update(cx, |shell, cx| {
                                            shell.update_playback(
                                                |settings| settings.watched_threshold = parsed,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        ),
                    )
                    .description("An episode counts as watched here or at the end of the file."),
                    SettingItem::new(
                        "Auto-update progress",
                        SettingField::switch(
                            {
                                let shell = cx.entity();
                                move |cx: &gpui_kit::App| shell.read(cx).playback.auto_sync_progress
                            },
                            {
                                let shell = cx.entity();
                                move |value: bool, cx: &mut gpui_kit::App| {
                                    shell.update(cx, |shell, cx| {
                                        shell.update_playback(
                                            |settings| settings.auto_sync_progress = value,
                                            cx,
                                        );
                                    });
                                }
                            },
                        ),
                    )
                    .description("Update the active platform when an episode is watched."),
                    SettingItem::new(
                        "Resume where I stopped",
                        SettingField::switch(
                            {
                                let shell = cx.entity();
                                move |cx: &gpui_kit::App| shell.read(cx).playback.resume
                            },
                            {
                                let shell = cx.entity();
                                move |value: bool, cx: &mut gpui_kit::App| {
                                    shell.update(cx, |shell, cx| {
                                        shell.update_playback(
                                            |settings| settings.resume = value,
                                            cx,
                                        );
                                    });
                                }
                            },
                        ),
                    ),
                    SettingItem::new(
                        "Auto-next",
                        SettingField::switch(
                            {
                                let shell = cx.entity();
                                move |cx: &gpui_kit::App| shell.read(cx).playback.auto_next
                            },
                            {
                                let shell = cx.entity();
                                move |value: bool, cx: &mut gpui_kit::App| {
                                    shell.update(cx, |shell, cx| {
                                        shell.update_playback(
                                            |settings| settings.auto_next = value,
                                            cx,
                                        );
                                    });
                                }
                            },
                        ),
                    ),
                    SettingItem::new(
                        "Countdown seconds",
                        SettingField::input(
                            {
                                let shell = cx.entity();
                                move |cx: &gpui_kit::App| {
                                    shell.read(cx).playback.auto_next_seconds.to_string().into()
                                }
                            },
                            {
                                let shell = cx.entity();
                                move |value: SharedString, cx: &mut gpui_kit::App| {
                                    if let Ok(parsed) = value.parse::<u8>() {
                                        shell.update(cx, |shell, cx| {
                                            shell.update_playback(
                                                |settings| settings.auto_next_seconds = parsed,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        ),
                    ),
                    SettingItem::new(
                        "Auto-skip OP/ED",
                        SettingField::switch(
                            {
                                let shell = cx.entity();
                                move |cx: &gpui_kit::App| shell.read(cx).playback.skip_op_ed
                            },
                            {
                                let shell = cx.entity();
                                move |value: bool, cx: &mut gpui_kit::App| {
                                    shell.update(cx, |shell, cx| {
                                        shell.update_playback(
                                            |settings| settings.skip_op_ed = value,
                                            cx,
                                        );
                                    });
                                }
                            },
                        ),
                    ),
                    SettingItem::new(
                        "Opening chapter patterns",
                        SettingField::input(
                            {
                                let shell = cx.entity();
                                move |cx: &gpui_kit::App| {
                                    shell.read(cx).playback.opening_patterns.join(", ").into()
                                }
                            },
                            {
                                let shell = cx.entity();
                                move |value: SharedString, cx: &mut gpui_kit::App| {
                                    let patterns = value
                                        .split(',')
                                        .map(str::trim)
                                        .filter(|pattern| !pattern.is_empty())
                                        .map(str::to_owned)
                                        .collect();
                                    shell.update(cx, |shell, cx| {
                                        shell.update_playback(
                                            |settings| settings.opening_patterns = patterns,
                                            cx,
                                        );
                                    });
                                }
                            },
                        ),
                    )
                    .description("Comma-separated chapter names to skip at the opening."),
                    SettingItem::new(
                        "Ending chapter patterns",
                        SettingField::input(
                            {
                                let shell = cx.entity();
                                move |cx: &gpui_kit::App| {
                                    shell.read(cx).playback.ending_patterns.join(", ").into()
                                }
                            },
                            {
                                let shell = cx.entity();
                                move |value: SharedString, cx: &mut gpui_kit::App| {
                                    let patterns = value
                                        .split(',')
                                        .map(str::trim)
                                        .filter(|pattern| !pattern.is_empty())
                                        .map(str::to_owned)
                                        .collect();
                                    shell.update(cx, |shell, cx| {
                                        shell.update_playback(
                                            |settings| settings.ending_patterns = patterns,
                                            cx,
                                        );
                                    });
                                }
                            },
                        ),
                    )
                    .description("Comma-separated chapter names to skip at the ending."),
                ];

                if mpchc_selected {
                    items.push(SettingItem::new(
                        "MPC-HC host",
                        SettingField::input(
                            {
                                let shell = cx.entity();
                                move |cx: &gpui_kit::App| {
                                    shell.read(cx).playback.mpchc_host.clone().into()
                                }
                            },
                            {
                                let shell = cx.entity();
                                move |value: SharedString, cx: &mut gpui_kit::App| {
                                    shell.update(cx, |shell, cx| {
                                        shell.update_playback(
                                            |settings| settings.mpchc_host = value.to_string(),
                                            cx,
                                        );
                                    });
                                }
                            },
                        ),
                    ));
                    items.push(SettingItem::new(
                        "MPC-HC port",
                        SettingField::input(
                            {
                                let shell = cx.entity();
                                move |cx: &gpui_kit::App| {
                                    shell.read(cx).playback.mpchc_port.to_string().into()
                                }
                            },
                            {
                                let shell = cx.entity();
                                move |value: SharedString, cx: &mut gpui_kit::App| {
                                    if let Ok(parsed) = value.parse::<u16>() {
                                        shell.update(cx, |shell, cx| {
                                            shell.update_playback(
                                                |settings| settings.mpchc_port = parsed,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        ),
                    ));
                    items.push(SettingItem::render(move |_, _, _| {
                        Label::new(
                            "MPC-HC needs its web interface enabled in \
                             Options → Playback → Web Interface. The Test button \
                             explains a refused connection.",
                        )
                        .text_sm()
                    }));
                }

                items.push(SettingItem::render(move |_, _, _| {
                    let shell = test_shell.clone();
                    let testing = matches!(&test_status, PlaybackTestStatus::Testing);
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            Button::new("test-player")
                                .label(if testing { "Testing…" } else { "Test player" })
                                .disabled(testing)
                                .on_click(move |_: &ClickEvent, _, cx| {
                                    shell.update(cx, AppShell::test_player);
                                }),
                        )
                        .child(Label::new(test_status.label()).text_sm())
                        .when_some(validation_error.clone(), |row, message| {
                            row.child(Label::new(message).text_sm())
                        })
                }));

                ("Playback", items)
            }
            SettingsTab::About => (
                "About",
                vec![SettingItem::render(move |_, _, _| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child("Attribution")
                        .child(Label::new(ODBL_CREDIT).text_sm())
                })],
            ),
            SettingsTab::Storage => {
                let status = self.storage_status;
                let shell = cx.entity().downgrade();
                (
                    "Storage",
                    vec![SettingItem::render(move |_, _, _| {
                        let shell = shell.clone();
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .flex_wrap()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child("Poster cache")
                                    .child(Label::new(status.label()).text_sm()),
                            )
                            .child(
                                Button::new("clear-poster-cache")
                                    .label("Clear poster cache")
                                    .disabled(status.busy())
                                    .on_click(move |_: &ClickEvent, _: &mut Window, cx| {
                                        let _ = shell.update(cx, |shell, cx| {
                                            shell
                                                .request_poster_cache(PosterCacheAction::Clear, cx);
                                        });
                                    }),
                            )
                    })],
                )
            }
        };

        // The route controls which page is active. Hide the widget's own
        // navigation and page title because the tab bar already supplies both.
        let hidden = StyleRefinement {
            display: Some(Display::None),
            ..StyleRefinement::default()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().px_8().pt_8().text_3xl().child("Settings"))
            .child(div().px_8().pt_4().child(tabs))
            .child(
                div().flex_1().min_h_0().px_4().child(
                    Settings::new("settings-page")
                        .sidebar_width(px(0.))
                        .sidebar_size_range(px(0.)..px(0.))
                        .sidebar_style(&hidden)
                        .page(
                            SettingPage::new(title).header_style(&hidden).group(
                                items
                                    .into_iter()
                                    .fold(SettingGroup::new(), SettingGroup::item),
                            ),
                        ),
                ),
            )
    }
}

/// Formats a byte count in decimal units, as macOS Finder shows it.
fn size_label(bytes: u64) -> String {
    const UNITS: [(&str, u64); 3] = [("GB", 1_000_000_000), ("MB", 1_000_000), ("KB", 1_000)];
    for (name, unit) in UNITS {
        if bytes >= unit {
            let tenths = bytes.saturating_mul(10) / unit;
            return format!("{}.{} {name}", tenths / 10, tenths % 10);
        }
    }
    format!("{bytes} bytes")
}

fn settings_tab(id: &'static str, label: &'static str) -> Tab {
    Tab::new()
        .aria_label(label)
        .child(div().id(id).test_support().child(label))
}

#[cfg(test)]
mod tests {
    use super::{PlaybackTestError, size_label};

    #[test]
    fn a_disabled_mpc_hc_interface_is_explained() {
        let message = PlaybackTestError::WebInterfaceDisabled.to_string();
        assert!(message.contains("web interface"), "{message}");
        assert!(message.contains("Options"), "{message}");
    }

    #[test]
    fn sizes_read_in_decimal_units_with_one_decimal() {
        assert_eq!(size_label(0), "0 bytes");
        assert_eq!(size_label(999), "999 bytes");
        assert_eq!(size_label(1_000), "1.0 KB");
        assert_eq!(size_label(48_213_912), "48.2 MB");
        assert_eq!(size_label(3_500_000_000), "3.5 GB");
    }
}
