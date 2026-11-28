//! The Entry page's Library tab (#132, #133): the missing-episodes banner,
//! sub-tabs, range chips, the grid/row toggle, selection with bulk actions,
//! and the episode cards, which play through the playback store.

use std::{path::PathBuf, rc::Rc};

use db::FileFlag;
use domain::ListStatus;
use gpui_kit::{
    App, Context, Entity, InteractiveElement, IntoElement, Modifiers, ParentElement, Resource,
    SharedString, Styled, TestSupportExt, Window,
    component::{
        ActiveTheme, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
    },
    div, image_cache, px,
};
use ui_components::{EpisodeActions, EpisodeCard, EpisodeCardView, EpisodeRow};
use ui_state::{
    EntryLibraryStore, EpisodeLayout, Gesture, LibraryEpisode, LibrarySection, PlaybackStore,
};

use crate::{entry::EntryScreen, shell::AppShell};

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

/// Sets the list entry's progress to `episode` through the Entry editor's
/// save, so the Overview draft and the list agree.
pub(crate) fn mark_watched_up_to(screen: &EntryScreen<'_>, episode: u32, cx: &mut App) {
    screen.store.update(cx, |store, cx| {
        if let Some(draft) = store.draft_mut() {
            draft.progress = episode;
            if draft.status == ListStatus::Planning {
                draft.status = ListStatus::Current;
            }
        }
        store.save(cx);
    });
    screen.library.update(cx, |library, cx| {
        library.set_progress(episode, cx);
        library.clear_selection(cx);
    });
}

/// The Library tab's body. `missing` is the banner text, if any.
pub(crate) fn render(
    screen: &EntryScreen<'_>,
    labels: &PlayLabels,
    missing: Option<String>,
    cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    let (library, cache) = (screen.library, screen.cache);
    let store = library.read(cx);
    let selected = store.selected();
    let section = store.section();
    let layout = store.layout();
    let episodes = store.episodes();
    let muted = cx.theme().colors.muted_foreground;

    let chips = range_chips(library, cx);

    let items = episodes.iter().map(|episode| {
        let id = SharedString::from(format!(
            "entry-episode-{}-{}",
            section.label(),
            episode.path.display()
        ));
        let view = EpisodeCardView {
            selected: selected.contains(&episode.path),
            ..card_view(episode)
        };
        let actions = episode_actions(screen, episode, labels);
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
        .child(toolbar(library, cx))
        .children(missing.map(|text| {
            div()
                .id("entry-library-missing")
                .test_support()
                .px_3()
                .py_2()
                .rounded(px(6.))
                .bg(cx.theme().colors.secondary)
                .text_sm()
                .child(text)
        }))
        .children(chips)
        .children((!selected.is_empty()).then(|| bulk_bar(screen, selected.len(), cx)))
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

/// The sub-tabs, Select all and the grid/row toggle.
fn toolbar(library: &Entity<EntryLibraryStore>, cx: &App) -> impl IntoElement {
    let store = library.read(cx);
    let sections = store.sections();
    let section = store.section();
    let layout = store.layout();
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

    let select_all = {
        let library = library.clone();
        Button::new("entry-library-select-all")
            .ghost()
            .small()
            .label("Select all")
            .on_click(move |_, _, cx| library.update(cx, EntryLibraryStore::select_all))
    };

    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .children(sub_tabs)
        .child(div().flex_1())
        .child(select_all)
        .child(toggle)
}

/// The range chips, for media past 100 episodes.
fn range_chips(library: &Entity<EntryLibraryStore>, cx: &App) -> Option<impl IntoElement> {
    let store = library.read(cx);
    let ranges = store.ranges();
    let range = store.range();
    (!ranges.is_empty()).then(|| {
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
    })
}

/// The actions of the selected cards, shown while a selection exists.
fn bulk_bar(
    screen: &EntryScreen<'_>,
    count: usize,
    cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    let library = screen.library.clone();
    let lock = |id: &'static str, label: &'static str, flag: FileFlag| {
        let library = library.clone();
        Button::new(id)
            .small()
            .label(label)
            .on_click(move |_, _, cx| library.update(cx, |store, cx| store.set_lock(flag, cx)))
    };
    let up_to = library.read(cx).watched_up_to();
    let mark = up_to.map(|episode| {
        let (store, editor, library, playback, cache) = (
            screen.store.clone(),
            screen.editor.clone(),
            screen.library.clone(),
            screen.playback.clone(),
            screen.cache.clone(),
        );
        let torrents = screen.torrents.clone();
        Button::new("entry-bulk-watched")
            .small()
            .label(format!("Mark watched up to {episode}"))
            .on_click(move |_, _, cx| {
                let screen = EntryScreen {
                    store: &store,
                    editor: &editor,
                    library: &library,
                    playback: &playback,
                    cache: &cache,
                    torrents: &torrents,
                };
                mark_watched_up_to(&screen, episode, cx);
            })
    });
    let clear = {
        let library = library.clone();
        Button::new("entry-bulk-clear")
            .ghost()
            .small()
            .label("Clear")
            .on_click(move |_, _, cx| library.update(cx, EntryLibraryStore::clear_selection))
    };
    div()
        .id("entry-library-bulk")
        .test_support()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(div().text_sm().child(format!("{count} selected")))
        .child(lock("entry-bulk-lock", "Lock", FileFlag::Set))
        .child(lock("entry-bulk-unlock", "Unlock", FileFlag::Clear))
        .child(
            Button::new("entry-bulk-rematch")
                .small()
                .label("Re-match…")
                .on_click(cx.listener(|shell, _, window, cx| shell.open_entry_rematch(window, cx))),
        )
        .children(mark)
        .child(
            Button::new("entry-bulk-trash")
                .small()
                .danger()
                .label("Move to trash…")
                .on_click(move |_, window, cx| {
                    let paths = library.read(cx).selected();
                    let library = library.clone();
                    AppShell::confirm_trash(
                        paths,
                        Rc::new(move |paths: &[PathBuf], cx: &mut App| {
                            library.update(cx, |store, cx| store.trash(paths, cx));
                        }),
                        window,
                        cx,
                    );
                }),
        )
        .child(clear)
}

/// The gesture a click with `modifiers` makes: Shift extends, Cmd or Ctrl
/// toggles.
fn gesture(modifiers: Modifiers) -> Gesture {
    if modifiers.shift {
        Gesture::Extend
    } else if modifiers.platform || modifiers.control {
        Gesture::Toggle
    } else {
        Gesture::Plain
    }
}

/// Click, play, mark-watched and open-folder actions for one card.
fn episode_actions(
    screen: &EntryScreen<'_>,
    episode: &LibraryEpisode,
    labels: &PlayLabels,
) -> EpisodeActions {
    let folder = episode.path.parent().map(std::path::Path::to_path_buf);
    let (library, playback) = (screen.library.clone(), screen.playback.clone());
    let play_action = {
        let (library, playback) = (library.clone(), playback.clone());
        let (episode, labels) = (episode.clone(), labels.clone());
        move |_: &mut Window, cx: &mut App| play(&library, &playback, &episode, &labels, cx)
    };
    let click = {
        let (episode, labels) = (episode.clone(), labels.clone());
        move |modifiers: Modifiers, _: &mut Window, cx: &mut App| {
            let plays = library.update(cx, |store, cx| {
                store.click(&episode.path, gesture(modifiers), cx)
            });
            if plays {
                play(&library, &playback, &episode, &labels, cx);
            }
        }
    };
    let mut actions = EpisodeActions::new().on_play(play_action).on_click(click);
    if episode.section == LibrarySection::Episodes {
        let (store, editor, library, playback, cache) = (
            screen.store.clone(),
            screen.editor.clone(),
            screen.library.clone(),
            screen.playback.clone(),
            screen.cache.clone(),
        );
        let torrents = screen.torrents.clone();
        let number = episode.number;
        actions = actions.on_mark_watched(move |_, cx| {
            let screen = EntryScreen {
                store: &store,
                editor: &editor,
                library: &library,
                playback: &playback,
                cache: &cache,
                torrents: &torrents,
            };
            mark_watched_up_to(&screen, number, cx);
        });
    }
    if let Some(folder) = folder {
        actions = actions.on_open_folder(move |_, cx| cx.open_with_system(&folder));
    }
    actions
}
