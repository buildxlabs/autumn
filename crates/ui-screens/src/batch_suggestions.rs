//! Batch suggestion strip on the Torrents tab (#227, #182 Q4).
//!
//! Above the episode list, only for finished media with more than one
//! episode. Each card shows group, range, resolution, codec, size, seeders
//! and indexer plus **Episodes ▾**, which expands the file list on demand.
//! Picking a file streams it. All logic lives in `ui_state::batch_suggestions`;
//! this view only renders, reusing the release list's batch file pick.

use gpui_kit::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled,
    component::{
        ActiveTheme, Disableable as _,
        button::{Button, ButtonVariants as _},
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use ui_state::{EntryTorrentStore, batch_suggestions::BatchSuggestion, release_list::BatchState};

use crate::release_list::format_size;

/// Renders the strip; `None` when it does not show (#227).
pub(crate) fn render(
    torrents: &gpui_kit::Entity<EntryTorrentStore>,
    cx: &mut Context<'_, crate::shell::AppShell>,
) -> Option<AnyElement> {
    let store = torrents.read(cx);
    if !store.should_show_batch_suggestions() {
        return None;
    }
    let muted = cx.theme().colors.muted_foreground;
    let loading = store.batch_suggestions_loading();
    let error = store.batch_suggestions_error().map(str::to_owned);
    let has_error = error.is_some();
    let expanded = store.batch_expanded().map(str::to_owned);
    let cards: Vec<BatchSuggestion> = store.batch_suggestions().to_vec();
    let states: Vec<(String, BatchState)> = cards
        .iter()
        .map(|card| {
            (
                card.infohash.clone(),
                store.batch_state(&card.infohash).clone(),
            )
        })
        .collect();

    let mut strip = div()
        .id("entry-torrents-batch-suggestions")
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(8.))
        .border_1()
        .child(div().text_sm().text_color(muted).child("Batch suggestions"));
    if loading {
        strip = strip.child(div().text_sm().text_color(muted).child("Searching…"));
    }
    if let Some(error) = error {
        strip = strip.child(div().text_sm().text_color(muted).child(error));
    }
    if !loading && cards.is_empty() && !has_error {
        strip = strip.child(div().text_sm().text_color(muted).child("No batch found."));
    }
    for card in cards {
        let state = states
            .iter()
            .find(|(hash, _)| hash == &card.infohash)
            .map_or(BatchState::Loading, |(_, state)| state.clone());
        strip = strip.child(card_element(
            torrents,
            &card,
            &state,
            expanded.as_deref() == Some(card.infohash.as_str()),
            muted,
        ));
    }
    Some(strip.into_any_element())
}

/// One batch card with its **Episodes ▾** file list.
#[expect(clippy::fn_params_excessive_bools, reason = "one flag: expanded")]
fn card_element(
    torrents: &gpui_kit::Entity<EntryTorrentStore>,
    card: &BatchSuggestion,
    state: &BatchState,
    expanded: bool,
    muted: Hsla,
) -> impl IntoElement {
    let hash = card.infohash.clone();
    let details = [
        card.group.clone(),
        Some(card.range.clone()),
        card.resolution.map(|value| format!("{value}p")),
        card.codec.clone(),
        card.size.map(format_size),
        card.seeders.map(|value| format!("{value} seeders")),
        Some(card.indexer.label().to_owned()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");

    let mut element = div()
        .id(SharedString::from(format!("batch-suggestion-{hash}")))
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded(px(6.))
        .border_1()
        .child(div().text_sm().child(card.name.clone()))
        .child(div().text_sm().text_color(muted).child(details));

    let button_torrents = torrents.clone();
    let button_hash = hash.clone();
    element = element.child(
        Button::new(format!("batch-suggestion-episodes-{hash}"))
            .label(if expanded {
                "Episodes ▲"
            } else {
                "Episodes ▾"
            })
            .on_click(move |_, _, cx| {
                button_torrents.update(cx, |store, cx| {
                    store.toggle_batch_episodes(&button_hash, cx);
                });
            }),
    );

    if expanded {
        element = element.child(files(torrents, &hash, state, muted));
    }
    element
}

fn files(
    torrents: &gpui_kit::Entity<EntryTorrentStore>,
    hash: &str,
    state: &BatchState,
    muted: Hsla,
) -> impl IntoElement {
    let mut list = div().flex().flex_col().gap_1().p_2();
    match state {
        BatchState::Loading => {
            list = list.child(div().text_sm().text_color(muted).child("Loading…"));
        }
        BatchState::Failed => {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Could not load file list."),
            );
        }
        BatchState::Loaded(files) => {
            for file in files {
                let label = match file.episode {
                    Some(episode) => format!("E{episode} · {}", file.name),
                    None => file.name.clone(),
                };
                let torrents = torrents.clone();
                let file_hash = hash.to_owned();
                let index = file.index;
                let path = file.path.clone();
                let playable = file.playable;
                let preselected = file.preselected;
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .opacity(if playable { 1.0 } else { 0.4 })
                        .child(div().flex_1().text_sm().child(label))
                        .child(div().text_sm().child(format_size(file.size)))
                        .child(
                            Button::new(format!("batch-suggestion-file-{file_hash}-{index}"))
                                .label(if preselected { "▶ Stream" } else { "Stream" })
                                .disabled(!playable)
                                .when(preselected, Button::primary)
                                .on_click(move |_, _, cx| {
                                    let path = path.clone();
                                    torrents.update(cx, |store, cx| {
                                        store.stream_batch_suggestion_file(
                                            &file_hash, index, &path, cx,
                                        );
                                    });
                                }),
                        ),
                );
            }
        }
        _ => {}
    }
    list
}
