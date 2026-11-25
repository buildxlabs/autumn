//! The Library Explorer's tree view model and store (#135).
//!
//! Rows are derived from the library's local files, not from a live folder
//! walk, so nesting, annotations and filters are pure functions that can be
//! tested without a GUI.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use db::{FileFlag, LocalFile};
use domain::AppEvent;
use engine::EngineServices;
use gpui_kit::{Context, Task};

use crate::entry_store::call;

/// Which files the explorer shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExplorerFilter {
    /// Every local file.
    #[default]
    All,
    /// Files without a media.
    UnmatchedOnly,
    /// Files whose match is fixed.
    LockedOnly,
}

/// One row of the explorer tree.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ExplorerRow {
    /// The folder or file path.
    pub path: PathBuf,
    /// Nesting depth: library folders sit at zero.
    pub depth: usize,
    /// Whether the row is a folder.
    pub folder: bool,
    /// The local file, when this row is a file.
    pub file: Option<LocalFile>,
}

fn visible(file: &LocalFile, filter: ExplorerFilter) -> bool {
    match filter {
        ExplorerFilter::All => true,
        ExplorerFilter::UnmatchedOnly => file.media_id.is_none(),
        ExplorerFilter::LockedOnly => file.locked,
    }
}

/// Builds the tree rows for `folders` from the library's files.
#[must_use]
pub fn explorer_rows(
    files: &[LocalFile],
    folders: &[PathBuf],
    filter: ExplorerFilter,
) -> Vec<ExplorerRow> {
    let mut by_parent: BTreeMap<PathBuf, Vec<&LocalFile>> = BTreeMap::new();
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for file in files.iter().filter(|file| visible(file, filter)) {
        let Some(root) = folders.iter().find(|folder| file.path.starts_with(folder)) else {
            continue;
        };
        let Some(parent) = file.path.parent() else {
            continue;
        };
        by_parent
            .entry(parent.to_path_buf())
            .or_default()
            .push(file);
        dirs.insert(root.clone());
        let mut cursor = parent;
        while cursor.starts_with(root) && cursor != root {
            dirs.insert(cursor.to_path_buf());
            let Some(above) = cursor.parent() else {
                break;
            };
            cursor = above;
        }
    }

    let mut roots: Vec<PathBuf> = folders.to_vec();
    roots.sort();
    roots.dedup();
    let mut rows = Vec::new();
    for root in roots {
        if dirs.contains(&root) {
            rows.push(ExplorerRow {
                path: root.clone(),
                depth: 0,
                folder: true,
                file: None,
            });
            emit(&root, 0, &dirs, &by_parent, &mut rows);
        }
    }
    rows
}

fn emit(
    dir: &Path,
    depth: usize,
    dirs: &BTreeSet<PathBuf>,
    by_parent: &BTreeMap<PathBuf, Vec<&LocalFile>>,
    rows: &mut Vec<ExplorerRow>,
) {
    if let Some(files) = by_parent.get(dir) {
        for file in files {
            rows.push(ExplorerRow {
                path: file.path.clone(),
                depth: depth + 1,
                folder: false,
                file: Some((*file).clone()),
            });
        }
    }
    let children: Vec<PathBuf> = dirs
        .iter()
        .filter(|candidate| candidate.parent() == Some(dir))
        .cloned()
        .collect();
    for child in children {
        rows.push(ExplorerRow {
            path: child.clone(),
            depth: depth + 1,
            folder: true,
            file: None,
        });
        emit(&child, depth + 1, dirs, by_parent, rows);
    }
}

/// The store behind the explorer sheet, kept in sync by scan events.
pub struct LibraryExplorerStore {
    services: Arc<dyn EngineServices>,
    files: Vec<LocalFile>,
    filter: ExplorerFilter,
    load_task: Option<Task<()>>,
    command_task: Option<Task<()>>,
    error: Option<String>,
}

impl std::fmt::Debug for LibraryExplorerStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryExplorerStore")
            .field("files", &self.files.len())
            .field("filter", &self.filter)
            .finish_non_exhaustive()
    }
}

impl LibraryExplorerStore {
    /// A store that reads and updates the library through `services`.
    #[must_use]
    pub fn new(services: Arc<dyn EngineServices>) -> Self {
        Self {
            services,
            files: Vec::new(),
            filter: ExplorerFilter::default(),
            load_task: None,
            command_task: None,
            error: None,
        }
    }

    /// The local files, after the latest load.
    #[must_use]
    pub fn files(&self) -> &[LocalFile] {
        &self.files
    }

    /// The active filter.
    #[must_use]
    pub fn filter(&self) -> ExplorerFilter {
        self.filter
    }

    /// The tree rows for the Library folders.
    #[must_use]
    pub fn rows(&self, folders: &[PathBuf]) -> Vec<ExplorerRow> {
        explorer_rows(&self.files, folders, self.filter)
    }

    /// Loads the files on first open.
    pub fn open(&mut self, cx: &mut Context<'_, Self>) {
        self.reload(cx);
    }

    /// Takes an error from the last command, once.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    /// Applies a scan event from the foreground pump.
    pub fn apply(&mut self, event: &AppEvent, cx: &mut Context<'_, Self>) {
        if matches!(
            event,
            AppEvent::LibraryScanFinished { .. } | AppEvent::LibraryChanged
        ) {
            self.reload(cx);
        }
    }

    /// Changes the filter without touching the engine.
    pub fn set_filter(&mut self, filter: ExplorerFilter, cx: &mut Context<'_, Self>) {
        if self.filter != filter {
            self.filter = filter;
            cx.notify();
        }
    }

    /// Sets or clears a file's lock.
    pub fn set_lock(&mut self, path: PathBuf, flag: FileFlag, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.command_task = Some(cx.spawn(async move |store, cx| {
            match call(cx, services.set_library_lock(&path, flag)).await {
                Ok(_) => {
                    let _ = store.update(cx, |store, cx| {
                        store.error = None;
                        store.reload(cx);
                    });
                }
                Err(error) => {
                    let _ = store.update(cx, |store, cx| {
                        store.error = Some(error);
                        cx.notify();
                    });
                }
            }
        }));
    }

    /// Sets or clears a file's ignored flag.
    pub fn set_ignored(&mut self, path: PathBuf, flag: FileFlag, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.command_task = Some(cx.spawn(async move |store, cx| {
            match call(cx, services.set_library_ignored(&path, flag)).await {
                Ok(_) => {
                    let _ = store.update(cx, |store, cx| {
                        store.error = None;
                        store.reload(cx);
                    });
                }
                Err(error) => {
                    let _ = store.update(cx, |store, cx| {
                        store.error = Some(error);
                        cx.notify();
                    });
                }
            }
        }));
    }

    /// Renames a file, updating its tree row through the next reload.
    pub fn rename(&mut self, from: PathBuf, to: PathBuf, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.command_task = Some(cx.spawn(async move |store, cx| {
            match call(cx, services.rename_library_file(&from, &to)).await {
                Ok(_) => {
                    let _ = store.update(cx, |store, cx| {
                        store.error = None;
                        store.reload(cx);
                    });
                }
                Err(error) => {
                    let _ = store.update(cx, |store, cx| {
                        store.error = Some(error);
                        cx.notify();
                    });
                }
            }
        }));
    }

    /// Moves files to the OS trash and removes their rows.
    pub fn trash(&mut self, paths: Vec<PathBuf>, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.command_task = Some(cx.spawn(async move |store, cx| {
            match call(cx, services.trash_library_files(&paths)).await {
                Ok(_) => {
                    let _ = store.update(cx, |store, cx| {
                        store.error = None;
                        store.reload(cx);
                    });
                }
                Err(error) => {
                    let _ = store.update(cx, |store, cx| {
                        store.error = Some(error);
                        cx.notify();
                    });
                }
            }
        }));
    }

    /// Re-matches a file to AniList media.
    pub fn rematch(&mut self, path: PathBuf, anilist_id: u32, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.command_task = Some(cx.spawn(async move |store, cx| {
            match call(
                cx,
                services.manual_match(std::slice::from_ref(&path), anilist_id, None),
            )
            .await
            {
                Ok(_) => {
                    let _ = store.update(cx, |store, cx| {
                        store.error = None;
                        store.reload(cx);
                    });
                }
                Err(error) => {
                    let _ = store.update(cx, |store, cx| {
                        store.error = Some(error);
                        cx.notify();
                    });
                }
            }
        }));
    }

    fn reload(&mut self, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.load_task = Some(cx.spawn(async move |store, cx| {
            let Ok(files) = call(cx, services.all_library_files()).await else {
                return;
            };
            let _ = store.update(cx, |store, cx| {
                store.files = files;
                cx.notify();
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use domain::MediaId;

    use super::*;

    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "the fixture only needs one lock flag"
    )]
    fn file(path: &str, media: Option<u32>, locked: bool) -> LocalFile {
        LocalFile {
            path: PathBuf::from(path),
            size: 0,
            modified: 0,
            title: None,
            group: None,
            season: None,
            episode: None,
            kind: "episode".to_owned(),
            media_id: media.map(|id| MediaId::new(i64::from(id))),
            confidence: media.map(|_| 0.8),
            locked,
            ignored: false,
            filler: false,
            shelved: false,
        }
    }

    fn fixtures() -> Vec<LocalFile> {
        vec![
            file("/shows/Alpha/Season 01/01.mkv", Some(1), false),
            file("/shows/Alpha/Season 01/02.mkv", Some(1), true),
            file("/shows/Beta/01.mkv", None, false),
        ]
    }

    #[test]
    fn nests_files_under_their_folders_with_annotations() {
        let folders = [PathBuf::from("/shows")];
        let rows = explorer_rows(&fixtures(), &folders, ExplorerFilter::All);

        let folder = |path: &str| {
            rows.iter()
                .find(|row| row.folder && row.path == Path::new(path))
                .cloned()
                .expect("the folder row exists")
        };
        assert_eq!(folder("/shows").depth, 0);
        assert_eq!(folder("/shows/Alpha").depth, 1);
        assert_eq!(folder("/shows/Alpha/Season 01").depth, 2);

        let episode = rows
            .iter()
            .find(|row| row.path.ends_with("01.mkv") && !row.folder)
            .expect("the episode row");
        assert_eq!(episode.depth, 3);
        assert_eq!(
            episode.file.as_ref().and_then(|file| file.media_id),
            Some(MediaId::new(1)),
            "the matched media is on the row"
        );
    }

    #[test]
    fn filters_keep_only_the_matching_files_and_their_folders() {
        let folders = [PathBuf::from("/shows")];
        let files = fixtures();
        let unmatched = explorer_rows(&files, &folders, ExplorerFilter::UnmatchedOnly);
        assert_eq!(
            unmatched
                .iter()
                .filter(|row| !row.folder)
                .map(|row| row.path.clone())
                .collect::<Vec<_>>(),
            [PathBuf::from("/shows/Beta/01.mkv")]
        );
        assert!(
            !unmatched
                .iter()
                .any(|row| row.folder && row.path == Path::new("/shows/Alpha")),
            "empty folders are not shown"
        );

        let locked = explorer_rows(&files, &folders, ExplorerFilter::LockedOnly);
        assert_eq!(
            locked
                .iter()
                .filter(|row| !row.folder)
                .map(|row| row.path.clone())
                .collect::<Vec<_>>(),
            [PathBuf::from("/shows/Alpha/Season 01/02.mkv")]
        );
        assert!(
            locked
                .iter()
                .any(|row| row.file.as_ref().is_some_and(|file| file.locked))
        );
    }
}
