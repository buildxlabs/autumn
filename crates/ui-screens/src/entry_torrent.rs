//! The Entry page's Torrents tab (#220, #182 Q1–Q3, Q11).
//!
//! An episode list; each row has **Stream** (auto-select → verify → stream)
//! and **Choose…** (the release list lands in #221; a no-op for now). While
//! running, the row shows `Searching › Verifying files n/3 › Starting
//! stream` with Cancel; on failure it shows the reason. Unaired rows are
//! dimmed with their air time; `in library` and `✓ watched` pills. The
//! per-media **Profile ▾** (#182 Q11). Profile editing is Settings →
//! Torrents (#228).

use gpui_kit::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled,
    component::{
        ActiveTheme, Disableable as _,
        button::{Button, ButtonVariants as _},
        menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    },
    div, hsla, px,
};
use ui_state::{
    Availability, EntryTab, EntryTorrentStore, TorrentEpisode, TorrentRowState, TorrentStage,
    WATCHED,
};

use crate::{entry::EntryScreen, entry_library::PlayLabels, shell::AppShell};

/// The `in library` pill label (#182 Q3).
const IN_LIBRARY: &str = "in library";
/// **Choose…**; opens the release list once #221 lands.
const CHOOSE: &str = "Choose…";

/// The trail stages, in order.
const TRAIL: [&str; 3] = ["Searching", "Verifying files n/3", "Starting stream"];

/// Renders the Torrents tab.
pub(crate) fn render(
    screen: &EntryScreen<'_>,
    _labels: &PlayLabels,
    _id: u32,
    cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    let store = screen.torrents.read(cx);
    let episodes = store.episodes().to_vec();
    let muted = cx.theme().colors.muted_foreground;
    div()
        .id("entry-torrents")
        .flex()
        .flex_col()
        .gap_3()
        .child(profile_row(screen, cx))
        .children(
            episodes
                .iter()
                .map(|episode| row(screen, episode, muted, cx))
                .collect::<Vec<AnyElement>>(),
        )
        .children(
            episodes
                .is_empty()
                .then(|| div().text_sm().text_color(muted).child("No episodes.")),
        )
}

/// The per-media **Profile ▾** (#182 Q11): global default, each saved
/// profile, writes `media_profiles`. "Edit profiles…" is #228.
fn profile_row(screen: &EntryScreen<'_>, cx: &mut Context<'_, AppShell>) -> impl IntoElement {
    let store = screen.torrents.read(cx);
    let profiles = store.profiles().to_vec();
    let default_name = store.default_profile_name().to_owned();
    let media_override = store.media_profile().map(str::to_owned);
    let effective_name = store.effective_profile().name;
    let torrents = screen.torrents.clone();
    let label = match &media_override {
        Some(name) => name.clone(),
        None => format!("Global default ({default_name})"),
    };

    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().colors.muted_foreground)
                .child("Profile"),
        )
        .child(
            Button::new("entry-torrents-profile")
                .label(format!("{label} ▾"))
                .dropdown_menu(move |menu: PopupMenu, _, _| {
                    let mut menu = menu.item(
                        PopupMenuItem::new(format!("Global default ({default_name})"))
                            .checked(media_override.is_none())
                            .on_click({
                                let torrents = torrents.clone();
                                move |_, _, cx| {
                                    torrents
                                        .update(cx, |store, cx| store.set_media_profile(None, cx));
                                }
                            }),
                    );
                    for profile in &profiles {
                        let name = profile.name.clone();
                        let checked = media_override.as_deref() == Some(name.as_str())
                            || (media_override.is_none() && effective_name == name);
                        menu = menu.item(
                            PopupMenuItem::new(name.clone()).checked(checked).on_click({
                                let torrents = torrents.clone();
                                move |_, _, cx| {
                                    torrents.update(cx, |store, cx| {
                                        store.set_media_profile(Some(name.clone()), cx);
                                    });
                                }
                            }),
                        );
                    }
                    menu
                }),
        )
}

/// One episode row.
fn row(
    screen: &EntryScreen<'_>,
    episode: &TorrentEpisode,
    muted: Hsla,
    cx: &mut Context<'_, AppShell>,
) -> AnyElement {
    let (state, availability) = {
        let store = screen.torrents.read(cx);
        (
            store.state(episode.number).clone(),
            store.availability(episode.number),
        )
    };
    let unaired = !episode.aired;
    let id = episode.number;
    let torrents = screen.torrents.clone();

    let pills = div()
        .flex()
        .items_center()
        .gap_1()
        .children(
            availability
                .label()
                .map(|label| availability_pill(label, availability, cx)),
        )
        .children(episode.in_library.then(|| in_library_pill(cx)))
        .children(episode.watched.then(|| pill(WATCHED)));

    let title_row = div()
        .flex()
        .items_center()
        .gap_2()
        .flex_wrap()
        .child(
            div()
                .text_sm()
                .text_color(muted)
                .child(format!("Episode {id}")),
        )
        .child(episode.title.clone())
        .child(pills);

    let air = if unaired {
        div()
            .text_sm()
            .text_color(muted)
            .child(format!("airs {}", air_time(episode)))
    } else {
        div().text_sm().text_color(muted).child(air_time(episode))
    };

    let body = match &state {
        TorrentRowState::Running { stage } => running(*stage, &torrents, id).into_any_element(),
        TorrentRowState::Failed { reason } => div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_sm().text_color(muted).child(reason.clone()))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(choose(id, Some(reason.clone()), cx))
                    .child(stream_button(&torrents, id, unaired)),
            )
            .into_any_element(),
        TorrentRowState::Idle => div()
            .flex()
            .items_center()
            .gap_2()
            .child(stream_button(&torrents, id, unaired))
            .child(choose(id, None, cx))
            .into_any_element(),
    };

    div()
        .id(SharedString::from(format!("entry-torrents-episode-{id}")))
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .p_3()
        .rounded(px(8.))
        .opacity(if unaired { 0.55 } else { 1.0 })
        .child(div().flex().flex_col().gap_1().child(title_row).child(air))
        .child(body)
        .into_any_element()
}

/// The inline stage trail `Searching › Verifying files n/3 › Starting
/// stream` with Cancel (#220).
fn running(
    stage: TorrentStage,
    torrents: &gpui_kit::Entity<EntryTorrentStore>,
    id: u32,
) -> impl IntoElement {
    let current = match stage {
        TorrentStage::Searching => 0,
        TorrentStage::VerifyingFiles { .. } => 1,
        TorrentStage::StartingStream => 2,
    };
    let current_label = stage.label();
    let torrents = torrents.clone();
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .children(
            TRAIL
                .iter()
                .enumerate()
                .flat_map(|(index, label)| {
                    let label = if index == 1 && current == 1 {
                        current_label.clone()
                    } else {
                        (*label).to_owned()
                    };
                    let arrow = (index < TRAIL.len() - 1).then(|| "›".to_owned());
                    [Some(label), arrow].into_iter().flatten()
                })
                .map(|text| div().child(text)),
        )
        .child(
            Button::new(format!("entry-torrents-cancel-{id}"))
                .label("Cancel")
                .on_click(move |_, _, cx| torrents.update(cx, |store, cx| store.cancel(id, cx))),
        )
}

#[expect(clippy::fn_params_excessive_bools, reason = "id plus one flag")]
fn stream_button(
    torrents: &gpui_kit::Entity<EntryTorrentStore>,
    id: u32,
    unaired: bool,
) -> impl IntoElement {
    let torrents = torrents.clone();
    Button::new(format!("entry-torrents-stream-{id}"))
        .primary()
        .label("Stream")
        .disabled(unaired)
        .on_click(move |_, _, cx| torrents.update(cx, |store, cx| store.stream(id, cx)))
}

/// **Choose…** opens the release list for this episode (#221); on a failed
/// row the auto-select reason is carried into the sheet header.
fn choose(
    id: u32,
    failure_reason: Option<String>,
    cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    Button::new(format!("entry-torrents-choose-{id}"))
        .label(CHOOSE)
        .on_click(cx.listener(move |shell, _, window, cx| {
            shell.open_release_list(id, failure_reason.clone(), window, cx);
        }))
}

/// The `in library` pill; jumps to this episode in the Library tab (#182 Q3).
fn in_library_pill(cx: &mut Context<'_, AppShell>) -> impl IntoElement {
    let inner = pill(IN_LIBRARY);
    div()
        .id("entry-torrents-in-library")
        .child(inner)
        .on_mouse_down(
            gpui_kit::MouseButton::Left,
            cx.listener(|shell, _, _, cx| {
                shell.select_entry_tab(EntryTab::Library, cx);
            }),
        )
}

/// The `available` / `checking` / `waiting` pill (#182 Q3). `unknown` and
/// `none` have no label and render nothing.
fn availability_pill(
    label: &'static str,
    availability: Availability,
    cx: &Context<'_, AppShell>,
) -> impl IntoElement {
    let color = match availability {
        Availability::Available => hsla(142., 0.62, 0.45, 1.),
        Availability::Checking => cx.theme().colors.accent,
        Availability::Waiting => hsla(38., 0.92, 0.62, 1.),
        Availability::Unknown | Availability::None => cx.theme().colors.muted_foreground,
    };
    div()
        .rounded_full()
        .border_1()
        .px_2()
        .py_1()
        .text_sm()
        .text_color(color)
        .child(label.to_owned())
}

fn pill(label: &str) -> impl IntoElement {
    div()
        .rounded_full()
        .border_1()
        .px_2()
        .py_1()
        .text_sm()
        .child(label.to_owned())
}

fn air_time(episode: &TorrentEpisode) -> String {
    episode
        .air_date
        .clone()
        .unwrap_or_else(|| "unknown time".to_owned())
}
