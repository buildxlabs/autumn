//! Settings pages and their poster cache controls.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use domain::{
    ListStatus, TransferChoice, TransferDirection, TransferPreview, TransferProgress, TransferState,
};
use gpui_kit::{
    App, ClickEvent, Context, Display, Entity, InteractiveElement, IntoElement, ObjectFit,
    ParentElement, PathPromptOptions, SharedString, StyleRefinement, Styled, StyledImage, Task,
    TestSupportExt, Window,
    component::{
        ActiveTheme, Disableable,
        button::Button,
        input::Input,
        label::Label,
        radio::{Radio, RadioGroup},
        setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
        slider::Slider,
        switch::Switch,
        tab::{Tab, TabBar},
    },
    div, img,
    prelude::FluentBuilder,
    px,
};
use metadata::ODBL_CREDIT;
use settings::{
    GeneralSettings, LibrarySettings, PlaybackKind, PlaybackSettings, PlayerChoice,
    TorrentSettings, validate_torrent_folder,
};
use ui_state::{
    LibraryStore, Route, ScanState, SettingsTab,
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

type SaveGeneral = Rc<dyn Fn(&GeneralSettings, &mut App) -> Task<()>>;
type SavePlayback = Rc<dyn Fn(&PlaybackSettings, &mut App) -> Task<()>>;
type SaveLibrary = Rc<dyn Fn(&LibrarySettings, &mut App) -> Task<()>>;
type SaveTorrent = Rc<dyn Fn(&TorrentSettings, &mut App) -> Task<()>>;
type TorrentCacheRequest = Rc<dyn Fn(TorrentCacheAction, &mut App) -> Task<gpui_kit::Result<u64>>>;
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

/// Writes the General tab, which holds the first-run flag (#138).
#[derive(Clone)]
pub struct GeneralServices {
    save: SaveGeneral,
}

impl std::fmt::Debug for GeneralServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneralServices").finish_non_exhaustive()
    }
}

impl GeneralServices {
    /// Creates the callback the General tab and the wizard use to save.
    pub fn new(save: impl Fn(&GeneralSettings, &mut App) -> Task<()> + 'static) -> Self {
        Self {
            save: Rc::new(save),
        }
    }

    /// Inert callbacks for embedding the shell without a settings backend.
    #[must_use]
    pub fn inert() -> Self {
        Self::new(|_, _| Task::ready(()))
    }

    /// Saves one General tab value; the wizard and the test seam use it.
    pub fn save(&self, settings: &GeneralSettings, cx: &mut App) -> Task<()> {
        (self.save)(settings, cx)
    }
}

/// The composition-root callback that persists the Library tab.
#[derive(Clone)]
pub struct LibraryServices {
    save: SaveLibrary,
}

impl std::fmt::Debug for LibraryServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryServices").finish_non_exhaustive()
    }
}

impl LibraryServices {
    /// Creates the save callback the Library tab needs.
    pub fn new(save: impl Fn(&LibrarySettings, &mut App) -> Task<()> + 'static) -> Self {
        Self {
            save: Rc::new(save),
        }
    }

    /// Inert callbacks for embedding the shell without a settings backend.
    #[must_use]
    pub fn inert() -> Self {
        Self::new(|_, _| Task::ready(()))
    }
}

/// The composition-root calls behind the torrent controls on the Storage tab.
#[derive(Clone)]
pub struct TorrentServices {
    save: SaveTorrent,
    cache: TorrentCacheRequest,
    default_folder: PathBuf,
}

impl std::fmt::Debug for TorrentServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TorrentServices")
            .field("default_folder", &self.default_folder)
            .finish_non_exhaustive()
    }
}

impl TorrentServices {
    /// Creates the save and cache callbacks, and the default download folder.
    pub fn new(
        save: impl Fn(&TorrentSettings, &mut App) -> Task<()> + 'static,
        cache: impl Fn(TorrentCacheAction, &mut App) -> Task<gpui_kit::Result<u64>> + 'static,
        default_folder: PathBuf,
    ) -> Self {
        Self {
            save: Rc::new(save),
            cache: Rc::new(cache),
            default_folder,
        }
    }

    /// Inert callbacks for embedding the shell without a torrent backend.
    #[must_use]
    pub fn inert() -> Self {
        Self::new(
            |_, _| Task::ready(()),
            |_, _| Task::ready(Ok(0)),
            PathBuf::from("<cache>/torrents"),
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

/// A request from Settings → Storage for the on-disk torrent cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TorrentCacheAction {
    /// Read the current size.
    Measure,
    /// Clear idle torrents and then read the remaining size.
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

#[derive(Debug, Clone, Copy)]
pub(super) enum TorrentCacheStatus {
    Measuring,
    Clearing,
    Size(u64),
    Failed(TorrentCacheAction),
}

impl TorrentCacheStatus {
    fn label(self) -> SharedString {
        match self {
            Self::Measuring => "Reading torrent cache size…".into(),
            Self::Clearing => "Clearing torrent cache…".into(),
            Self::Size(bytes) => size_label(bytes).into(),
            Self::Failed(TorrentCacheAction::Measure) => {
                "Could not read the torrent cache size.".into()
            }
            Self::Failed(TorrentCacheAction::Clear) => "Could not clear the torrent cache.".into(),
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
    pub(super) fn update_playback(
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

    /// Adds the chosen folders to the library, skipping ones already listed.
    pub(super) fn add_library_folders(&mut self, cx: &mut Context<'_, Self>) {
        self.pick_library_folders(false, cx);
    }

    /// Picks folders for the Library tab, or for the wizard's first step,
    /// where a successful pick also starts the first scan.
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "the Library tab and the wizard share this picker; only the wizard scans after saving"
    )]
    pub(super) fn pick_library_folders(&mut self, scan_after: bool, cx: &mut Context<'_, Self>) {
        if self.library_pick_task.is_some() {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Choose library folders".into()),
        });
        let shell = cx.entity().downgrade();
        self.library_pick_task = Some(cx.spawn(async move |_, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths,
                _ => Vec::new(),
            };
            let _ = shell.update(cx, |shell, cx| {
                shell.library_pick_task = None;
                for path in picked {
                    if !shell.library.folders.contains(&path) {
                        shell.library.folders.push(path);
                    }
                }
                shell.commit_library(cx);
                if scan_after {
                    shell.start_scan(cx);
                }
            });
        }));
        cx.notify();
    }

    /// Removes one folder; files found only there are shelved on the next scan.
    fn remove_library_folder(&mut self, path: &Path, cx: &mut Context<'_, Self>) {
        self.library.folders.retain(|folder| folder != path);
        self.commit_library(cx);
        cx.notify();
    }

    /// Applies one Library change and saves it without waiting.
    fn update_library(
        &mut self,
        change: impl FnOnce(&mut LibrarySettings),
        cx: &mut Context<'_, Self>,
    ) {
        change(&mut self.library);
        self.commit_library(cx);
        cx.notify();
    }

    fn commit_library(&mut self, cx: &mut Context<'_, Self>) {
        if let Err(error) = self.library.validate() {
            self.library_error = Some(error.to_string());
            return;
        }
        // A library folder added later must not swallow the torrent cache.
        if let Err(error) = self.validate_torrent_folder() {
            self.library_error = Some(error.to_string());
            return;
        }
        self.library_error = None;
        let task = (self.library_services.save)(&self.library, cx);
        self.save_library_task = Some(cx.spawn(async move |_, _| {
            task.await;
        }));
    }

    /// Requests one scan from the Library tab. Progress arrives as `AppEvent`s.
    pub(super) fn start_scan(&mut self, cx: &mut Context<'_, Self>) {
        self.library_store.update(cx, LibraryStore::start_scan);
    }

    /// Keeps the slider's value in step with the saved setting.
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the slider is clamped to 0..=100 before conversion"
    )]
    pub(super) fn library_threshold_changed(&mut self, value: f32, cx: &mut Context<'_, Self>) {
        let percent = value.round().clamp(0.0, 100.0) as u8;
        if self.library.matching_threshold != percent {
            self.update_library(|settings| settings.matching_threshold = percent, cx);
        }
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

    /// The effective folder: the chosen one or the cache default.
    fn torrent_folder(&self) -> PathBuf {
        self.torrent
            .download_folder
            .clone()
            .unwrap_or_else(|| self.torrent_services.default_folder.clone())
    }

    fn validate_torrent_folder(&self) -> Result<(), settings::SettingsError> {
        validate_torrent_folder(&self.torrent_folder(), &self.library.folders)
    }

    /// Picks a torrent download folder and saves it unless it overlaps the
    /// library folders.
    pub(super) fn pick_torrent_folder(&mut self, cx: &mut Context<'_, Self>) {
        if self.torrent_pick_task.is_some() {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a torrent download folder".into()),
        });
        let shell = cx.entity().downgrade();
        self.torrent_pick_task = Some(cx.spawn(async move |_, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let _ = shell.update(cx, |shell, cx| {
                shell.torrent_pick_task = None;
                if let Some(path) = picked {
                    shell.update_torrent(|settings| settings.download_folder = Some(path), cx);
                }
            });
        }));
        cx.notify();
    }

    /// Applies one torrent change and saves it without waiting.
    pub(super) fn update_torrent(
        &mut self,
        change: impl FnOnce(&mut TorrentSettings),
        cx: &mut Context<'_, Self>,
    ) {
        change(&mut self.torrent);
        self.commit_torrent(cx);
        cx.notify();
    }

    fn commit_torrent(&mut self, cx: &mut Context<'_, Self>) {
        if let Err(error) = self.validate_torrent_folder() {
            self.torrent_error = Some(error.to_string());
            return;
        }
        self.torrent_error = None;
        let task = (self.torrent_services.save)(&self.torrent, cx);
        self.save_torrent_task = Some(cx.spawn(async move |_, _| {
            task.await;
        }));
    }

    pub(super) fn request_torrent_cache(
        &mut self,
        action: TorrentCacheAction,
        cx: &mut Context<'_, Self>,
    ) {
        if matches!(self.torrent_cache_status, TorrentCacheStatus::Clearing)
            && self.torrent_cache_task.is_some()
        {
            return;
        }
        self.torrent_cache_status = match action {
            TorrentCacheAction::Measure => TorrentCacheStatus::Measuring,
            TorrentCacheAction::Clear => TorrentCacheStatus::Clearing,
        };
        let task = (self.torrent_services.cache)(action, cx);
        self.torrent_cache_task = Some(cx.spawn(async move |shell, cx| {
            let result = task.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.torrent_cache_status = match result {
                    Ok(bytes) => TorrentCacheStatus::Size(bytes),
                    Err(_) => TorrentCacheStatus::Failed(action),
                };
                shell.torrent_cache_task = None;
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
        const TABS: [SettingsTab; 7] = [
            SettingsTab::General,
            SettingsTab::Appearance,
            SettingsTab::Account,
            SettingsTab::Playback,
            SettingsTab::Library,
            SettingsTab::Storage,
            SettingsTab::About,
        ];
        let selected = match tab {
            SettingsTab::General => 0,
            SettingsTab::Appearance => 1,
            SettingsTab::Account => 2,
            SettingsTab::Playback => 3,
            SettingsTab::Library => 4,
            SettingsTab::Storage => 5,
            SettingsTab::About => 6,
        };
        let tabs = TabBar::new("settings-tabs")
            .underline()
            .selected_index(selected)
            .child(settings_tab("settings-general", "General"))
            .child(settings_tab("settings-appearance", "Appearance"))
            .child(settings_tab("settings-account", "Account"))
            .child(settings_tab("settings-playback", "Playback"))
            .child(settings_tab("settings-library", "Library"))
            .child(settings_tab("settings-storage", "Storage"))
            .child(settings_tab("settings-about", "About"))
            .on_click(cx.listener(|shell, index: &usize, _, cx| {
                if let Some(&tab) = TABS.get(*index) {
                    shell.navigate(Route::Settings { tab }, cx);
                }
            }));

        let (title, items) = match tab {
            SettingsTab::General => {
                let shell = cx.entity().downgrade();
                (
                    "General",
                    vec![SettingItem::render(move |_, _, _| {
                        let shell = shell.clone();
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(Label::new("Review the library, account and player setup."))
                            .child(
                                Button::new("settings-run-setup")
                                    .label("Run setup again")
                                    .on_click(move |_, _, cx| {
                                        let _ = shell.update(cx, |shell, cx| {
                                            shell.reopen_onboarding(cx);
                                        });
                                    }),
                            )
                            .into_any_element()
                    })],
                )
            }
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
                let review = self.account.read(cx).transfer_preview().cloned();
                let progress = self.account.read(cx).transfer_progress().cloned();
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
                            .when(review.is_some() || progress.is_some(), |column| {
                                column.child(transfer_review(
                                    account.clone(),
                                    review.as_ref(),
                                    progress.as_ref(),
                                ))
                            })
                            .when(
                                review.is_none() && logged_in && !expired,
                                |column| {
                                    let both = account.clone();
                                    let to_local = account.clone();
                                    column.child(
                                        div()
                                            .flex()
                                            .gap_3()
                                            .child(
                                                Button::new("transfer-to-anilist")
                                                    .label("Copy Local lists to AniList")
                                                    .on_click(move |_, _, cx| {
                                                        both.update(cx, |_, cx| {
                                                            AccountStore::request(
                                                                AccountCommand::TransferPreview {
                                                                    direction: domain::TransferDirection::ToAniList,
                                                                },
                                                                cx,
                                                            );
                                                        });
                                                    }),
                                            )
                                            .child(
                                                Button::new("transfer-to-local")
                                                    .label("Copy AniList lists to Local")
                                                    .on_click(move |_, _, cx| {
                                                        to_local.update(cx, |_, cx| {
                                                            AccountStore::request(
                                                                AccountCommand::TransferPreview {
                                                                    direction: domain::TransferDirection::ToLocal,
                                                                },
                                                                cx,
                                                            );
                                                        });
                                                    }),
                                            ),
                                    )
                                },
                            )
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
            SettingsTab::Library => {
                let shell = cx.entity().downgrade();
                let folders = self.library.folders.clone();
                let pick_busy = self.library_pick_task.is_some();
                let validation_error = self.library_error.clone();
                let slider = self.threshold_slider.clone();
                let scan_state = self.library_store.read(cx).scan();
                let last = self.library_store.read(cx).summary().cloned();
                let scanning = matches!(scan_state, ScanState::Running { .. });
                let progress = match scan_state {
                    ScanState::Running {
                        files_walked,
                        files_matched,
                    } => format!("Scanning… {files_matched} matched of {files_walked} files"),
                    _ => "Ready to scan".to_owned(),
                };
                let folder_shell = shell.clone();
                let scan_shell = shell;
                let summaries_shell = cx.entity().downgrade();
                let items = vec![
                    SettingItem::render(move |_, _, _| {
                        let shell = folder_shell.clone();
                        let mut column = div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(Label::new("Library folders"));
                        for folder in &folders {
                            let path = folder.clone();
                            let remove_shell = shell.clone();
                            column = column.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        Label::new(folder.to_string_lossy().to_string()).text_sm(),
                                    )
                                    .child(
                                        Button::new(format!("remove-folder-{}", folder.display()))
                                            .label("Remove")
                                            .on_click(move |_, _, cx| {
                                                let _ = remove_shell.update(cx, |shell, cx| {
                                                    shell.remove_library_folder(&path, cx);
                                                });
                                            }),
                                    ),
                            );
                        }
                        column
                            .child(
                                Button::new("add-library-folder")
                                    .label(if pick_busy {
                                        "Choosing…"
                                    } else {
                                        "Add folder…"
                                    })
                                    .disabled(pick_busy)
                                    .on_click(move |_, _, cx| {
                                        let _ = shell.update(cx, AppShell::add_library_folders);
                                    }),
                            )
                            .when_some(validation_error.clone(), |column, message| {
                                column.child(Label::new(message).text_sm())
                            })
                    }),
                    SettingItem::render(move |_, _, cx| {
                        let value = slider.read(cx).value().end();
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(Label::new(format!("Matching threshold: {value:.0}%")))
                            .child(Slider::new(&slider).accessibility_label("Matching threshold"))
                    }),
                    SettingItem::new(
                        "Auto-scan folders",
                        SettingField::switch(
                            {
                                let shell = cx.entity();
                                move |cx: &gpui_kit::App| shell.read(cx).library.auto_scan
                            },
                            {
                                let shell = cx.entity();
                                move |value: bool, cx: &mut gpui_kit::App| {
                                    shell.update(cx, |shell, cx| {
                                        shell.update_library(
                                            |settings| settings.auto_scan = value,
                                            cx,
                                        );
                                    });
                                }
                            },
                        ),
                    )
                    .description("Scan again when a folder's contents change."),
                    SettingItem::new(
                        "Mark filler episodes",
                        SettingField::switch(
                            {
                                let shell = cx.entity();
                                move |cx: &gpui_kit::App| {
                                    shell.read(cx).library.fetch_filler_episodes
                                }
                            },
                            {
                                let shell = cx.entity();
                                move |value: bool, cx: &mut gpui_kit::App| {
                                    shell.update(cx, |shell, cx| {
                                        shell.update_library(
                                            |settings| settings.fetch_filler_episodes = value,
                                            cx,
                                        );
                                    });
                                }
                            },
                        ),
                    )
                    .description("Look up filler lists from animefillerlist.com (OD-3)."),
                    SettingItem::render(move |_, _, _| {
                        let shell = scan_shell.clone();
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                Button::new("scan-now")
                                    .label(if scanning { "Scanning…" } else { "Scan now" })
                                    .disabled(scanning)
                                    .on_click(move |_, _, cx| {
                                        let _ = shell.update(cx, AppShell::start_scan);
                                    }),
                            )
                            .child(Label::new(progress.clone()).text_sm())
                            .when_some(last.clone(), |column, summary| {
                                column.child(
                                    Label::new(format!(
                                        "Last scan: {} files matched to {} media, {} unmatched",
                                        summary.files_matched,
                                        summary.by_media.len(),
                                        summary.files_unmatched
                                    ))
                                    .text_sm(),
                                )
                            })
                    }),
                    SettingItem::render(move |_, _, _| {
                        let shell = summaries_shell.clone();
                        Button::new("open-scan-summaries")
                            .label("View scan summaries")
                            .on_click(move |_, _, cx| {
                                let _ = shell.update(cx, |shell, cx| {
                                    shell.navigate(Route::ScanSummaries, cx);
                                });
                            })
                    }),
                ];
                ("Library", items)
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
                let torrent_status = self.torrent_cache_status;
                let folder = self.torrent_folder();
                let disable_upload = self.torrent.disable_upload;
                let pick_busy = self.torrent_pick_task.is_some();
                let validation_error = self.torrent_error.clone();
                let shell = cx.entity().downgrade();
                let folder_shell = shell.clone();
                let cache_shell = shell.clone();
                let upload_shell = cx.entity().downgrade();
                (
                    "Storage",
                    vec![
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
                                                shell.request_poster_cache(
                                                    PosterCacheAction::Clear,
                                                    cx,
                                                );
                                            });
                                        }),
                                )
                        }),
                        SettingItem::render(move |_, _, _| {
                            let pick = folder_shell.clone();
                            let reset = folder_shell.clone();
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child("Torrent download folder")
                                .child(Label::new(folder.to_string_lossy().to_string()).text_sm())
                                .child(
                                    div()
                                        .flex()
                                        .gap_3()
                                        .child(
                                            Button::new("pick-torrent-folder")
                                                .label(if pick_busy {
                                                    "Choosing…"
                                                } else {
                                                    "Choose folder…"
                                                })
                                                .disabled(pick_busy)
                                                .on_click(move |_, _, cx| {
                                                    let _ = pick
                                                        .update(cx, AppShell::pick_torrent_folder);
                                                }),
                                        )
                                        .child(
                                            Button::new("reset-torrent-folder")
                                                .label("Use default")
                                                .on_click(move |_, _, cx| {
                                                    let _ = reset.update(cx, |shell, cx| {
                                                        shell.update_torrent(
                                                            |settings| {
                                                                settings.download_folder = None;
                                                            },
                                                            cx,
                                                        );
                                                    });
                                                }),
                                        ),
                                )
                                .when_some(validation_error.clone(), |column, message| {
                                    column.child(Label::new(message).text_sm())
                                })
                        }),
                        SettingItem::render(move |_, _, _| {
                            let shell = cache_shell.clone();
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
                                        .child("Torrent cache")
                                        .child(Label::new(torrent_status.label()).text_sm()),
                                )
                                .child(
                                    Button::new("clear-torrent-cache")
                                        .label("Clear torrent cache")
                                        .disabled(torrent_status.busy())
                                        .on_click(move |_: &ClickEvent, _: &mut Window, cx| {
                                            let _ = shell.update(cx, |shell, cx| {
                                                shell.request_torrent_cache(
                                                    TorrentCacheAction::Clear,
                                                    cx,
                                                );
                                            });
                                        }),
                                )
                        }),
                        SettingItem::render(move |_, _, _| {
                            let shell = upload_shell.clone();
                            Switch::new("setting-disable-upload")
                                .label("Disable upload (advanced)")
                                .checked(disable_upload)
                                .on_click(move |checked, _, cx| {
                                    let _ = shell.update(cx, |shell, cx| {
                                        shell.update_torrent(
                                            |settings| settings.disable_upload = *checked,
                                            cx,
                                        );
                                    });
                                })
                        }),
                    ],
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

/// The reviewed preview and progress of a list transfer in Settings → Account.
fn transfer_review(
    account: Entity<AccountStore>,
    preview: Option<&TransferPreview>,
    progress: Option<&TransferProgress>,
) -> impl IntoElement {
    let mut column = div()
        .id("list-transfer-review")
        .test_support()
        .flex()
        .flex_col()
        .gap_2()
        .child(Label::new("List transfer"));
    if let Some(preview) = preview {
        column = column
            .child(
                Label::new(format!(
                    "Copy {} lists to {}",
                    preview.direction.source_label(),
                    preview.direction.destination_label()
                ))
                .text_sm(),
            )
            .child(transfer_items(&account, preview));
        if !preview.custom_lists.is_empty() {
            let names: Vec<_> = preview
                .custom_lists
                .iter()
                .map(|(_, name)| name.as_str())
                .collect();
            column = column
                .child(Label::new(format!("Create custom lists: {}", names.join(", "))).text_sm());
        }
        column = column.child(transfer_actions(account, preview));
    }
    if let Some(progress) = progress {
        column = column.child(transfer_progress(progress));
    }
    column
}

/// One row per list entry, with a button that overrides that entry's choice.
fn transfer_items(account: &Entity<AccountStore>, preview: &TransferPreview) -> impl IntoElement {
    let rows = preview.items.iter().map(|item| {
        let media = item.media;
        let decision = choice_label(item.destination.as_ref(), item.choice, preview.direction);
        let next = next_choice(item.destination.as_ref(), item.choice);
        let destination = item
            .destination
            .as_ref()
            .map_or_else(|| "new".to_owned(), |entry| entry.progress.to_string());
        let row = account.clone();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(Label::new(format!(
                "Media {} · {} · {} → {}",
                item.media.get(),
                status_label(item.source.status),
                item.source.progress,
                destination
            )))
            .child(
                Button::new(format!("transfer-choice-{}", media.get()))
                    .label(decision)
                    .on_click(move |_, _, cx| {
                        row.update(cx, |store, cx| store.choose(media, next, cx));
                    }),
            )
    });
    div().flex().flex_col().gap_2().children(rows)
}

fn choice_label(
    destination: Option<&domain::ListEntry>,
    choice: TransferChoice,
    direction: TransferDirection,
) -> String {
    match (destination.is_some(), choice) {
        (true, TransferChoice::Keep) => format!("Keep {}", direction.destination_label()),
        (true, TransferChoice::Copy) => format!("Use {}", direction.source_label()),
        (true, TransferChoice::Skip) => "Skip".to_owned(),
        (false, TransferChoice::Skip) => "Skipped".to_owned(),
        (false, _) => "Add".to_owned(),
        _ => "Keep".to_owned(),
    }
}

fn next_choice(destination: Option<&domain::ListEntry>, choice: TransferChoice) -> TransferChoice {
    if destination.is_none() {
        return TransferChoice::Copy;
    }
    match choice {
        TransferChoice::Copy => TransferChoice::Skip,
        TransferChoice::Skip => TransferChoice::Keep,
        _ => TransferChoice::Copy,
    }
}

/// Confirm or dismiss the preview. Both finish a sign-out that was waiting on it.
fn transfer_actions(account: Entity<AccountStore>, preview: &TransferPreview) -> impl IntoElement {
    let confirm = account.clone();
    let choices: Vec<_> = preview
        .items
        .iter()
        .map(|item| (item.media, item.choice))
        .collect();
    let direction = preview.direction;
    div()
        .flex()
        .gap_3()
        .child(
            Button::new("transfer-confirm")
                .label("Transfer entries")
                .on_click(move |_, _, cx| {
                    confirm.update(cx, |_, cx| {
                        AccountStore::request(
                            AccountCommand::TransferConfirm {
                                direction,
                                choices: choices.clone(),
                            },
                            cx,
                        );
                    });
                }),
        )
        .child(
            Button::new("transfer-cancel")
                .label("Cancel")
                .on_click(move |_, _, cx| {
                    account.update(cx, |_, cx| {
                        AccountStore::request(AccountCommand::TransferCancel { direction }, cx);
                    });
                }),
        )
}

fn transfer_progress(progress: &TransferProgress) -> impl IntoElement {
    let message = match &progress.state {
        TransferState::Running => format!(
            "Transferring {} of {} entries…",
            progress.done, progress.total
        ),
        TransferState::Done => format!("Transferred {} entries", progress.done),
        TransferState::Failed(message) => {
            format!("Transfer stopped after {}: {message}", progress.done)
        }
        _ => "Transfer finished".to_owned(),
    };
    Label::new(message).text_sm()
}

fn status_label(status: ListStatus) -> &'static str {
    match status {
        ListStatus::Current => "Watching",
        ListStatus::Repeating => "Repeating",
        ListStatus::Planning => "Planned",
        ListStatus::Completed => "Completed",
        ListStatus::Paused => "Paused",
        ListStatus::Dropped => "Dropped",
        _ => "Listed",
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
