//! Settings pages and their poster cache controls.

use gpui_kit::{
    App, ClickEvent, Context, Display, InteractiveElement, IntoElement, ObjectFit, ParentElement,
    SharedString, StyleRefinement, Styled, StyledImage, Task, TestSupportExt, Window,
    component::{
        ActiveTheme, Disableable,
        button::Button,
        input::Input,
        label::Label,
        setting::{SettingGroup, SettingItem, SettingPage, Settings},
        switch::Switch,
        tab::{Tab, TabBar},
    },
    div, img,
    prelude::FluentBuilder,
    px,
};
use metadata::ODBL_CREDIT;
use ui_state::{
    Route, SettingsTab,
    account::{AccountCommand, AccountStore},
};

use super::{AppShell, RailMode};

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
        reason = "the three settings tabs share one renderer and are short individually"
    )]
    pub(super) fn render_settings(
        &self,
        tab: SettingsTab,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        const TABS: [SettingsTab; 4] = [
            SettingsTab::Appearance,
            SettingsTab::Account,
            SettingsTab::Storage,
            SettingsTab::About,
        ];
        let selected = match tab {
            SettingsTab::Appearance => 0,
            SettingsTab::Account => 1,
            SettingsTab::Storage => 2,
            SettingsTab::About => 3,
        };
        let tabs = TabBar::new("settings-tabs")
            .underline()
            .selected_index(selected)
            .child(settings_tab("settings-appearance", "Appearance"))
            .child(settings_tab("settings-account", "Account"))
            .child(settings_tab("settings-storage", "Storage"))
            .child(settings_tab("settings-about", "About"))
            .on_click(cx.listener(|shell, index: &usize, _, cx| {
                if let Some(&tab) = TABS.get(*index) {
                    shell.navigate(Route::Settings { tab }, cx);
                }
            }));

        let (title, item) = match tab {
            SettingsTab::Appearance => {
                let checked = self.prefs.mode == RailMode::ClickPin;
                let shell = cx.entity().downgrade();
                (
                    "Appearance",
                    SettingItem::render(move |_, _, _| {
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
                    }),
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
                    SettingItem::render(move |_, _, _| {
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
                    }),
                )
            }
            SettingsTab::About => (
                "About",
                SettingItem::render(move |_, _, _| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child("Attribution")
                        .child(Label::new(ODBL_CREDIT).text_sm())
                }),
            ),
            SettingsTab::Storage => {
                let status = self.storage_status;
                let shell = cx.entity().downgrade();
                (
                    "Storage",
                    SettingItem::render(move |_, _, _| {
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
                    }),
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
                            SettingPage::new(title)
                                .header_style(&hidden)
                                .group(SettingGroup::new().item(item)),
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
    use super::size_label;

    #[test]
    fn sizes_read_in_decimal_units_with_one_decimal() {
        assert_eq!(size_label(0), "0 bytes");
        assert_eq!(size_label(999), "999 bytes");
        assert_eq!(size_label(1_000), "1.0 KB");
        assert_eq!(size_label(48_213_912), "48.2 MB");
        assert_eq!(size_label(3_500_000_000), "3.5 GB");
    }
}
