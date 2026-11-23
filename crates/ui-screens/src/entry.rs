//! The Anime Entry route (#58): the media card and the Local list editor.
//!
//! The hero header and the Relations/Recommendations carousels land with #56;
//! this module covers the parts that need only the card and the store's
//! editor draft.

use std::time::{SystemTime, UNIX_EPOCH};

use domain::{FuzzyDate, ListStatus, Score};
use gpui_kit::{
    App, AppContext, Context, ElementId, Entity, IntoElement, ParentElement, SharedString, Styled,
    Window,
    component::{
        button::Button,
        input::{Input, InputState},
        label::Label,
    },
    div,
};
use ui_components::{
    MediaCard, MediaCardView, MediaFormat,
    fetch_status::{FetchError, error_card, fetch_loader},
};
use ui_state::entry_store::{EntryDraft, EntryState, EntryStore};

use crate::shell::AppShell;

/// The statuses the editor offers, in AniList's order.
const STATUSES: [ListStatus; 6] = [
    ListStatus::Current,
    ListStatus::Planning,
    ListStatus::Completed,
    ListStatus::Dropped,
    ListStatus::Paused,
    ListStatus::Repeating,
];

/// The Entry editor's local UI state: the notes input and the Entry it was
/// last loaded for.
pub(crate) struct EntryEditor {
    notes: Entity<InputState>,
    synced_for: Option<u32>,
}

impl EntryEditor {
    /// Creates the editor's notes input.
    pub(crate) fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
        let notes = cx.new(|cx| InputState::new(window, cx).placeholder("Notes"));
        cx.new(|_| Self {
            notes,
            synced_for: None,
        })
    }

    /// The value currently in the notes input.
    fn notes(&self, cx: &App) -> String {
        self.notes.read(cx).value().to_string()
    }

    /// Loads the draft's notes into the input once per opened Entry, so a
    /// render caused by another field edit never overwrites what the user
    /// typed.
    fn sync(&mut self, id: u32, notes: &str, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.synced_for == Some(id) {
            return;
        }
        self.synced_for = Some(id);
        self.notes.update(cx, |input, cx| {
            input.set_value(notes.to_owned(), window, cx);
        });
    }
}

/// Renders the Entry route from the store's current state.
pub(crate) fn render(
    entry_store: &Entity<EntryStore>,
    editor: &Entity<EntryEditor>,
    id: u32,
    window: &mut Window,
    cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    let state = entry_store.read(cx).state().clone();
    match state {
        EntryState::Loading => div().child(fetch_loader()).into_any_element(),
        EntryState::Failed(message) => {
            let store = entry_store.clone();
            div()
                .px_8()
                .child(error_card(
                    "entry-error",
                    FetchError::Request(&message),
                    move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                        store.update(cx, |store, cx| store.open(id, cx));
                    },
                ))
                .into_any_element()
        }
        EntryState::Ready {
            detail,
            entry,
            draft,
        } => {
            editor.update(cx, |editor, cx| editor.sync(id, &draft.notes, window, cx));
            let summary = &detail.summary;
            let title = summary
                .title
                .english
                .as_ref()
                .or(summary.title.romaji.as_ref())
                .or(summary.title.native.as_ref())
                .map_or_else(|| "Untitled".to_owned(), Clone::clone);
            let format = summary.format.clone().map(|format| match format {
                metadata::MediaFormat::Manga
                | metadata::MediaFormat::Novel
                | metadata::MediaFormat::OneShot => MediaFormat::Manga,
                _ => MediaFormat::Anime,
            });
            let card = MediaCard::new(
                MediaCardView {
                    poster: summary
                        .poster
                        .clone()
                        .map(|url| gpui_kit::Resource::Uri(url.into())),
                    title: title.clone().into(),
                    format,
                    score: draft.score.to_point_10_decimal(),
                    progress: draft.progress,
                    total: summary.episodes.or(summary.chapters),
                    detail: title.clone().into(),
                },
                format!("entry-card-{id}"),
            );
            let store = entry_store.clone();
            let editor_for_save = editor.clone();
            div()
                .flex()
                .flex_col()
                .gap_6()
                .p_8()
                .child(
                    div().flex().gap_6().child(card).child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(Label::new(title).text_xl())
                            .child(Label::new(format!(
                                "{} · {} episodes",
                                format.map_or("Anime", MediaFormat::label),
                                summary.episodes.unwrap_or(0),
                            )))
                            .child(Label::new(if entry.is_some() {
                                "On your Local list"
                            } else {
                                "Not on your Local list yet"
                            })),
                    ),
                )
                .child(editor_section(entry_store, editor, &draft, cx))
                .child(
                    Button::new("entry-save")
                        .label("Save to Local list")
                        .on_click(move |_, _, cx| {
                            store.update(cx, |store, cx| {
                                if let Some(draft) = store.draft_mut() {
                                    draft.notes = editor_for_save.read(cx).notes(cx);
                                }
                                cx.notify();
                            });
                            store.update(cx, EntryStore::save);
                        }),
                )
                .into_any_element()
        }
    }
}

/// The five editor fields, plus the entry's presence on the Local list.
fn editor_section(
    store: &Entity<EntryStore>,
    editor: &Entity<EntryEditor>,
    draft: &EntryDraft,
    cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    let mut status_row = div().flex().flex_wrap().gap_2();
    for status in STATUSES {
        status_row = status_row.child(field_button(
            format!("entry-status-{status:?}"),
            status_label(status),
            store,
            cx,
            move |draft| draft.status = status,
        ));
    }
    let progress_row = div()
        .flex()
        .items_center()
        .gap_2()
        .child(field_button(
            "entry-progress-down",
            "−",
            store,
            cx,
            |draft| draft.progress = draft.progress.saturating_sub(1),
        ))
        .child(Label::new(format!("Progress: {}", draft.progress)))
        .child(field_button("entry-progress-up", "+", store, cx, |draft| {
            draft.progress = draft.progress.saturating_add(1);
        }));
    let score_row = div()
        .flex()
        .items_center()
        .gap_2()
        .child(field_button("entry-score-down", "−", store, cx, |draft| {
            let raw = draft.score.raw().saturating_sub(10);
            draft.score = Score::new(raw).unwrap_or(Score::UNSCORED);
        }))
        .child(Label::new(format!(
            "Score: {}",
            draft
                .score
                .to_point_10_decimal()
                .map_or_else(|| "–".to_owned(), |score| format!("{score:.1}"),)
        )))
        .child(field_button("entry-score-up", "+", store, cx, |draft| {
            let raw = draft.score.raw().saturating_add(10).min(100);
            draft.score = Score::new(raw).unwrap_or(Score::UNSCORED);
        }));
    let dates_row = div()
        .flex()
        .flex_wrap()
        .gap_4()
        .child(date_field(
            "started",
            "Started",
            draft.started_at,
            store,
            cx,
        ))
        .child(date_field(
            "completed",
            "Completed",
            draft.completed_at,
            store,
            cx,
        ));
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .child(Label::new("Local list").text_lg())
        .child(status_row)
        .child(progress_row)
        .child(score_row)
        .child(dates_row)
        .child(Label::new("Notes"))
        .child(Input::new(&editor.read(cx).notes).aria_label("Notes"))
}

/// One date field: the current value, a Today shortcut and a Clear button.
fn date_field(
    id: &'static str,
    title: &'static str,
    date: FuzzyDate,
    store: &Entity<EntryStore>,
    cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(Label::new(title))
        .child(Label::new(format_fuzzy_date(date)).text_sm())
        .child(
            div()
                .flex()
                .gap_1()
                .child(field_button(
                    format!("entry-{id}-today"),
                    "Today",
                    store,
                    cx,
                    move |draft| {
                        let target = if id == "started" {
                            &mut draft.started_at
                        } else {
                            &mut draft.completed_at
                        };
                        *target = today();
                    },
                ))
                .child(field_button(
                    format!("entry-{id}-clear"),
                    "Clear",
                    store,
                    cx,
                    move |draft| {
                        let target = if id == "started" {
                            &mut draft.started_at
                        } else {
                            &mut draft.completed_at
                        };
                        *target = FuzzyDate::UNKNOWN;
                    },
                )),
        )
}

/// A button that applies one draft change to the store.
fn field_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    store: &Entity<EntryStore>,
    _cx: &mut Context<'_, AppShell>,
    change: impl Fn(&mut EntryDraft) + 'static,
) -> impl IntoElement {
    let store = store.clone();
    Button::new(id).label(label).on_click(move |_, _, cx| {
        store.update(cx, |store, cx| {
            if let Some(draft) = store.draft_mut() {
                change(draft);
            }
            cx.notify();
        });
    })
}

fn status_label(status: ListStatus) -> &'static str {
    match status {
        ListStatus::Current => "Watching",
        ListStatus::Planning => "Planning",
        ListStatus::Completed => "Completed",
        ListStatus::Dropped => "Dropped",
        ListStatus::Paused => "Paused",
        ListStatus::Repeating => "Repeating",
        _ => "Other",
    }
}

fn format_fuzzy_date(date: FuzzyDate) -> String {
    match (date.year(), date.month(), date.day()) {
        (Some(year), Some(month), Some(day)) => format!("{year:04}-{month:02}-{day:02}"),
        (Some(year), Some(month), None) => format!("{year:04}-{month:02}"),
        (Some(year), None, None) => format!("{year:04}"),
        _ => "–".to_owned(),
    }
}

/// Today's date, from the Unix clock. AniList accepts a partial date, but a
/// full one is what the Today shortcut means here.
fn today() -> FuzzyDate {
    let Ok(since_epoch) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return FuzzyDate::UNKNOWN;
    };
    let Ok(days) = i64::try_from(since_epoch.as_secs() / 86_400) else {
        return FuzzyDate::UNKNOWN;
    };
    // Days from 1970-01-01 to a civil date, after Howard Hinnant's algorithm.
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    let (Ok(year), Ok(month), Ok(day)) =
        (u16::try_from(year), u8::try_from(month), u8::try_from(day))
    else {
        return FuzzyDate::UNKNOWN;
    };
    FuzzyDate::new(Some(year), Some(month), Some(day)).unwrap_or(FuzzyDate::UNKNOWN)
}

#[cfg(test)]
mod tests {
    use super::{format_fuzzy_date, today};

    #[test]
    fn a_fuzzy_date_shows_only_its_known_parts() {
        let full = domain::FuzzyDate::new(Some(2024), Some(3), Some(9));
        let month = domain::FuzzyDate::new(Some(2024), Some(3), None);
        let year = domain::FuzzyDate::new(Some(2024), None, None);
        assert_eq!(format_fuzzy_date(full.expect("a valid date")), "2024-03-09");
        assert_eq!(format_fuzzy_date(month.expect("a valid date")), "2024-03");
        assert_eq!(format_fuzzy_date(year.expect("a valid date")), "2024");
        assert_eq!(format_fuzzy_date(domain::FuzzyDate::UNKNOWN), "–");
    }

    #[test]
    fn today_is_a_full_valid_date() {
        let date = today();
        assert!(date.year().is_some_and(|year| year >= 2026));
        assert!(date.month().is_some_and(|month| (1..=12).contains(&month)));
        assert!(date.day().is_some_and(|day| (1..=31).contains(&day)));
    }
}
