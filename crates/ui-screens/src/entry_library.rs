//! The Entry page's Library tab (#132): sub-tabs, range chips, the grid/row
//! toggle and the episode cards, which play through the playback store.

use gpui_kit::{
    App, Context, Entity, InteractiveElement, IntoElement, ParentElement, Resource, SharedString,
    Styled, TestSupportExt, Window,
    component::{
        ActiveTheme, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
    },
    div, image_cache,
};
use ui_components::{EpisodeActions, EpisodeCard, EpisodeCardView, EpisodeRow, LruImageCache};
use ui_state::{EntryLibraryStore, EpisodeLayout, LibraryEpisode, PlaybackStore};

use crate::shell::AppShell;

/// Why a shelved file can't play.
const SHELVED: &str = "Drive disconnected";

/// The labels a play request carries to the HUD.
#[derive(Debug, Clone)]
pub(crate) struct PlayLabels {
    /// The media's title.
    pub(crate) title: String,
    /// The media's poster URL.
    pub(crate) poster: Option<String>,
}

/// Plays `episode` through the playback manager, if it can play.
pub(crate) fn play(
    library: &Entity<EntryLibraryStore>,
    playback: &Entity<PlaybackStore>,
    episode: &LibraryEpisode,
    labels: &PlayLabels,
    cx: &mut App,
) {
    let request = library
        .read(cx)
        .play_request(episode, &labels.title, labels.poster.clone());
    if let Some(request) = request {
        playback.update(cx, |store, cx| store.play(request, cx));
    }
}

/// The Library tab's body.
pub(crate) fn render(
    library: &Entity<EntryLibraryStore>,
    playback: &Entity<PlaybackStore>,
    cache: &Entity<LruImageCache>,
    labels: &PlayLabels,
    cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    let store = library.read(cx);
    let sections = store.sections();
    let section = store.section();
    let ranges = store.ranges();
    let range = store.range();
    let layout = store.layout();
    let episodes = store.episodes();
    let muted = cx.theme().colors.muted_foreground;

    let sub_tabs = (sections.len() > 1).then(|| {
        div()
            .flex()
            .gap_2()
            .children(sections.into_iter().map(|candidate| {
                let library = library.clone();
                Button::new(SharedString::from(format!(
                    "entry-library-section-{}",
                    candidate.label()
                )))
                .label(candidate.label())
                .selected(candidate == section)
                .on_click(move |_, _, cx| {
                    library.update(cx, |store, cx| store.select_section(candidate, cx));
                })
            }))
    });
    let chips = (!ranges.is_empty()).then(|| {
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(ranges.into_iter().map(|candidate| {
                let library = library.clone();
                Button::new(SharedString::from(format!(
                    "entry-library-range-{}",
                    candidate.start
                )))
                .small()
                .label(candidate.label())
                .selected(Some(candidate) == range)
                .on_click(move |_, _, cx| {
                    library.update(cx, |store, cx| store.select_range(candidate, cx));
                })
            }))
    });
    let toggle = {
        let library = library.clone();
        Button::new("entry-library-layout")
            .ghost()
            .small()
            .label(match layout {
                EpisodeLayout::Grid => "Rows",
                EpisodeLayout::Rows => "Grid",
            })
            .on_click(move |_, _, cx| library.update(cx, EntryLibraryStore::toggle_layout))
    };

    let items = episodes.iter().map(|episode| {
        let id = SharedString::from(format!(
            "entry-episode-{}-{}",
            section.label(),
            episode.path.display()
        ));
        let view = card_view(episode);
        let actions = episode_actions(library, playback, episode, labels);
        match layout {
            EpisodeLayout::Grid => EpisodeCard::new(view, id)
                .actions(actions)
                .into_any_element(),
            EpisodeLayout::Rows => EpisodeRow::new(view, id)
                .actions(actions)
                .into_any_element(),
        }
    });
    let list = match layout {
        EpisodeLayout::Grid => div().flex().flex_wrap().gap_4(),
        EpisodeLayout::Rows => div().flex().flex_col().gap_3(),
    };

    div()
        .id("entry-library")
        .test_support()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .children(sub_tabs)
                .child(div().flex_1())
                .child(toggle),
        )
        .children(chips)
        .child(image_cache(cache.clone()).child(list.children(items)))
        .children(episodes.is_empty().then(|| {
            div()
                .text_sm()
                .text_color(muted)
                .child("No files in this range.")
        }))
}

/// The card view model of one library episode.
fn card_view(episode: &LibraryEpisode) -> EpisodeCardView {
    EpisodeCardView {
        thumbnail: episode
            .thumbnail
            .clone()
            .map(|url| Resource::Uri(url.into())),
        episode_number: episode.number,
        title: episode.title.clone().into(),
        filename: Some(episode.file_name().into()),
        filler: episode.filler,
        watched: episode.watched,
        locked: episode.locked,
        progress: episode.progress,
        unavailable: episode.shelved.then(|| SHELVED.into()),
        ..EpisodeCardView::default()
    }
}

/// Play and open-folder actions for one card.
fn episode_actions(
    library: &Entity<EntryLibraryStore>,
    playback: &Entity<PlaybackStore>,
    episode: &LibraryEpisode,
    labels: &PlayLabels,
) -> EpisodeActions {
    let folder = episode.path.parent().map(std::path::Path::to_path_buf);
    let (library, playback) = (library.clone(), playback.clone());
    let (episode, labels) = (episode.clone(), labels.clone());
    let mut actions = EpisodeActions::new().on_play(move |_: &mut Window, cx: &mut App| {
        play(&library, &playback, &episode, &labels, cx);
    });
    if let Some(folder) = folder {
        actions = actions.on_open_folder(move |_, cx| cx.open_with_system(&folder));
    }
    actions
}
