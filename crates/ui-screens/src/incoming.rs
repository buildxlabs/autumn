//! Match review (#223, #209 Q2-Q3, ADR-0009): attach an incoming torrent
//! to a Media and Episode before it can stream. Rendering is pure; all
//! state lives in [`IncomingStore`]. Nothing auto-commits.

use gpui_kit::{
    AnyElement, Context, Entity, IntoElement, ParentElement, Styled,
    component::{ActiveTheme, Disableable, button::Button, label::Label},
    div, px,
};
use ui_state::{IncomingStore, PlaybackStore, ReviewState};

/// Renders the match-review overlay content. `Cx` is the shell's context.
#[expect(clippy::too_many_lines, reason = "a flat match-review render")]
pub(crate) fn render_review(
    incoming: &Entity<IncomingStore>,
    playback: &Entity<PlaybackStore>,
    cx: &mut Context<'_, crate::shell::AppShell>,
) -> AnyElement {
    let colors = cx.theme().colors;
    let mut view = div().flex().flex_col().gap_3().w(px(520.0));

    {
        let store = incoming.read(cx);
        match &store.state {
            ReviewState::FindingFiles => {
                let incoming = incoming.clone();
                return view
                    .child(Label::new("Finding files…").text_color(colors.foreground))
                    .child(
                        Label::new("Added for metadata. Cancel drops it.")
                            .text_color(colors.muted_foreground),
                    )
                    .child(
                        Button::new("incoming-cancel-finding")
                            .label("Cancel")
                            .on_click(move |_, _, cx| {
                                incoming.update(cx, ui_state::IncomingStore::cancel);
                            }),
                    )
                    .into_any_element();
            }
            ReviewState::Rejected(message) => {
                let message = message.clone();
                let incoming = incoming.clone();
                return view
                    .child(Label::new(message).text_color(colors.muted_foreground))
                    .child(Button::new("incoming-close").label("Close").on_click(
                        move |_, _, cx| {
                            incoming.update(cx, ui_state::IncomingStore::cancel);
                        },
                    ))
                    .into_any_element();
            }
            ReviewState::Duplicate(_) => {
                view = view.child(
                    Label::new("This torrent is already in your Torrent List.")
                        .text_color(colors.muted_foreground),
                );
            }
            ReviewState::Idle | ReviewState::Reviewing | _ => {}
        }
    }

    let Some(review) = incoming.read(cx).review.clone() else {
        return view.into_any_element();
    };

    view = view.child(
        Label::new(
            review
                .name
                .clone()
                .unwrap_or_else(|| "Incoming torrent".to_owned()),
        )
        .text_color(colors.foreground),
    );

    if !review.suggestions.is_empty() {
        view = view.child(Label::new("Match").text_color(colors.muted_foreground));
        view = view.children(review.suggestions.iter().enumerate().map(
            |(index, candidate)| -> AnyElement {
                let chosen = review.chosen.as_ref() == Some(candidate);
                let candidate = candidate.clone();
                let incoming = incoming.clone();
                Button::new(format!("incoming-suggestion-{index}"))
                    .label(if chosen {
                        format!("✓ {}", candidate.title)
                    } else {
                        candidate.title.clone()
                    })
                    .on_click(move |_, _, cx| {
                        let candidate = candidate.clone();
                        incoming.update(cx, |store, cx| store.choose(candidate, cx));
                    })
                    .into_any_element()
            },
        ));
    } else if review.failed {
        view = view.child(
            Label::new("No match. Pick a media and episode.").text_color(colors.muted_foreground),
        );
    }

    let episode = review.episode;
    let inc = incoming.clone();
    let dec = incoming.clone();
    view = view.child(
        div()
            .flex()
            .gap_2()
            .items_center()
            .child(Label::new("Episode").text_color(colors.muted_foreground))
            .child(
                Button::new("incoming-episode-dec")
                    .label("−")
                    .on_click(move |_, _, cx| {
                        dec.update(cx, |store, cx| {
                            store.set_episode(episode.saturating_sub(1), cx);
                        });
                    }),
            )
            .child(Label::new(episode.to_string()).text_color(colors.foreground))
            .child(
                Button::new("incoming-episode-inc")
                    .label("+")
                    .on_click(move |_, _, cx| {
                        inc.update(cx, |store, cx| store.set_episode(episode + 1, cx));
                    }),
            ),
    );

    let has_media = review.chosen.is_some();
    let incoming_stream = incoming.clone();
    let incoming_add = incoming.clone();
    let incoming_cancel = incoming.clone();
    let playback = playback.clone();
    view = view.child(
        div()
            .flex()
            .gap_2()
            .child(
                Button::new("incoming-stream")
                    .label("Stream")
                    .disabled(!has_media)
                    .on_click(move |_, _, cx| {
                        let playback = playback.clone();
                        incoming_stream.update(cx, |store, cx| {
                            store.confirm(
                                false,
                                None,
                                cx,
                                move |media, episode, info_hash, file_index, cx| {
                                    playback.update(cx, |p, cx| {
                                        p.play_torrent_file(
                                            media, episode, info_hash, file_index, cx,
                                        );
                                    });
                                },
                            );
                        });
                    }),
            )
            .child(
                Button::new("incoming-add-to-list")
                    .label("Add to Torrent List")
                    .on_click(move |_, _, cx| {
                        incoming_add.update(cx, |store, cx| {
                            store.confirm(true, None, cx, |_, _, _, _, _| {});
                        });
                    }),
            )
            .child(
                Button::new("incoming-cancel")
                    .label("Cancel")
                    .on_click(move |_, _, cx| {
                        incoming_cancel.update(cx, ui_state::IncomingStore::cancel);
                    }),
            ),
    );

    view.into_any_element()
}
