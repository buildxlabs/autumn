//! The manual **release list** sheet (#221, #182 Q5–Q7, Q9).
//!
//! A right-side sheet for one episode: header (episode + title, availability
//! pill, auto-select failure reason), editable query,
//! one-shot **Profile ▾**, **Refresh**, client-side chips, a sortable table
//! (rank by default, ★ Best, failing rows dimmed), and inline batch file
//! pick. All logic lives in `ui_state::release_list`; this view only renders.

use gpui_kit::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Window,
    component::{
        ActiveTheme, Disableable as _,
        button::{Button, ButtonVariants as _},
        input::{Input, InputEvent, InputState},
        menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use ui_state::Availability;
use ui_state::release_list::{
    BatchFile, BatchState, KindFilter, ReleaseFilters, ReleaseListStore, ReleaseRow, ReleaseSort,
    SortColumn, SortDirection,
};

use crate::entry_torrent::availability_pill;

/// The release-list sheet body.
pub(crate) struct ReleaseListScreen {
    store: Entity<ReleaseListStore>,
    input: Entity<InputState>,
    _input_events: Subscription,
}

impl std::fmt::Debug for ReleaseListScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReleaseListScreen").finish_non_exhaustive()
    }
}

impl ReleaseListScreen {
    /// Builds the screen over `store`, seeding the query input.
    pub(crate) fn new(
        store: Entity<ReleaseListStore>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let query = store.read(cx).query().to_owned();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search releases…"));
        input.update(cx, |input, cx| input.set_value(query, window, cx));
        let input_events = cx.subscribe(&input, {
            let store = store.clone();
            move |_screen, input, event, cx| {
                let value = input.read(cx).value().to_string();
                match event {
                    InputEvent::Change => {
                        store.update(cx, |store, cx| store.set_query(value, cx));
                    }
                    InputEvent::PressEnter { .. } => {
                        store.update(cx, |store, cx| {
                            store.set_query(value, cx);
                            store.search(false, cx);
                        });
                    }
                    _ => {}
                }
            }
        });
        Self {
            store,
            input,
            _input_events: input_events,
        }
    }
}

impl Render for ReleaseListScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let muted = cx.theme().colors.muted_foreground;
        let amber = gpui_kit::hsla(0.11, 0.9, 0.55, 1.0);

        let context = store.context();
        let header_title = context.map_or_else(
            || "Release list".to_owned(),
            |c| match &c.episode_title {
                Some(title) if !title.is_empty() => format!("Episode {} · {}", c.episode, title),
                _ => format!("Episode {}", c.episode),
            },
        );
        let availability = context.map_or(Availability::None, |c| c.availability);
        let failure = context.and_then(|c| c.failure_reason.clone());
        let profiles = context.map_or_else(Vec::new, |c| c.profiles.clone());
        let saved_profile = context.map(|c| c.profile.name.clone());
        let active_profile_name = store.active_profile().name;

        let filters = store.filters().clone();
        let sort = store.sort();
        let resolutions = store.available_resolutions();
        let indexers = store.available_indexers();
        let searching = store.searching();
        let error = store.error().map(str::to_owned);
        let notices = store.notices().to_vec();
        let query_has_text = !store.query().trim().is_empty();

        let visible = store.visible_rows();
        let best = store.best_index();
        let expanded = store.expanded().map(str::to_owned);

        // Snapshot per-row view data so no `cx` borrow crosses listeners.
        let rows: Vec<RowView> = visible
            .iter()
            .enumerate()
            .map(|(index, row)| RowView {
                row: (*row).clone(),
                best: best == Some(index),
                expanded: expanded.as_deref() == Some(row.infohash.as_str()),
                amber: store.amber(&row.infohash).map(str::to_owned),
                batch: match store.batch_state(&row.infohash) {
                    BatchState::Loading => BatchView::Loading,
                    BatchState::Loaded(files) => BatchView::Loaded(files.clone()),
                    _ => BatchView::Failed,
                },
            })
            .collect();

        let store = self.store.clone();
        let _ = saved_profile;

        let header = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_base().child(header_title))
            .child(
                div().flex().items_center().gap_2().children(
                    availability
                        .label()
                        .map(|label| availability_pill(label, availability, cx)),
                ),
            )
            .children(failure.map(|reason| {
                div()
                    .text_sm()
                    .text_color(amber)
                    .child(format!("auto-select failed: {reason}"))
            }));

        let query_row = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().flex_1().child(Input::new(&self.input)))
            .child({
                let store = store.clone();
                Button::new("release-list-refresh")
                    .label("Refresh")
                    .disabled(searching || !query_has_text)
                    .on_click(move |_, _, cx| store.update(cx, |store, cx| store.search(true, cx)))
            })
            .child(profile_menu(store.clone(), profiles, active_profile_name));

        div()
            .id("release-list")
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .child(header)
            .child(query_row)
            .child(chip_row(store.clone(), &filters, &resolutions, &indexers))
            .children(searching.then(|| div().text_sm().text_color(muted).child("Searching…")))
            .children(error.map(|error| div().text_sm().text_color(amber).child(error)))
            .children(notices.into_iter().map(|notice| {
                div()
                    .text_sm()
                    .text_color(amber)
                    .child(format!("{}: {}", notice.indexer, notice.reason))
            }))
            .child(table(&store, &rows, sort, muted, amber))
    }
}

struct RowView {
    row: ReleaseRow,
    best: bool,
    expanded: bool,
    amber: Option<String>,
    batch: BatchView,
}

enum BatchView {
    Loading,
    Failed,
    Loaded(Vec<BatchFile>),
}

fn profile_menu(
    store: Entity<ReleaseListStore>,
    profiles: Vec<settings::AutoSelectProfile>,
    active_name: String,
) -> impl IntoElement {
    let label = format!("Profile ▾ {active_name}");
    Button::new("release-list-profile")
        .label(label)
        .dropdown_menu(move |menu: PopupMenu, _, _| {
            let mut menu = menu;
            for profile in &profiles {
                let name = profile.name.clone();
                let checked = name == active_name;
                let profile = profile.clone();
                let store = store.clone();
                menu = menu.item(PopupMenuItem::new(name).checked(checked).on_click(
                    move |_, _, cx| {
                        let profile = profile.clone();
                        store.update(cx, |store, cx| {
                            store.set_profile_override(Some(profile), cx);
                        });
                    },
                ));
            }
            menu.item(PopupMenuItem::new("One-shot: re-rank, not saved".to_owned()).disabled(true))
        })
}

fn chip_row(
    store: Entity<ReleaseListStore>,
    filters: &ReleaseFilters,
    resolutions: &[u32],
    indexers: &[String],
) -> impl IntoElement {
    let mut row = div().flex().flex_wrap().items_center().gap_1();

    for (kind, label) in [
        (KindFilter::All, "All"),
        (KindFilter::Single, "Single"),
        (KindFilter::Batch, "Batch"),
    ] {
        let store = store.clone();
        let active = filters.kind == kind;
        row = row.child(
            Button::new(format!("release-chip-kind-{label}"))
                .label(label)
                .when(active, Button::primary)
                .on_click(move |_, _, cx| {
                    store.update(cx, |store, cx| store.update_filters(|f| f.kind = kind, cx));
                }),
        );
    }

    for resolution in resolutions {
        let store = store.clone();
        let value = *resolution;
        let active = filters.resolutions.contains(&value);
        row = row.child(
            Button::new(format!("release-chip-res-{value}"))
                .label(format!("{value}p"))
                .when(active, Button::primary)
                .on_click(move |_, _, cx| {
                    store.update(cx, |store, cx| {
                        store.update_filters(
                            |f| {
                                if !f.resolutions.remove(&value) {
                                    f.resolutions.insert(value);
                                }
                            },
                            cx,
                        );
                    });
                }),
        );
    }

    for indexer in indexers {
        let store = store.clone();
        let label = indexer.clone();
        let active = filters.indexers.contains(&label);
        row = row.child(
            Button::new(format!("release-chip-indexer-{label}"))
                .label(label.clone())
                .when(active, Button::primary)
                .on_click(move |_, _, cx| {
                    let label = label.clone();
                    store.update(cx, |store, cx| {
                        store.update_filters(
                            |f| {
                                if !f.indexers.remove(&label) {
                                    f.indexers.insert(label.clone());
                                }
                            },
                            cx,
                        );
                    });
                }),
        );
    }

    let active = filters.show_filtered_out;
    row = row.child(
        Button::new("release-chip-filtered")
            .label("Show filtered-out")
            .when(active, Button::primary)
            .on_click(move |_, _, cx| {
                store.update(cx, |store, cx| {
                    store.update_filters(|f| f.show_filtered_out = !f.show_filtered_out, cx);
                });
            }),
    );

    row
}

const COLUMNS: [(SortColumn, &str); 8] = [
    (SortColumn::Resolution, "Res"),
    (SortColumn::Codec, "Codec"),
    (SortColumn::Group, "Group"),
    (SortColumn::Seeders, "Seeders"),
    (SortColumn::Size, "Size"),
    (SortColumn::Date, "Date"),
    (SortColumn::Batch, "Batch"),
    (SortColumn::Status, "Status"),
];

fn table(
    store: &Entity<ReleaseListStore>,
    rows: &[RowView],
    sort: ReleaseSort,
    muted: gpui_kit::Hsla,
    amber: gpui_kit::Hsla,
) -> impl IntoElement {
    let mut header = div()
        .flex()
        .items_center()
        .gap_2()
        .p_1()
        .text_sm()
        .text_color(muted);
    header = header.child({
        let store = store.clone();
        div().flex_1().child(
            Button::new("release-sort-release")
                .label(sort_label("Release", sort, SortColumn::Release))
                .on_click(move |_, _, cx| {
                    store.update(cx, |store, cx| store.click_sort(SortColumn::Release, cx));
                }),
        )
    });
    for (column, label) in COLUMNS {
        let store = store.clone();
        header = header.child(
            Button::new(format!("release-sort-{label}"))
                .label(sort_label(label, sort, column))
                .w(px(72.))
                .on_click(move |_, _, cx| {
                    store.update(cx, |store, cx| store.click_sort(column, cx));
                }),
        );
    }
    header = header.child({
        let store = store.clone();
        Button::new("release-sort-rank")
            .label(if sort.column == SortColumn::Rank {
                "Rank ✓"
            } else {
                "Rank"
            })
            .on_click(move |_, _, cx| {
                store.update(cx, |store, cx| store.click_sort(SortColumn::Rank, cx));
            })
    });

    let mut body = div().flex().flex_col().gap_1();
    for view in rows {
        body = body.child(row_element(store, view, muted, amber));
    }
    if rows.is_empty() {
        body = body.child(div().text_sm().text_color(muted).p_2().child("No results."));
    }

    div().flex().flex_col().gap_1().child(header).child(body)
}

fn sort_label(label: &str, sort: ReleaseSort, column: SortColumn) -> String {
    if sort.column == column {
        let arrow = match sort.direction {
            SortDirection::Ascending => " ▲",
            SortDirection::Descending => " ▼",
            _ => "",
        };
        format!("{label}{arrow}")
    } else {
        label.to_owned()
    }
}

fn row_element(
    store: &Entity<ReleaseListStore>,
    view: &RowView,
    muted: gpui_kit::Hsla,
    amber: gpui_kit::Hsla,
) -> impl IntoElement {
    let row = &view.row;
    let infohash = row.infohash.clone();

    let status = if row.no_peers {
        ui_state::release_list::NO_PEERS.to_owned()
    } else {
        row.status.clone().unwrap_or_default()
    };

    let release_cell = div()
        .flex_1()
        .flex()
        .items_center()
        .gap_1()
        .child(if view.best {
            "★ ".to_owned()
        } else {
            String::new()
        })
        .child(row.name.clone());

    let line = div()
        .flex()
        .items_center()
        .gap_2()
        .child(release_cell)
        .child(cell(row.resolution.map(|r| format!("{r}p"))))
        .child(cell(row.codec.clone()))
        .child(cell(row.group.clone()))
        .child(cell(row.seeders.map(|s| s.to_string())))
        .child(cell(row.size.map(format_size)))
        .child(cell(row.published.map(format_date)))
        .child(cell(if row.batch {
            Some(
                row.batch_range
                    .clone()
                    .unwrap_or_else(|| "Batch".to_owned()),
            )
        } else {
            None
        }))
        .child(cell(if status.is_empty() {
            None
        } else {
            Some(status)
        }));

    let store_click = store.clone();
    let hash = infohash.clone();
    let batch = row.batch;
    let line = line.on_mouse_down(gpui_kit::MouseButton::Left, move |_, _, cx| {
        if batch {
            store_click.update(cx, |store, cx| store.toggle_expanded(&hash, cx));
        } else {
            store_click.update(cx, |store, cx| store.stream_single(&hash, cx));
        }
    });

    let mut element = div()
        .id(SharedString::from(format!("release-row-{infohash}")))
        .flex()
        .flex_col()
        .gap_1()
        .p_1()
        .rounded(px(6.))
        .opacity(if row.passes { 1.0 } else { 0.5 })
        .child(line);

    if let Some(message) = &view.amber {
        element = element.child(div().text_sm().text_color(amber).child(message.clone()));
    }

    if view.expanded && row.batch {
        element = element.child(batch_view(store, &infohash, &view.batch, muted));
    }

    element
}

fn cell(text: Option<String>) -> impl IntoElement {
    div().w(px(72.)).text_sm().child(text.unwrap_or_default())
}

fn batch_view(
    store: &Entity<ReleaseListStore>,
    infohash: &str,
    state: &BatchView,
    muted: gpui_kit::Hsla,
) -> impl IntoElement {
    let mut list = div().flex().flex_col().gap_1().p_2();
    match state {
        BatchView::Loading => {
            list = list.child(div().text_sm().text_color(muted).child("Loading…"));
        }
        BatchView::Failed => {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Could not load file list."),
            );
        }
        BatchView::Loaded(files) => {
            for file in files {
                let hash = infohash.to_owned();
                let index = file.index;
                let path = file.path.clone();
                let playable = file.playable;
                let preselected = file.preselected;
                let label = match file.episode {
                    Some(episode) => format!("E{episode} · {}", file.name),
                    None => file.name.clone(),
                };
                let store = store.clone();
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .opacity(if playable { 1.0 } else { 0.4 })
                        .child(div().flex_1().text_sm().child(label))
                        .child(cell(Some(format_size(file.size))))
                        .child(
                            Button::new(format!("release-file-{hash}-{index}"))
                                .label(if preselected { "▶ Stream" } else { "Stream" })
                                .disabled(!playable)
                                .when(preselected, Button::primary)
                                .on_click(move |_, _, cx| {
                                    let path = path.clone();
                                    store.update(cx, |store, cx| {
                                        store.stream_batch_file(&hash, index, &path, cx);
                                    });
                                }),
                        ),
                );
            }
        }
    }
    list
}

pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024 && unit < UNITS.len() - 1 {
        value /= 1024;
        unit += 1;
    }
    let label = UNITS.get(unit).copied().unwrap_or("TiB");
    if unit == 0 {
        format!("{bytes} {label}")
    } else {
        format!("{value} {label}")
    }
}

fn format_date(unix: u64) -> String {
    match time::OffsetDateTime::from_unix_timestamp(i64::try_from(unix).unwrap_or(0)) {
        Ok(dt) => format!(
            "{:04}-{:02}-{:02}",
            dt.year(),
            u8::from(dt.month()),
            dt.day()
        ),
        Err(_) => String::new(),
    }
}
