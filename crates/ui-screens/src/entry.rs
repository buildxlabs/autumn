//! The Anime Entry route (#58): the hero header, media card, Characters & VA,
//! Relations/Recommendations carousels and the Local list editor.

use std::rc::Rc;

use domain::{FuzzyDate, ListStatus, Score};
use gpui_kit::{
    App, AppContext, Context, ElementId, Entity, Hsla, IntoElement, ParentElement, SharedString,
    Styled, Subscription, Window,
    component::{
        ActiveTheme,
        button::Button,
        carousel::CarouselState,
        input::{Input, InputState},
        label::Label,
    },
    div,
};
use metadata::{Character, MediaSummary};
use ui_components::{
    CarouselWheel, LruImageCache, MediaCard, MediaCardView, MediaCarousel, MediaFormat,
    fetch_status::{FetchError, error_card, fetch_loader},
    hero::{HeroBannerData, hero_banner},
};
use ui_state::{
    Route,
    entry_store::{EntryDraft, EntryState, EntryStore},
};

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
    /// Paging state for the two carousels, owned here so it survives redraws.
    relations: Entity<CarouselState>,
    recommendations: Entity<CarouselState>,
    relations_wheel: CarouselWheel,
    recommendations_wheel: CarouselWheel,
    synced_for: Option<u32>,
}

impl EntryEditor {
    /// Creates the editor's notes input.
    pub(crate) fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
        let notes = cx.new(|cx| InputState::new(window, cx).placeholder("Notes"));
        let relations = cx.new(|_| CarouselState::new(0));
        let recommendations = cx.new(|_| CarouselState::new(0));
        cx.new(|_| Self {
            notes,
            relations,
            recommendations,
            relations_wheel: CarouselWheel::default(),
            recommendations_wheel: CarouselWheel::default(),
            synced_for: None,
        })
    }

    /// The value currently in the notes input.
    fn notes(&self, cx: &App) -> String {
        self.notes.read(cx).value().to_string()
    }

    /// Loads the draft's notes into the input and sizes the carousels once
    /// per opened Entry, so a render caused by another field edit never
    /// overwrites what the user typed or resets a carousel's page.
    fn sync(
        &mut self,
        id: u32,
        notes: &str,
        relations: usize,
        recommendations: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.synced_for == Some(id) {
            return;
        }
        self.synced_for = Some(id);
        self.notes.update(cx, |input, cx| {
            input.set_value(notes.to_owned(), window, cx);
        });
        self.relations
            .update(cx, |state, cx| state.set_item_count(relations, cx));
        self.recommendations
            .update(cx, |state, cx| state.set_item_count(recommendations, cx));
    }
}

/// Loads the editor's notes and the carousels' item counts whenever the store
/// finishes opening an Entry. `render` stays pure: the only entity writes are
/// here, in a notification callback that also has a `Window` for the input.
pub(crate) fn observe(
    store: &Entity<EntryStore>,
    editor: &Entity<EntryEditor>,
    window: &mut Window,
    cx: &mut Context<'_, AppShell>,
) -> Subscription {
    let editor = editor.clone();
    cx.observe_in(store, window, move |_, store, window, cx| {
        let ready = match store.read(cx).state() {
            EntryState::Ready { detail, draft, .. } => (
                detail.summary.anilist_id,
                draft.notes.clone(),
                detail.relations.len(),
                detail.recommendations.len(),
            ),
            EntryState::Loading => {
                // A reopened Entry loads the stored notes again, dropping
                // text typed but never saved.
                editor.update(cx, |editor, _| editor.synced_for = None);
                return;
            }
            EntryState::Failed(_) => return,
        };
        let (id, notes, relations, recommendations) = ready;
        editor.update(cx, |editor, cx| {
            editor.sync(id, &notes, relations, recommendations, window, cx);
        });
    })
}

/// Renders the Entry route from the store's current state.
pub(crate) fn render(
    entry_store: &Entity<EntryStore>,
    editor: &Entity<EntryEditor>,
    id: u32,
    cache: &Entity<LruImageCache>,
    cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    let state = entry_store.read(cx).state().clone();
    let save_error = entry_store.read(cx).save_error().map(str::to_owned);
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
        } => render_ready(
            entry_store,
            editor,
            id,
            cache,
            &detail,
            if entry.is_some() {
                "On your Local list"
            } else {
                "Not on your Local list yet"
            },
            &draft,
            save_error.as_deref(),
            cx,
        )
        .into_any_element(),
    }
}

/// The open Entry: hero, info box, poster, carousels and list editor.
#[expect(
    clippy::too_many_arguments,
    reason = "the route view hands the shell's collaborators to one renderer"
)]
fn render_ready(
    entry_store: &Entity<EntryStore>,
    editor: &Entity<EntryEditor>,
    id: u32,
    cache: &Entity<LruImageCache>,
    detail: &metadata::MediaDetail,
    list_label: &str,
    draft: &EntryDraft,
    save_error: Option<&str>,
    cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    let relations = detail
        .relations
        .iter()
        .map(|relation| &relation.media)
        .collect::<Vec<_>>();
    let recommendations = detail.recommendations.iter().collect::<Vec<_>>();
    let colors = cx.theme().colors;
    let summary = &detail.summary;
    let title = title_of(summary).to_owned();
    let store = entry_store.clone();
    let editor_for_save = editor.clone();
    let save = Button::new("entry-save")
        .label("Save to Local list")
        .on_click(move |_, _, cx| {
            store.update(cx, |store, cx| {
                if let Some(draft) = store.draft_mut() {
                    draft.notes = editor_for_save.read(cx).notes(cx);
                }
                cx.notify();
            });
            store.update(cx, EntryStore::save);
        });
    let hero = hero_banner(
        "entry-hero",
        HeroBannerData {
            title: title.clone().into(),
            synopsis: detail.description.clone().unwrap_or_default().into(),
            banner: summary.banner.clone(),
        },
        cache,
        colors.accent,
        colors.background,
        save,
    );
    let info = info_box(
        summary,
        list_label,
        &detail.studios,
        colors.background,
        colors.muted_foreground,
    );
    let card = MediaCard::new(
        MediaCardView {
            poster: summary
                .poster
                .clone()
                .map(|url| gpui_kit::Resource::Uri(url.into())),
            title: title.clone().into(),
            format: card_format(summary.format.clone()),
            score: draft.score.to_point_10_decimal(),
            progress: draft.progress,
            total: summary.episodes.or(summary.chapters),
            detail: title.into(),
        },
        format!("entry-card-{id}"),
    );
    let [relations_carousel, recommendations_carousel] =
        carousels(&relations, &recommendations, editor, cache, cx);
    div()
        .flex()
        .flex_col()
        .gap_6()
        .p_6()
        .child(hero)
        .child(info)
        .children((!detail.characters.is_empty()).then(|| characters_section(&detail.characters)))
        .child(div().flex().gap_6().child(card))
        .child(carousel_section("Relations", relations_carousel))
        .child(carousel_section(
            "Recommendations",
            recommendations_carousel,
        ))
        .child(editor_section(entry_store, editor, draft, cx))
        .children(save_error.map(|message| {
            let store = entry_store.clone();
            div().px_1().child(error_card(
                "entry-save-error",
                FetchError::Request(message),
                move |_, _, cx: &mut App| {
                    store.update(cx, EntryStore::save);
                },
            ))
        }))
        .into_any_element()
}

/// The Relations and Recommendations carousels. Clicking a card opens that
/// media's Entry, using the AniList id carried by its card view model.
fn carousels(
    relations: &[&MediaSummary],
    recommendations: &[&MediaSummary],
    editor: &Entity<EntryEditor>,
    cache: &Entity<LruImageCache>,
    cx: &mut Context<'_, AppShell>,
) -> [MediaCarousel; 2] {
    let relation_ids = relations.iter().map(|media| media.anilist_id).collect();
    let recommendation_ids = recommendations
        .iter()
        .map(|media| media.anilist_id)
        .collect();
    [
        MediaCarousel::new(
            "entry-relations",
            editor.read(cx).relations.clone(),
            card_views(relations),
            cache.clone(),
            editor.read(cx).relations_wheel.clone(),
        )
        .on_open(open_entry(relation_ids, cx)),
        MediaCarousel::new(
            "entry-recommendations",
            editor.read(cx).recommendations.clone(),
            card_views(recommendations),
            cache.clone(),
            editor.read(cx).recommendations_wheel.clone(),
        )
        .on_open(open_entry(recommendation_ids, cx)),
    ]
}

/// The default banner mode's info box: a fluid row of metadata chips.
fn info_box(
    summary: &MediaSummary,
    list_label: &str,
    studios: &[String],
    surface: Hsla,
    muted: Hsla,
) -> impl IntoElement {
    let mut chips = vec![summary.format.as_ref().map_or_else(
        || "Format unknown".to_owned(),
        |format| format!("{format:?}"),
    )];
    if let Some(episodes) = summary.episodes {
        chips.push(format!("{episodes} episodes"));
    }
    if let Some(season) = &summary.season {
        chips.push(format!(
            "{season:?} {}",
            summary
                .season_year
                .map_or_else(|| "?".to_owned(), |year| year.to_string())
        ));
    }
    chips.extend(studios.iter().cloned());
    chips.push(list_label.to_owned());
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .children(chips.into_iter().map(|chip| {
            div()
                .rounded_full()
                .bg(surface)
                .px_3()
                .py_1()
                .text_sm()
                .text_color(muted)
                .child(chip)
        }))
}

/// A titled carousel row.
fn carousel_section(title: &'static str, carousel: MediaCarousel) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            Label::new(title)
                .id(format!("entry-{title}-label"))
                .text_lg(),
        )
        .child(carousel)
}

/// Opens the Entry of the media at a carousel card's index.
fn open_entry(
    ids: Vec<u32>,
    cx: &mut Context<'_, AppShell>,
) -> impl Fn(usize, &mut Window, &mut App) + 'static {
    let shell = cx.entity().downgrade();
    move |index, _, cx| {
        if let Some(id) = ids.get(index).copied() {
            let _ = shell.update(cx, |shell, cx| {
                shell.navigate(Route::Entry { id }, cx);
            });
        }
    }
}

/// The main cast and their Japanese voice actors, as listed by AniList.
fn characters_section(characters: &[Character]) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            Label::new("Characters & VA")
                .id("entry-characters-label")
                .text_lg(),
        )
        .children(characters.iter().map(|character| {
            let name = character
                .name
                .clone()
                .unwrap_or_else(|| "Unknown character".to_owned());
            let actors = character
                .voice_actors
                .iter()
                .filter_map(|actor| actor.name.as_deref())
                .collect::<Vec<_>>()
                .join(", ");
            let line = if actors.is_empty() {
                name
            } else {
                format!("{name} — {actors}")
            };
            Label::new(line)
                .id(format!("entry-character-{}", character.anilist_id))
                .text_sm()
        }))
}

/// Cards for the related and recommended media.
fn card_views(items: &[&MediaSummary]) -> Rc<[MediaCardView]> {
    items
        .iter()
        .map(|item| MediaCardView {
            poster: item
                .poster
                .clone()
                .map(|url| gpui_kit::Resource::Uri(url.into())),
            title: title_of(item).into(),
            format: card_format(item.format.clone()),
            score: item.mean_score.map(|score| f32::from(score) / 10.0),
            progress: 0,
            total: item.episodes.or(item.chapters),
            detail: format!(
                "{} · {}",
                title_of(item),
                item.format.as_ref().map_or_else(
                    || "Unknown format".to_owned(),
                    |format| format!("{format:?}"),
                )
            )
            .into(),
        })
        .collect()
}

fn title_of(summary: &MediaSummary) -> &str {
    summary
        .title
        .english
        .as_deref()
        .or(summary.title.romaji.as_deref())
        .or(summary.title.native.as_deref())
        .unwrap_or("Untitled")
}

fn card_format(format: Option<metadata::MediaFormat>) -> Option<MediaFormat> {
    format.map(|format| match format {
        metadata::MediaFormat::Manga
        | metadata::MediaFormat::Novel
        | metadata::MediaFormat::OneShot => MediaFormat::Manga,
        _ => MediaFormat::Anime,
    })
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
        status_row = status_row.child(status_button(status, store, editor, cx));
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
        .child(Label::new(title).id(format!("entry-{id}-title")))
        .child(
            Label::new(format_fuzzy_date(date))
                .id(format!("entry-{id}-value"))
                .text_sm(),
        )
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

/// A status choice is a user fact: save it at once, with whatever the notes
/// field currently holds, like My Lists' quick editor.
fn status_button(
    status: ListStatus,
    store: &Entity<EntryStore>,
    editor: &Entity<EntryEditor>,
    _cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    let store = store.clone();
    let editor = editor.clone();
    Button::new(format!("entry-status-{status:?}"))
        .label(status_label(status))
        .on_click(move |_, _, cx| {
            store.update(cx, |store, cx| {
                if let Some(draft) = store.draft_mut() {
                    draft.status = status;
                    draft.notes = editor.read(cx).notes(cx);
                }
                cx.notify();
            });
            store.update(cx, EntryStore::save);
        })
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

/// Today's date in the user's local time. AniList accepts a partial date, but
/// a full one is what the Today shortcut means here.
fn today() -> FuzzyDate {
    // The local UTC offset can be indeterminate; UTC is still a full date.
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let Ok(year) = u16::try_from(now.year()) else {
        return FuzzyDate::UNKNOWN;
    };
    FuzzyDate::new(Some(year), Some(u8::from(now.month())), Some(now.day()))
        .unwrap_or(FuzzyDate::UNKNOWN)
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::clone_on_ref_ptr,
        reason = "the screen test keeps a concrete handle to the canned fake"
    )]
    #![expect(
        clippy::indexing_slicing,
        reason = "the screen test checks the one recorded save"
    )]

    use std::{rc::Rc, sync::Arc, time::Duration};

    use domain::{FuzzyDate, ListStatus, MediaKind};
    use gpui_kit::{
        App, AppContext, Bounds, Task, TestAppContext, WindowBounds, WindowOptions, base::Root,
        point, px, size, test::TestWindowExt,
    };
    use metadata::{MediaDetail, MediaSummary, MediaTitle, Relation};
    use tokio::runtime::Builder;
    use ui_state::{
        Route,
        bridge::{self, BridgeError},
        entry_store::EntryStore,
        home_store::HomeStore,
        test_support::CannedServices,
    };

    use crate::{
        discover::DiscoverUpdate,
        shell::{AppShell, RailPrefs},
    };

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

    fn summary(id: u32, title: &str) -> MediaSummary {
        MediaSummary {
            anilist_id: id,
            mal_id: None,
            kind: Some(MediaKind::Anime),
            title: MediaTitle {
                romaji: Some(title.to_owned()),
                ..MediaTitle::default()
            },
            poster: None,
            banner: None,
            format: None,
            status: None,
            season: None,
            season_year: None,
            episodes: Some(12),
            chapters: None,
            mean_score: None,
            genres: Vec::new(),
            is_adult: false,
        }
    }

    fn detail() -> MediaDetail {
        MediaDetail {
            summary: summary(100, "An Entry"),
            description: Some("A synopsis.".to_owned()),
            start_date: FuzzyDate::UNKNOWN,
            end_date: FuzzyDate::UNKNOWN,
            duration: None,
            studios: Vec::new(),
            popularity: None,
            characters: Vec::new(),
            relations: vec![Relation {
                relation: None,
                media: summary(200, "A Relation"),
            }],
            recommendations: vec![summary(300, "A Recommendation")],
        }
    }

    #[gpui_kit::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one screen test covers rendering, saving and carousel navigation"
    )]
    async fn the_entry_screen_renders_saves_and_opens_a_related_entry(cx: &mut TestAppContext) {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a test runtime");
        let _guard = runtime.enter();
        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));

        let services = Arc::new(CannedServices::new(detail()));
        let fake = Arc::clone(&services);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::init(cx);
                ui_theme::init(cx);
                cx.set_reduce_motion(true);
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(1280.), px(2200.)),
                    })),
                    ..WindowOptions::default()
                };
                gpui_kit::open_window(options, cx, |window, cx| {
                    let entry_store = cx.new(|_| EntryStore::new(services.clone()));
                    let home_store = cx.new(|_| HomeStore::new(services.clone()));
                    cx.new(|cx| {
                        AppShell::new(
                            RailPrefs::default(),
                            settings::PlaybackSettings::default(),
                            crate::shell::PlaybackServices::inert(),
                            settings::LibrarySettings::default(),
                            crate::shell::LibraryServices::inert(),
                            cx.new(|_| {
                                ui_state::LibraryStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            cx.new(|_| ui_state::fetch_status::FetchStatus::default()),
                            cx.new(|_| ui_state::account::AccountStore::default()),
                            "https://example.test/login".to_owned(),
                            |_, _| Task::ready(()),
                            |_, _| Task::ready(Ok(0)),
                            |_, _| Task::ready(()),
                            Rc::new(|_: Option<String>, _: &App| {
                                Task::ready(Ok::<_, BridgeError>(DiscoverUpdate::default()))
                            }),
                            entry_store,
                            home_store,
                            cx.new(|_| {
                                ui_state::PlaybackStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            window,
                            cx,
                        )
                    })
                })
            })
            .expect("a test window");
        let window = window.downcast::<Root>().expect("the root window");

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            view.update(cx, |shell, cx| shell.navigate(Route::Entry { id: 100 }, cx));
        })
        .expect("navigation");
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let hero = window.find("entry-hero-banner").bounds();
            assert!(
                hero.size.width > px(0.) && hero.size.height > px(0.),
                "the Entry hero has real bounds"
            );
            window.click("entry-status-Current", cx);
            window.render_frame(cx);
        })
        .expect("the rendered Entry");
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();
        let saves = fake.saved();
        assert_eq!(saves.len(), 1, "clicking a status button saved the draft");
        assert_eq!(saves[0].status, ListStatus::Current);

        let relation_carousel = cx
            .update(|cx| {
                view.update(cx, |shell, cx| {
                    let store = shell.entry_store().clone();
                    let editor = shell.entry_editor().clone();
                    let cache = shell.banner_cache().clone();
                    let (relations, recommendations) = match store.read(cx).state() {
                        super::EntryState::Ready { detail, .. } => (
                            detail
                                .relations
                                .iter()
                                .map(|relation| relation.media.clone())
                                .collect::<Vec<_>>(),
                            detail.recommendations.clone(),
                        ),
                        _ => return None,
                    };
                    let relation_refs = relations.iter().collect::<Vec<_>>();
                    let recommendation_refs = recommendations.iter().collect::<Vec<_>>();
                    let [relations, _] =
                        super::carousels(&relation_refs, &recommendation_refs, &editor, &cache, cx);
                    Some(relations)
                })
            })
            .expect("the Relations carousel");
        cx.update_window(window.into(), |_, window, cx| {
            relation_carousel.fire_open(0, window, cx);
            window.render_frame(cx);
        })
        .expect("the carousel Open action");
        assert_eq!(
            view.read_with(cx, |shell, _| shell.route()),
            Route::Entry { id: 200 },
            "Open on a relation card navigated to its Entry"
        );
    }
}
