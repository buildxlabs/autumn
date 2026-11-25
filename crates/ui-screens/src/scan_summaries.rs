//! The scan-summaries screen: an audit log of recent scans (#134).
//!
//! A list on the left selects a scan; the detail pane on the right shows its
//! folders, matched media and unmatched files. Matching uses a small AniList
//! search dialog, independent of the media-search work in #133.

use std::{collections::HashMap, path::PathBuf, rc::Rc, time::Duration};

use db::{ScanSummary, UnmatchedReason};
use gpui_kit::{
    App, AppContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle, InteractiveElement,
    IntoElement, ParentElement, Render, ScrollHandle, StatefulInteractiveElement, Styled,
    Subscription, Task, TestSupportExt, Window,
    component::{
        ActiveTheme, WindowExt,
        button::{Button, ButtonVariants},
        dialog::Cancel,
        input::{Input, InputEvent, InputState},
        label::Label,
        scroll::ScrollableElement,
    },
    div, px,
};
use metadata::{AniList, MediaSummary, PageRequest, SearchFilters};
use ui_state::{LibraryMediaInfo, PendingEdit, ScanSummariesStore, bridge};

use crate::palette_search::display_label;

/// How long the search dialog waits after typing before asking AniList.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
const MAX_RESULTS: u32 = 12;

/// What the search dialog tells the screen after a match is chosen.
#[derive(Debug, Clone)]
pub(crate) enum MediaSearchEvent {
    /// Match `path` to the chosen AniList media.
    Matched {
        /// The file the user came from.
        path: PathBuf,
        /// The AniList media the user chose.
        anilist_id: u32,
    },
}

/// A small AniList search used by Match… in the summaries detail pane.
pub(crate) struct MediaSearchDialog {
    input: Entity<InputState>,
    focus: FocusHandle,
    results: Vec<MediaSummary>,
    path: Option<PathBuf>,
    anilist: AniList,
    search: Option<Task<()>>,
    _input_events: Subscription,
}

impl EventEmitter<MediaSearchEvent> for MediaSearchDialog {}

impl MediaSearchDialog {
    pub(crate) fn new(window: &mut Window, anilist: AniList, cx: &mut Context<'_, Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search AniList title"));
        let focus = cx.focus_handle();
        let input_events =
            cx.subscribe(&input, |this, _, _: &InputEvent, cx| this.query_changed(cx));
        Self {
            input,
            focus,
            results: Vec::new(),
            path: None,
            anilist,
            search: None,
            _input_events: input_events,
        }
    }

    /// Starts with `path` as the file to match, and focuses the query.
    pub(crate) fn open(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.path = Some(path);
        self.results.clear();
        self.search = None;
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn query_changed(&mut self, cx: &mut Context<'_, Self>) {
        let query = self.input.read(cx).value().to_string();
        self.search = None;
        if query.trim().is_empty() {
            self.results.clear();
            cx.notify();
            return;
        }
        let anilist = self.anilist.clone();
        self.search = Some(cx.spawn(async move |dialog, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let fetch = cx.update(|cx| {
                bridge::run(cx, async move {
                    anilist
                        .search(
                            &SearchFilters {
                                search: Some(query),
                                ..SearchFilters::default()
                            },
                            PageRequest::new(1, MAX_RESULTS),
                        )
                        .await
                })
            });
            let results = fetch.await.ok().and_then(Result::ok).map(|page| page.media);
            let _ = dialog.update(cx, |dialog, cx| {
                dialog.results = results.unwrap_or_default();
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        let (Some(path), Some(media)) = (self.path.take(), self.results.get(index).cloned()) else {
            return;
        };
        cx.emit(MediaSearchEvent::Matched {
            path,
            anilist_id: media.anilist_id,
        });
        window.close_dialog(cx);
    }

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.path = None;
        window.close_dialog(cx);
    }
}

impl Render for MediaSearchDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let results = self.results.clone();
        let dialog = cx.entity();
        div()
            .id("media-search-dialog")
            .key_context("MediaSearchDialog")
            .capture_action(cx.listener(Self::cancel))
            .flex()
            .flex_col()
            .gap_3()
            .w(px(440.))
            .track_focus(&self.focus)
            .child(Input::new(&self.input))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(results.into_iter().enumerate().map(|(index, media)| {
                        let dialog = dialog.clone();
                        Button::new(format!("media-result-{index}"))
                            .label(display_label(&media))
                            .on_click(move |_: &ClickEvent, window, cx| {
                                dialog.update(cx, |dialog, cx| dialog.confirm(index, window, cx));
                            })
                    })),
            )
    }
}

type OpenEntry = dyn Fn(u32, &mut Window, &mut App) + 'static;
type OpenMatch = Rc<dyn Fn(PathBuf, &mut Window, &mut App)>;

/// The screen hosted by [`crate::AppShell`] for the Scan Summaries route.
pub(crate) struct ScanSummariesScreen {
    store: Entity<ScanSummariesStore>,
    dialog: Entity<MediaSearchDialog>,
    _dialog_events: Subscription,
    on_open: Option<Rc<OpenEntry>>,
    scroll: ScrollHandle,
}

impl std::fmt::Debug for ScanSummariesScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanSummariesScreen")
            .finish_non_exhaustive()
    }
}

impl ScanSummariesScreen {
    /// Builds the screen over the shared summaries store.
    pub(crate) fn new(
        store: Entity<ScanSummariesStore>,
        anilist: AniList,
        scroll: ScrollHandle,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let dialog = cx.new(|cx| MediaSearchDialog::new(window, anilist, cx));
        let store_for_dialog = store.clone();
        let events = cx.subscribe(&dialog, move |_, _, event, cx| {
            let MediaSearchEvent::Matched { path, anilist_id } = event;
            store_for_dialog.update(cx, |store, cx| {
                store.match_file(path.clone(), *anilist_id, None, cx);
            });
        });
        Self {
            store,
            dialog,
            _dialog_events: events,
            on_open: None,
            scroll,
        }
    }

    /// Sets the callback that opens an Entry when a matched media is clicked.
    pub(crate) fn set_on_open(&mut self, on_open: impl Fn(u32, &mut Window, &mut App) + 'static) {
        self.on_open = Some(Rc::new(on_open));
    }

    fn open_dialog(&self, path: PathBuf, window: &mut Window, cx: &mut Context<'_, Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let dialog = self.dialog.clone();
        window.open_dialog(cx, move |dialog_view, _, _| {
            dialog_view
                .title("Match a local file")
                .child(dialog.clone())
        });
        self.dialog
            .update(cx, |dialog, cx| dialog.open(path, window, cx));
    }
}

impl Render for ScanSummariesScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let list: Vec<(usize, String, usize, usize)> = self.store.read_with(cx, |store, _| {
            store
                .summaries()
                .iter()
                .enumerate()
                .map(|(index, summary)| {
                    (
                        index,
                        summary_time(summary),
                        summary.files_matched,
                        summary.files_unmatched,
                    )
                })
                .collect()
        });
        let selected_index = self.store.read_with(cx, |store, _| store.selected_index());
        let selected = self
            .store
            .read_with(cx, |store, _| store.selected().cloned());
        let colors = cx.theme().colors;
        let list = div()
            .w(px(280.))
            .flex_none()
            .overflow_y_scrollbar()
            .flex()
            .flex_col()
            .gap_1()
            .children(list.iter().map(|(index, time, matched, unmatched)| {
                let index = *index;
                let store = self.store.clone();
                let button = Button::new(format!("summary-{index}"))
                    .label(format!("{time} · {matched} matched, {unmatched} unmatched"));
                let button = if Some(index) == selected_index {
                    button.primary()
                } else {
                    button.ghost()
                };
                button.on_click(move |_, _, cx| {
                    store.update(cx, |store, cx| store.select(index, cx));
                })
            }));
        let screen = cx.entity().downgrade();
        let open_match: OpenMatch = Rc::new(move |path, window, cx| {
            let _ = screen.update(cx, |screen, cx| screen.open_dialog(path, window, cx));
        });
        let detail = div()
            .id("scan-summaries-detail")
            .flex_1()
            .min_w_0()
            .track_scroll(&self.scroll)
            .overflow_y_scrollbar()
            .child(selected.map_or_else(
                || {
                    Label::new("No scan has run yet")
                        .text_color(colors.muted_foreground)
                        .into_any_element()
                },
                |summary| {
                    let (media, pending) = self.store.read_with(cx, |store, _| {
                        (store.media().to_vec(), store.pending().clone())
                    });
                    render_detail(
                        &summary,
                        &media,
                        &pending,
                        &self.store,
                        &open_match,
                        self.on_open.as_ref(),
                        &mut *cx,
                    )
                    .into_any_element()
                },
            ));
        div()
            .id("scan-summaries")
            .test_support()
            .size_full()
            .flex()
            .bg(colors.background)
            .child(
                div()
                    .h_full()
                    .border_r_1()
                    .border_color(colors.border)
                    .child(list),
            )
            .child(detail)
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one renderer builds the summary, media and unmatched sections"
)]
fn render_detail(
    summary: &ScanSummary,
    media: &[LibraryMediaInfo],
    pending: &HashMap<PathBuf, PendingEdit>,
    store: &Entity<ScanSummariesStore>,
    open_match: &OpenMatch,
    on_open: Option<&Rc<OpenEntry>>,
    cx: &mut App,
) -> impl IntoElement {
    let mut column = div()
        .flex()
        .flex_col()
        .gap_3()
        .p_8()
        .child(Label::new("Scan summary"))
        .child(
            Label::new(format!(
                "Started {} · took {}s · {} files walked, {} matched, {} unmatched, {} shelved",
                summary_time(summary),
                summary.duration.as_secs(),
                summary.files_walked,
                summary.files_matched,
                summary.files_unmatched,
                summary.shelved
            ))
            .text_sm(),
        )
        .child(Label::new(format!("Folders: {}", paths(&summary.folders))).text_sm());

    if !summary.by_media.is_empty() {
        column =
            column.child(Label::new("Matched media")).child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(summary.by_media.iter().map(|entry| {
                        let info = media.iter().find(|info| info.media == entry.media);
                        let title = info
                            .and_then(|info| info.title.clone())
                            .unwrap_or_else(|| format!("Media {}", entry.media.get()));
                        let low = entry.lowest_confidence < 0.6;
                        let mut row = div().flex().items_center().justify_between().gap_3().child(
                            Label::new(format!(
                                "{title} · {} files · lowest confidence {:.2}",
                                entry.files, entry.lowest_confidence
                            )),
                        );
                        if low {
                            row = row.child(Label::new("Low confidence").text_sm());
                        }
                        match info.and_then(|info| info.anilist_id).zip(on_open.cloned()) {
                            Some((anilist_id, open)) => row.child(
                                Button::new(format!("entry-{}", entry.media.get()))
                                    .label("Open entry")
                                    .on_click(move |_, window, cx| open(anilist_id, window, cx)),
                            ),
                            None => row,
                        }
                    })),
            );
    }

    if !summary.unmatched.is_empty() {
        column = column.child(Label::new("Unmatched files")).child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .children(summary.unmatched.iter().map(|file| {
                    let path = file.path.clone();
                    let open_match = Rc::clone(open_match);
                    let match_path = path.clone();
                    let ignore_store = store.clone();
                    let ignore_path = path.clone();
                    let applied = pending.get(&path).copied();
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(Label::new(path.to_string_lossy().to_string()))
                        .child(applied.map_or_else(
                            || {
                                Label::new(reason(&file.reason))
                                    .text_sm()
                                    .into_any_element()
                            },
                            |edit| {
                                let label = match edit {
                                    PendingEdit::Matched(id) => {
                                        format!("Matched to AniList media {id}")
                                    }
                                    PendingEdit::Ignored => "Ignored".to_owned(),
                                    _ => "Updated".to_owned(),
                                };
                                Label::new(label).text_sm().into_any_element()
                            },
                        ))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    Button::new(format!("match-{}", path.display()))
                                        .label("Match…")
                                        .on_click(move |_, window, cx| {
                                            open_match(match_path.clone(), window, cx);
                                        }),
                                )
                                .child(
                                    Button::new(format!("ignore-{}", path.display()))
                                        .label("Ignore")
                                        .on_click(move |_, _, cx| {
                                            ignore_store.update(cx, |store, cx| {
                                                store.ignore(ignore_path.clone(), cx);
                                            });
                                        }),
                                ),
                        )
                })),
        );
    }
    let _ = cx;
    column
}

fn summary_time(summary: &ScanSummary) -> String {
    let Ok(elapsed) = summary.started_at.duration_since(std::time::UNIX_EPOCH) else {
        return "unknown".to_owned();
    };
    let Ok(seconds) = i64::try_from(elapsed.as_secs()) else {
        return "unknown".to_owned();
    };
    time::OffsetDateTime::from_unix_timestamp(seconds).map_or_else(
        |_| "unknown".to_owned(),
        |stamp| {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                stamp.year(),
                u8::from(stamp.month()),
                stamp.day(),
                stamp.hour(),
                stamp.minute()
            )
        },
    )
}

fn paths(folders: &[PathBuf]) -> String {
    folders
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join(", ")
}

fn reason(reason: &UnmatchedReason) -> String {
    match reason {
        UnmatchedReason::BelowThreshold { best_score, .. } => {
            format!("no candidate above threshold (best {best_score:.2})")
        }
        UnmatchedReason::NoCandidate => "no candidate".to_owned(),
        UnmatchedReason::NoTitle => "no title in the filename".to_owned(),
        UnmatchedReason::Ignored => "ignored".to_owned(),
        UnmatchedReason::Shelved => "on a disconnected drive".to_owned(),
        _ => "unknown reason".to_owned(),
    }
}
