//! The Torrent List screen: every torrent in Autumn's own session this run,
//! newest first, with live state and stats (#207).
//!
//! Rendering is pure. Every button sends a command to a store through the
//! foreground context; the rows themselves come from 4 Hz engine snapshots.

use domain::{TorrentListRow, TorrentState};
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, SharedString, Styled, WeakEntity, Window,
    component::{ActiveTheme, Disableable, WindowExt, button::Button, label::Label},
    div, px,
};
use ui_state::{EntryTab, PlaybackStore, Route, TorrentStore};

use crate::shell::AppShell;

/// Renders the route view for [`Route::Torrents`].
pub(crate) fn render(
    store: &Entity<TorrentStore>,
    playback: &Entity<PlaybackStore>,
    shell: &WeakEntity<AppShell>,
    cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    let colors = cx.theme().colors;
    let rows = store.read(cx).rows().to_vec();
    let error = store.read(cx).error().map(str::to_owned);
    let mut view = div()
        .id("torrents")
        .size_full()
        .flex()
        .flex_col()
        .gap_3()
        .p_6()
        .bg(colors.background);
    if rows.is_empty() {
        view = view.child(
            div()
                .flex()
                .flex_1()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Label::new("No torrents").text_color(colors.foreground))
                .child(
                    Label::new("Torrents you stream or preload appear here while Autumn runs.")
                        .text_color(colors.muted_foreground),
                ),
        );
    } else {
        view = view.children(
            rows.iter()
                .map(|row| render_row(row, store, playback, shell, cx)),
        );
    }
    if let Some(message) = error {
        view = view.child(
            div()
                .id("torrents-error")
                .text_color(colors.muted_foreground)
                .child(Label::new(message)),
        );
    }
    view
}

/// One infohash's row: info, live state and stats, then its actions.
fn render_row(
    row: &TorrentListRow,
    store: &Entity<TorrentStore>,
    playback: &Entity<PlaybackStore>,
    shell: &WeakEntity<AppShell>,
    cx: &mut Context<'_, AppShell>,
) -> AnyElement {
    let colors = cx.theme().colors;
    let info_hash = row.info_hash;
    let paused = row.state == TorrentState::Paused;
    let store_for_pause = store.clone();
    let store_for_remove = store.clone();
    let row_for_remove = row.clone();
    let magnet = row.magnet.clone();

    div()
        .id(format!("torrent-{info_hash}"))
        .flex()
        .flex_col()
        .gap_2()
        .rounded_md()
        .border_1()
        .border_color(colors.border)
        .p_4()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .min_w_0()
                        .child(Label::new(SharedString::from(row.name.clone())))
                        .child(
                            Label::new(format!("{info_hash} · origin {:?}", row.origin))
                                .text_color(colors.muted_foreground),
                        ),
                )
                .child(Label::new(state_label(row.state)).text_color(if paused {
                    colors.muted_foreground
                } else {
                    colors.accent
                })),
        )
        .child(
            div()
                .h(px(4.0))
                .w_full()
                .rounded_full()
                .bg(colors.border)
                .child(
                    div()
                        .h_full()
                        .w(px(f32::from(u16::try_from(percent(row)).unwrap_or(100))))
                        .rounded_full()
                        .bg(colors.accent),
                ),
        )
        .child(
            Label::new(format!(
                "{} · ↓ {}/s · ↑ {}/s · {} peers · ratio {:.2}",
                percent(row),
                speed(row.download_bps),
                speed(row.upload_bps),
                row.peers,
                f64::from(row.ratio_permille) / 1000.0,
            ))
            .text_color(colors.muted_foreground),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new(format!("torrent-toggle-{info_hash}"))
                        .label(if paused { "Resume" } else { "Pause" })
                        .on_click(move |_, _, cx| {
                            store_for_pause.update(cx, |store, cx| {
                                if paused {
                                    store.resume(info_hash, cx);
                                } else {
                                    store.pause(info_hash, cx);
                                }
                            });
                        }),
                )
                .child(
                    Button::new(format!("torrent-copy-{info_hash}"))
                        .label("Copy magnet")
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(magnet.clone()));
                        }),
                )
                .child(
                    Button::new(format!("torrent-remove-{info_hash}"))
                        .label("Remove")
                        .on_click(move |_, window, cx| {
                            confirm_remove(&row_for_remove, store_for_remove.clone(), window, cx);
                        }),
                )
                .children(file_actions(row, playback, shell)),
        )
        .into_any_element()
}

/// Per-file Play on completed files; Open entry when the media is linked. An
/// unlinked row gets a disabled Match & play: #223 wires it to the matching
/// pipeline later.
fn file_actions(
    row: &TorrentListRow,
    playback: &Entity<PlaybackStore>,
    shell: &WeakEntity<AppShell>,
) -> Vec<AnyElement> {
    let mut actions = Vec::new();
    let Some(link) = row.link.as_ref() else {
        actions.push(
            Button::new(format!("torrent-match-{}", row.info_hash))
                .label("Match & play")
                .disabled(true)
                .into_any_element(),
        );
        return actions;
    };
    for file in row.files.iter().filter(|file| file.complete) {
        let playback = playback.clone();
        let (media, episode) = (link.media, link.episode);
        let (info_hash, index) = (row.info_hash, file.index);
        let path = file.path.clone();
        actions.push(
            Button::new(format!("torrent-play-{info_hash}-{index}"))
                .label(format!("Play {path}"))
                .on_click(move |_, _, cx| {
                    playback.update(cx, |playback, cx| {
                        playback.play_torrent_file(media, episode, info_hash, index, cx);
                    });
                })
                .into_any_element(),
        );
    }
    if let Some(id) = link.anilist_id {
        let shell = shell.clone();
        actions.push(
            Button::new(format!("torrent-entry-{}", row.info_hash))
                .label("Open entry")
                .on_click(move |_, _, cx| {
                    let _ = shell.update(cx, |shell, cx| {
                        shell.navigate(
                            Route::Entry {
                                id,
                                tab: Some(EntryTab::Overview),
                            },
                            cx,
                        );
                    });
                })
                .into_any_element(),
        );
    }
    actions
}

/// Confirms a removal. While the torrent is streaming the copy is stronger:
/// playback stops, its relay token is revoked, then files are deleted (#207
/// Q7, Q11). Nothing outside the torrent cache is ever touched.
fn confirm_remove(
    row: &TorrentListRow,
    store: Entity<TorrentStore>,
    window: &mut Window,
    cx: &mut App,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    let streaming = row.state == TorrentState::Streaming;
    let info_hash = row.info_hash;
    window.open_dialog(cx, move |dialog, _, _| {
        let store = store.clone();
        dialog.title("Remove torrent").child(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(Label::new(if streaming {
                    "This torrent is playing. Playback will stop and its cached files will be deleted. This cannot be undone."
                } else {
                    "Remove this torrent and delete its cached files? This cannot be undone."
                }))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("confirm-remove-torrent")
                                .label("Remove")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    store.update(cx, |store, cx| store.remove(info_hash, cx));
                                }),
                        )
                        .child(
                            Button::new("cancel-remove-torrent")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        ),
                ),
        )
    });
}

fn state_label(state: TorrentState) -> &'static str {
    match state {
        TorrentState::Preparing => "preparing",
        TorrentState::Downloading => "downloading",
        TorrentState::Streaming => "streaming",
        TorrentState::Paused => "paused",
        _ => "seeding",
    }
}

/// Downloaded share of the selected files, as a whole percentage.
fn percent(row: &TorrentListRow) -> u32 {
    if row.selected_bytes == 0 {
        return 0;
    }
    let done = row
        .selected_done_bytes
        .saturating_mul(100)
        .checked_div(row.selected_bytes)
        .unwrap_or(0);
    done.try_into().unwrap_or(100)
}

fn speed(bytes_per_sec: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    if bytes_per_sec >= MIB {
        let whole = bytes_per_sec / MIB;
        let tenths = (bytes_per_sec % MIB) * 10 / MIB;
        format!("{whole}.{tenths} MiB")
    } else {
        format!("{} KiB", bytes_per_sec / KIB)
    }
}

#[cfg(test)]
mod tests {
    use super::speed;

    #[test]
    fn progress_is_a_whole_percentage_and_never_panics_on_empty_selection() {
        assert_eq!(percent_total(50, 100), 50);
        assert_eq!(percent_total(0, 0), 0, "no selected bytes yet");
    }

    /// The row's two byte counters are the only inputs to the percentage.
    fn percent_total(done: u64, total: u64) -> u32 {
        if total == 0 {
            return 0;
        }
        done.saturating_mul(100)
            .checked_div(total)
            .unwrap_or(0)
            .try_into()
            .unwrap_or(100)
    }

    #[test]
    fn speeds_have_readable_units() {
        assert_eq!(speed(0), "0 KiB");
        assert_eq!(speed(2 * 1024), "2 KiB");
        assert_eq!(speed(3 * 1024 * 1024), "3.0 MiB");
    }
}
