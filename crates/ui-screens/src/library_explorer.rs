//! The Library Explorer sheet: a tree of library folders and files (#135).
//!
//! Actions go through the explorer store, which uses the bridge; this view
//! only builds tree items and hands clicks back to the store.

use std::{collections::HashMap, fmt::Write as _, path::PathBuf, rc::Rc};

use db::FileFlag;
use domain::MediaId;
use gpui_kit::{
    App, AppContext, ClickEvent, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, Styled, Subscription, TestSupportExt, Window,
    component::{
        ActiveTheme, WindowExt,
        button::{Button, ButtonVariants},
        input::Input,
        label::Label,
        list::ListItem,
        tree::{TreeItem, TreeState},
    },
    div, px,
};
use metadata::AniList;
use ui_state::{ExplorerFilter, ExplorerRow, LibraryExplorerStore, ScanSummariesStore};

use crate::scan_summaries::MediaSearchDialog;

type TrashCallback = Rc<dyn Fn(Vec<PathBuf>, &mut Window, &mut App)>;

/// The explorer's tree view.
pub(crate) struct LibraryExplorerScreen {
    store: Entity<LibraryExplorerStore>,
    summaries: Entity<ScanSummariesStore>,
    tree: Entity<TreeState>,
    folders: Vec<PathBuf>,
    dialog: Entity<MediaSearchDialog>,
    rename: Entity<gpui_kit::component::input::InputState>,
    /// The file being renamed, recorded when Rename was clicked. The input
    /// closes when the selection moves, so confirm never renames another row.
    renaming: Option<PathBuf>,
    _store_events: Subscription,
    _tree_events: Subscription,
    _dialog_events: Subscription,
    on_trash: Option<TrashCallback>,
}

impl std::fmt::Debug for LibraryExplorerScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryExplorerScreen")
            .field("folders", &self.folders)
            .finish_non_exhaustive()
    }
}

impl LibraryExplorerScreen {
    /// Builds the sheet view over the shared explorer store.
    pub(crate) fn new(
        store: Entity<LibraryExplorerStore>,
        summaries: Entity<ScanSummariesStore>,
        anilist: AniList,
        folders: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let dialog = cx.new(|cx| MediaSearchDialog::new(window, anilist, cx));
        let rename = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx).placeholder("New file name")
        });
        let events = cx.observe(&store, |this, store, cx| {
            let folders = this.folders.clone();
            let rows = store.read(cx).rows(&folders);
            let items = tree_items(&rows);
            this.tree.update(cx, |state, cx| state.set_items(items, cx));
            cx.notify();
        });
        let tree_events = cx.observe(&tree, |this: &mut Self, tree, cx| {
            let selected = tree
                .read(cx)
                .selected_item()
                .map(|item| PathBuf::from(item.id.to_string()));
            if this.renaming.is_some() && this.renaming != selected {
                this.renaming = None;
                cx.notify();
            }
        });
        let dialog_events = cx.subscribe(&dialog, |this, _, event, cx| {
            let crate::scan_summaries::MediaSearchEvent::Matched { path, anilist_id } = event;
            this.store
                .update(cx, |store, cx| store.rematch(path, *anilist_id, cx));
        });
        Self {
            store,
            summaries,
            tree,
            folders,
            dialog,
            rename,
            renaming: None,
            _store_events: events,
            _tree_events: tree_events,
            _dialog_events: dialog_events,
            on_trash: None,
        }
    }

    /// Sets the host callback that shows the trash confirmation dialog.
    pub(crate) fn set_on_trash(
        &mut self,
        on_trash: impl Fn(Vec<PathBuf>, &mut Window, &mut App) + 'static,
    ) {
        self.on_trash = Some(Rc::new(on_trash));
    }

    /// Uses the Library settings' current folders, not a stale snapshot.
    pub(crate) fn set_folders(&mut self, folders: Vec<PathBuf>, cx: &mut Context<'_, Self>) {
        self.folders = folders;
        let rows = self.store.read(cx).rows(&self.folders);
        self.tree
            .update(cx, |state, cx| state.set_items(tree_items(&rows), cx));
        cx.notify();
    }

    /// Shows the AniList search dialog over the sheet.
    fn open_dialog(&self, path: PathBuf, window: &mut Window, cx: &mut Context<'_, Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let dialog = self.dialog.clone();
        window.open_dialog(cx, move |dialog_view, _, _| {
            dialog_view
                .w(px(480.))
                .title("Match a local file")
                .child(dialog.clone())
        });
        self.dialog
            .update(cx, |dialog, cx| dialog.open(path, window, cx));
    }

    /// Opens the sheet, refreshing the tree first.
    pub(crate) fn open(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.store.update(cx, LibraryExplorerStore::open);
        let screen = cx.entity();
        window.open_sheet(cx, move |sheet, _, _| {
            sheet
                .title("Library Explorer")
                .w(px(560.))
                .child(screen.clone())
        });
    }
}

impl Render for LibraryExplorerScreen {
    #[expect(
        clippy::too_many_lines,
        reason = "one renderer builds the toolbar, filters, rename and tree"
    )]
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let filter = self.store.read(cx).filter();
        let rows = self.store.read(cx).rows(&self.folders);
        let titles: HashMap<MediaId, String> = self
            .summaries
            .read(cx)
            .media()
            .iter()
            .filter_map(|info| info.title.clone().map(|title| (info.media, title)))
            .collect();
        let labels = row_labels(&rows, &titles);
        let selected = self
            .tree
            .read(cx)
            .selected_item()
            .map(|item| PathBuf::from(item.id.to_string()));
        let colors = cx.theme().colors;
        let mut actions = div().flex().flex_wrap().gap_2();
        let selected_is_folder = selected
            .as_ref()
            .is_some_and(|path| rows.iter().any(|row| row.folder && &row.path == path));
        if let Some(path) = selected.filter(|_| !selected_is_folder) {
            let locked = rows
                .iter()
                .find(|row| row.path == path)
                .and_then(|row| row.file.as_ref())
                .is_some_and(|file| file.locked);
            let store = self.store.clone();
            let lock_path = path.clone();
            actions = actions.child(
                Button::new("explorer-lock")
                    .label(if locked { "Unlock" } else { "Lock" })
                    .on_click(move |_, _, cx| {
                        store.update(cx, |store, cx| {
                            store.set_lock(
                                &lock_path,
                                if locked {
                                    FileFlag::Clear
                                } else {
                                    FileFlag::Set
                                },
                                cx,
                            );
                        });
                    }),
            );
            let rematch_path = path.clone();
            let screen = cx.entity();
            actions = actions.child(Button::new("explorer-rematch").label("Re-match…").on_click(
                move |_, window, cx| {
                    screen.update(cx, |screen, cx| {
                        screen.open_dialog(rematch_path.clone(), window, cx);
                    });
                },
            ));
            let store = self.store.clone();
            let ignore_path = path.clone();
            actions = actions.child(Button::new("explorer-ignore").label("Ignore").on_click(
                move |_, _, cx| {
                    store.update(cx, |store, cx| {
                        store.set_ignored(&ignore_path, FileFlag::Set, cx);
                    });
                },
            ));
            let screen = cx.entity();
            let rename_path = path.clone();
            actions = actions.child(Button::new("explorer-rename").label("Rename").on_click(
                move |_, _, cx| {
                    screen.update(cx, |screen, cx| {
                        screen.renaming = Some(rename_path.clone());
                        cx.notify();
                    });
                },
            ));
            let screen = cx.entity();
            actions = actions.child(
                Button::new("explorer-trash")
                    .label("Move to trash")
                    .on_click(move |_, window, cx| {
                        screen.update(cx, |screen, cx| screen.ask_trash(window, cx));
                    }),
            );
            actions = actions.child(
                Button::new("explorer-reveal")
                    .label("Reveal in file manager")
                    .on_click(move |_, _, cx| {
                        cx.reveal_path(&path);
                    }),
            );
        }
        let filter_row = div()
            .flex()
            .gap_3()
            .child(filter_button(
                "explorer-all",
                "All",
                filter == ExplorerFilter::All,
                {
                    let store = self.store.clone();
                    move |_, _, cx: &mut App| {
                        store.update(cx, |store, cx| store.set_filter(ExplorerFilter::All, cx));
                    }
                },
            ))
            .child(filter_button(
                "explorer-unmatched",
                "Unmatched only",
                filter == ExplorerFilter::UnmatchedOnly,
                {
                    let store = self.store.clone();
                    move |_, _, cx: &mut App| {
                        store.update(cx, |store, cx| {
                            store.set_filter(ExplorerFilter::UnmatchedOnly, cx);
                        });
                    }
                },
            ))
            .child(filter_button(
                "explorer-locked",
                "Locked only",
                filter == ExplorerFilter::LockedOnly,
                {
                    let store = self.store.clone();
                    move |_, _, cx: &mut App| {
                        store.update(cx, |store, cx| {
                            store.set_filter(ExplorerFilter::LockedOnly, cx);
                        });
                    }
                },
            ));
        let rename = self.renaming.clone().map(|from| {
            let store = self.store.clone();
            let input = self.rename.clone();
            let screen = cx.entity();
            div().flex().gap_2().child(Input::new(&input)).child(
                Button::new("explorer-rename-confirm")
                    .label("Rename")
                    .on_click(move |_, _, cx| {
                        let from = from.clone();
                        let name = input.read(cx).value().to_string();
                        let Some(parent) = from.parent().map(PathBuf::from) else {
                            return;
                        };
                        let to = parent.join(name);
                        store.update(cx, |store, cx| store.rename(&from, &to, cx));
                        screen.update(cx, |screen, cx| {
                            screen.renaming = None;
                            cx.notify();
                        });
                    }),
            )
        });
        div()
            .id("library-explorer")
            .test_support()
            .flex()
            .flex_col()
            .gap_3()
            .size_full()
            .bg(colors.background)
            .child(filter_row)
            .children(rename)
            .child(actions)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(gpui_kit::component::tree::tree(
                        &self.tree,
                        move |_, entry, _, _, _| {
                            let id = entry.item().id.clone();
                            let label = labels
                                .iter()
                                .find(|(key, _)| key.as_str() == id.as_ref())
                                .map_or_else(|| id.to_string(), |(_, label)| label.clone());
                            ListItem::new(id).child(Label::new(label))
                        },
                    )),
            )
    }
}

impl LibraryExplorerScreen {
    fn ask_trash(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(path) = self
            .tree
            .read(cx)
            .selected_item()
            .map(|item| PathBuf::from(item.id.to_string()))
        else {
            return;
        };
        if let Some(on_trash) = self.on_trash.clone() {
            on_trash(vec![path], window, cx);
        }
    }
}

#[expect(
    clippy::fn_params_excessive_bools,
    reason = "one toggle state for the filter chip"
)]
fn filter_button(
    id: &'static str,
    label: &'static str,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let button = Button::new(id).label(label);
    if active {
        button.primary().on_click(on_click)
    } else {
        button.ghost().on_click(on_click)
    }
}

fn tree_items(rows: &[ExplorerRow]) -> Vec<TreeItem> {
    let mut roots: Vec<TreeItem> = Vec::new();
    let mut stack: Vec<(usize, TreeItem)> = Vec::new();
    for row in rows {
        let item = TreeItem::new(
            row.path.to_string_lossy().to_string(),
            row.path.file_name().map_or_else(
                || row.path.to_string_lossy().to_string(),
                |name| name.to_string_lossy().to_string(),
            ),
        );
        while stack.last().is_some_and(|(depth, _)| *depth >= row.depth) {
            let Some((_, finished)) = stack.pop() else {
                break;
            };
            if let Some((_, parent)) = stack.last_mut() {
                parent.children.push(finished);
            } else {
                roots.push(finished);
            }
        }
        if row.folder {
            stack.push((row.depth, item));
        } else if let Some((_, parent)) = stack.last_mut() {
            parent.children.push(item);
        } else {
            roots.push(item);
        }
    }
    while let Some((_, finished)) = stack.pop() {
        if let Some((_, parent)) = stack.last_mut() {
            parent.children.push(finished);
        } else {
            roots.push(finished);
        }
    }
    roots
}

fn row_labels(rows: &[ExplorerRow], titles: &HashMap<MediaId, String>) -> Vec<(String, String)> {
    rows.iter()
        .map(|row| {
            let mut label = row.path.file_name().map_or_else(
                || row.path.to_string_lossy().to_string(),
                |name| name.to_string_lossy().to_string(),
            );
            if let Some(file) = &row.file {
                if let Some(media) = file.media_id {
                    let title = titles
                        .get(&media)
                        .cloned()
                        .unwrap_or_else(|| format!("media {}", media.get()));
                    let _ = write!(label, " · {title}");
                    if let Some(episode) = file.episode {
                        let _ = write!(label, " E{episode:02}");
                    }
                } else {
                    label.push_str(" · unmatched");
                }
                if file.locked {
                    label.push_str(" · locked");
                }
                if file.ignored {
                    label.push_str(" · ignored");
                }
                if file.shelved {
                    label.push_str(" · shelved");
                }
            }
            (row.path.to_string_lossy().to_string(), label)
        })
        .collect()
}
