//! My Lists screen: status and custom-list tabs, a searchable poster grid and
//! a quick editor opened from a card's context menu (ticket #60).

use db::Db;
use domain::{ListStatus, MediaId, Score};
use gpui_kit::{
    App, AppContext, ClickEvent, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, Resource, SharedString, Styled, Subscription, Task, TestSupportExt,
    Window,
    component::{
        ActiveTheme, Selectable, WindowExt,
        button::Button,
        input::{Input, InputEvent, InputState},
        label::Label,
    },
    div,
};
use metadata::CatalogCache;
use ui_components::{MediaCardView, PosterGrid};
use ui_state::{ListCard, ListSort, ListTab, ListsSession, QuickEdit, bridge};

use crate::lists_source::LocalListSource;

const STATUSES: [ListStatus; 6] = [
    ListStatus::Current,
    ListStatus::Repeating,
    ListStatus::Planning,
    ListStatus::Completed,
    ListStatus::Paused,
    ListStatus::Dropped,
];
const SORTS: [ListSort; 4] = [
    ListSort::Recent,
    ListSort::Title,
    ListSort::Progress,
    ListSort::Score,
];

/// What the screen asks its host to do.
#[derive(Debug, Clone)]
pub(crate) enum ListsEvent {
    /// The user picked a tab; the shell owns navigation.
    SelectTab(ListTab),
}

/// The state behind one list entry being edited.
#[derive(Debug, Clone, Copy)]
struct Editing {
    media: MediaId,
    status: ListStatus,
}

/// The My Lists view.
pub(crate) struct ListsScreen {
    session: ListsSession<LocalListSource>,
    grid: Entity<PosterGrid>,
    search: Entity<InputState>,
    progress: Entity<InputState>,
    score: Entity<InputState>,
    _search_subscription: Subscription,
    editing: Option<Editing>,
    visible_media: Vec<MediaId>,
    error: Option<String>,
    task: Option<Task<()>>,
}

impl std::fmt::Debug for ListsScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListsScreen")
            .field("tab", self.session.tab())
            .field("cards", &self.session.cards().len())
            .field("editing", &self.editing)
            .finish_non_exhaustive()
    }
}

impl EventEmitter<ListsEvent> for ListsScreen {}

fn status_label(status: ListStatus) -> &'static str {
    match status {
        ListStatus::Current => "Watching",
        ListStatus::Repeating => "Repeating",
        ListStatus::Planning => "Planning",
        ListStatus::Completed => "Completed",
        ListStatus::Paused => "Paused",
        ListStatus::Dropped => "Dropped",
        _ => "Listed",
    }
}

fn sort_label(sort: ListSort) -> &'static str {
    match sort {
        ListSort::Recent => "Recent",
        ListSort::Title => "Title",
        ListSort::Progress => "Progress",
        ListSort::Score => "Score",
        _ => "Custom",
    }
}

fn card_view(card: &ListCard) -> MediaCardView {
    MediaCardView {
        poster: card
            .poster
            .as_ref()
            .map(|url| Resource::Uri(url.clone().into())),
        title: card.title.clone().into(),
        format: None,
        score: card.entry.score.to_point_10_decimal(),
        progress: card.entry.progress,
        total: None,
        detail: format!("{} · {}", status_label(card.entry.status), card.title).into(),
    }
}

impl ListsScreen {
    /// Creates the screen over the active platform's Local source.
    pub(crate) fn new(
        db: Db,
        catalog: CatalogCache,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let screen = cx.new(|cx| {
            let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search your lists"));
            let progress = cx.new(|cx| InputState::new(window, cx).placeholder("Progress"));
            let score = cx.new(|cx| InputState::new(window, cx).placeholder("Score"));
            let search_entity = search.clone();
            let subscription = cx.subscribe(
                &search,
                move |screen: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        let text = search_entity.read(cx).value().to_string();
                        screen.session.set_query(text);
                        screen.sync_grid(cx);
                        cx.notify();
                    }
                },
            );
            Self {
                session: ListsSession::new(LocalListSource::new(db, catalog)),
                grid: PosterGrid::new(Vec::new(), cx),
                search,
                progress,
                score,
                _search_subscription: subscription,
                editing: None,
                visible_media: Vec::new(),
                error: None,
                task: None,
            }
        });
        let weak = screen.downgrade();
        screen.update(cx, |screen, cx| {
            screen.grid.update(cx, |grid, cx| {
                grid.on_add_to_list(move |index, window, cx| {
                    let _ = weak.update(cx, |screen, cx| screen.open_editor(index, window, cx));
                });
                cx.notify();
            });
            screen.reload(cx);
        });
        screen
    }

    /// Selects the tab named by the route.
    pub(crate) fn set_tab(&mut self, tab: ListTab, cx: &mut Context<'_, Self>) {
        if self.session.tab() != &tab {
            self.session.set_tab(tab);
            self.sync_grid(cx);
            cx.notify();
        }
    }

    fn sync_grid(&mut self, cx: &mut Context<'_, Self>) {
        let cards = self.session.visible();
        self.visible_media = cards.iter().map(|card| card.entry.media).collect();
        let views = cards.iter().map(|card| card_view(card)).collect();
        self.grid.update(cx, |grid, cx| grid.set_items(views, cx));
    }

    /// Swaps in freshly loaded entries without undoing a tab, query or sort
    /// the user changed while the request was in flight.
    fn adopt_session(
        &mut self,
        mut loaded: ListsSession<LocalListSource>,
        cx: &mut Context<'_, Self>,
    ) {
        let tab = self.session.tab().clone();
        let query = self.session.query().to_owned();
        let sort = self.session.sort();
        loaded.set_tab(tab);
        loaded.set_query(query);
        loaded.set_sort(sort);
        self.session = loaded;
        self.error = None;
        self.sync_grid(cx);
    }

    fn reload(&mut self, cx: &mut Context<'_, Self>) {
        let mut session = self.session.clone();
        self.task = Some(cx.spawn(async move |screen, cx| {
            let task = cx.update(|cx| {
                bridge::run(cx, async move { session.reload().await.map(|()| session) })
            });
            let result = task.await;
            let _ = screen.update(cx, |screen, cx| {
                match result {
                    Ok(Ok(session)) => screen.adopt_session(session, cx),
                    Ok(Err(error)) => screen.error = Some(error.to_string()),
                    Err(error) => screen.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
    }

    fn open_editor(&mut self, index: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(media) = self.visible_media.get(index).copied() else {
            return;
        };
        let Some(card) = self
            .session
            .cards()
            .iter()
            .find(|card| card.entry.media == media)
            .cloned()
        else {
            return;
        };
        let status = card.entry.status;
        self.editing = Some(Editing { media, status });
        let screen = cx.entity();
        let progress = self.progress.clone();
        let score = self.score.clone();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let screen = screen.clone();
            let progress = progress.clone();
            let score = score.clone();
            let shown = screen
                .read(cx)
                .editing
                .map_or(status, |editing| editing.status);
            dialog
                .title("Quick edit")
                .child(Label::new("Status"))
                .child(
                    Button::new("edit-status")
                        .label(status_label(shown))
                        .on_click({
                            let screen = screen.clone();
                            move |_: &ClickEvent, _, cx| {
                                screen.update(cx, |screen, cx| {
                                    let next = screen
                                        .editing
                                        .as_ref()
                                        .and_then(|editing| {
                                            STATUSES
                                                .iter()
                                                .position(|&status| status == editing.status)
                                        })
                                        .map_or(0, |at| (at + 1) % STATUSES.len());
                                    if let (Some(editing), Some(status)) =
                                        (screen.editing.as_mut(), STATUSES.get(next))
                                    {
                                        editing.status = *status;
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(Label::new("Progress"))
                .child(Input::new(&progress).aria_label("Progress"))
                .child(Label::new("Score (0-100)"))
                .child(Input::new(&score).aria_label("Score"))
                .child(Button::new("edit-save").label("Save").on_click({
                    move |_: &ClickEvent, window, cx| {
                        screen.update(cx, |screen, cx| screen.save_edit(window, cx));
                    }
                }))
                .child(
                    Button::new("edit-cancel")
                        .label("Cancel")
                        .on_click(|_: &ClickEvent, window, cx| window.close_dialog(cx)),
                )
        });
        self.progress.update(cx, |input, cx| {
            input.set_value(card.entry.progress.to_string(), window, cx);
        });
        self.score.update(cx, |input, cx| {
            input.set_value(card.entry.score.raw().to_string(), window, cx);
        });
    }

    fn save_edit(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        let progress = self.progress.read(cx).value().parse::<u32>().ok();
        let score = self
            .score
            .read(cx)
            .value()
            .parse::<u8>()
            .ok()
            .and_then(|raw| Score::new(raw).ok());
        let edit = QuickEdit::new().status(editing.status);
        let edit = progress.map_or(edit, |progress| edit.progress(progress));
        let edit = score.map_or(edit, |score| edit.score(score));
        let mut session = self.session.clone();
        self.task = Some(cx.spawn(async move |screen, cx| {
            let task = cx.update(|cx| {
                bridge::run(cx, async move {
                    session.quick_edit(editing.media, &edit).await?;
                    Ok::<_, ui_state::ListSourceError>(session)
                })
            });
            let result = task.await;
            let _ = screen.update(cx, |screen, cx| {
                match result {
                    Ok(Ok(session)) => screen.adopt_session(session, cx),
                    Ok(Err(error)) => screen.error = Some(error.to_string()),
                    Err(error) => screen.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        window.close_dialog(cx);
    }
}

impl Render for ListsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let tabs = self.session.tabs();
        let selected = tabs
            .iter()
            .position(|tab| tab == self.session.tab())
            .unwrap_or(0);
        let mut tab_bar = div()
            .id("list-tabs")
            .flex()
            .flex_wrap()
            .gap_2()
            .test_support();
        for (index, tab) in tabs.iter().enumerate() {
            let label: SharedString = match tab {
                ListTab::Status(status) => status_label(*status).into(),
                ListTab::Custom(name) => name.clone().into(),
                _ => "List".into(),
            };
            let tab = tab.clone();
            tab_bar = tab_bar.child(
                Button::new(format!("list-tab-{index}"))
                    .label(label)
                    .selected(selected == index)
                    .on_click(cx.listener(move |screen: &mut Self, _, _, cx| {
                        screen.session.set_tab(tab.clone());
                        screen.sync_grid(cx);
                        cx.emit(ListsEvent::SelectTab(tab.clone()));
                        cx.notify();
                    })),
            );
        }

        let sort = self.session.sort();
        let next = SORTS
            .iter()
            .position(|&s| s == sort)
            .map_or(0, |at| (at + 1) % SORTS.len());
        let sort_button = Button::new("list-sort")
            .label(format!("Sort: {}", sort_label(sort)))
            .on_click(cx.listener(move |screen: &mut Self, _, _, cx| {
                if let Some(sort) = SORTS.get(next).copied() {
                    screen.session.set_sort(sort);
                }
                screen.sync_grid(cx);
                cx.notify();
            }));

        div()
            .id("lists-screen")
            .size_full()
            .flex()
            .flex_col()
            .child(div().px_8().pt_8().text_3xl().child("My Lists"))
            .child(div().px_8().pt_4().child(tab_bar))
            .child(
                div()
                    .px_8()
                    .pt_4()
                    .flex()
                    .gap_3()
                    .items_center()
                    .child(Input::new(&self.search).aria_label("Search your lists"))
                    .child(sort_button),
            )
            .children(self.error.as_ref().map(|error| {
                div()
                    .px_8()
                    .pt_2()
                    .text_color(colors.danger)
                    .child(error.clone())
            }))
            .child(div().flex_1().min_h_0().child(self.grid.clone()))
            .test_support()
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use domain::{AdvancedScores, ExternalId, FuzzyDate, ListEntry, ListStatus, MediaKind, Score};
    use gpui_kit::{
        AppContext, Bounds, TestAppContext, WindowBounds, WindowOptions, base::Root, point, px,
        size, test::TestWindowExt,
    };

    use super::ListsScreen;

    #[gpui_kit::test]
    fn the_lists_screen_shows_tabs_and_a_grid(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("temp dir");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()
            .expect("runtime");
        let db = runtime
            .block_on(db::Db::open(&dir.path().join("autumn.db")))
            .expect("db");
        let media = runtime
            .block_on(
                db.media()
                    .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(1)]),
            )
            .expect("mint");
        let entry = ListEntry {
            media: media.media,
            status: ListStatus::Current,
            score: Score::UNSCORED,
            progress: 3,
            progress_volumes: 0,
            repeat: 0,
            started_at: FuzzyDate::UNKNOWN,
            completed_at: FuzzyDate::UNKNOWN,
            notes: String::new(),
            private: false,
            priority: 0,
            advanced_scores: AdvancedScores::default(),
            custom_lists: Vec::new(),
            hidden_from_status_lists: false,
            created_at: SystemTime::UNIX_EPOCH,
            updated_at: SystemTime::UNIX_EPOCH,
        };
        runtime
            .block_on(db.local_list().save(&entry))
            .expect("save entry");
        let catalog = metadata::CatalogCache::new(
            metadata::AniList::new(net::Client::new().expect("http client")),
            net::PosterCache::new(dir.path()).expect("poster cache"),
        );

        cx.executor().allow_parking();
        let (window, screen) = cx.update(|cx| {
            gpui_kit::init(cx);
            ui_theme::init(cx);
            ui_state::bridge::init(cx, runtime.handle().clone());
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(1000.), px(700.)),
                })),
                ..WindowOptions::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                ListsScreen::new(db, catalog, window, cx)
            })
            .expect("open window")
        });
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            cx.run_until_parked();
        }

        let window = window.downcast::<Root>().expect("root window");
        let mut shows = |id: &'static str| {
            cx.update_window(window.into(), |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).is_some()
            })
            .expect("window is open")
        };
        assert!(shows("lists-screen"), "the screen renders");
        assert!(shows("list-tabs"), "tabs render");
        assert!(shows("poster-grid"), "the grid has height");
        let (cards, error) = cx.update(|cx| {
            let screen = screen.read(cx);
            (screen.session.cards().len(), screen.error.clone())
        });
        assert_eq!(cards, 1, "the local entry loaded: {error:?}");
    }
}
