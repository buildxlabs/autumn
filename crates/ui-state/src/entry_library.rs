//! The Entry page's Library tab (#132): the media's local files joined with
//! `AniZip` titles and thumbnails, the list entry's progress and resume
//! positions, split into sub-tabs and 100-episode ranges.
//!
//! Behavior follows Seanime's Library tab (`internal/library/anime/entry.go`
//! and `episode_collection.go`, GPL-3.0); no code was ported.

use std::{path::PathBuf, sync::Arc};

use db::{Continuity, LocalFile};
use domain::{AppEvent, ExternalId, MediaId, MediaKind, PlaybackDetails, StreamSource};
use engine::{EngineServices, PlayRequest};
use gpui_kit::{Context, Task};

use crate::entry_store::call;

/// Media with more episodes than this page them in ranges of this size.
pub const RANGE_SIZE: u32 = 100;

/// A sub-tab of the Library tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the Library tab renders every sub-tab, in declaration order"
)]
pub enum LibrarySection {
    /// The regular episodes, and a movie's file.
    Episodes,
    /// Specials and OVAs.
    Specials,
    /// Creditless openings and endings.
    NonCredit,
}

impl LibrarySection {
    /// The sub-tab's label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Episodes => "Episodes",
            Self::Specials => "Specials",
            Self::NonCredit => "NCOP/NCED",
        }
    }
}

/// How the episodes are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the Library tab renders both layouts"
)]
pub enum EpisodeLayout {
    /// Cards in a wrapping grid.
    #[default]
    Grid,
    /// One row per episode.
    Rows,
}

/// A range of episode numbers, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct EpisodeRange {
    /// The first episode number.
    pub start: u32,
    /// The last episode number.
    pub end: u32,
}

impl EpisodeRange {
    /// The range of [`RANGE_SIZE`] episodes that holds `number`.
    #[must_use]
    pub fn holding(number: u32) -> Self {
        let block = number.saturating_sub(1) / RANGE_SIZE;
        let start = block * RANGE_SIZE + 1;
        Self {
            start,
            end: start + RANGE_SIZE - 1,
        }
    }

    /// Whether `number` is in the range.
    #[must_use]
    pub fn contains(self, number: u32) -> bool {
        (self.start..=self.end).contains(&number)
    }

    /// The chip label, such as `101–200`.
    #[must_use]
    pub fn label(self) -> String {
        format!("{}–{}", self.start, self.end)
    }
}

/// One local file as the Library tab shows it.
#[expect(
    clippy::struct_excessive_bools,
    reason = "filler, locked, shelved and watched are independent facts about the file"
)]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LibraryEpisode {
    /// The sub-tab the file belongs to.
    pub section: LibrarySection,
    /// The episode number within its section.
    pub number: u32,
    /// The `AniZip` title, or a numbered fallback.
    pub title: String,
    /// The `AniZip` thumbnail URL.
    pub thumbnail: Option<String>,
    /// The file on disk.
    pub path: PathBuf,
    /// Whether the episode is listed as filler.
    pub filler: bool,
    /// Whether the file's match is locked.
    pub locked: bool,
    /// Whether the file's drive is disconnected, so it can't play.
    pub shelved: bool,
    /// Whether the list entry's progress covers this episode.
    pub watched: bool,
    /// The resume position as a fraction of the duration.
    pub progress: Option<f32>,
}

impl LibraryEpisode {
    /// The file name, for the card's badge.
    #[must_use]
    pub fn file_name(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }
}

/// The joined library of the open media.
#[derive(Debug, Clone)]
struct Loaded {
    media: MediaId,
    /// Every episode, by section, then number, then path.
    episodes: Vec<LibraryEpisode>,
}

/// The Library tab's store: loads one media's files and their metadata, and
/// holds the selected sub-tab, range and layout.
pub struct EntryLibraryStore {
    services: Arc<dyn EngineServices>,
    anilist_id: Option<u32>,
    loaded: Option<Loaded>,
    section: LibrarySection,
    /// The range the user picked; `None` follows the next unwatched episode.
    range: Option<EpisodeRange>,
    layout: EpisodeLayout,
    load_task: Option<Task<()>>,
}

impl std::fmt::Debug for EntryLibraryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntryLibraryStore")
            .field("anilist_id", &self.anilist_id)
            .field("section", &self.section)
            .field("range", &self.range)
            .finish_non_exhaustive()
    }
}

impl EntryLibraryStore {
    /// A store with no media open.
    #[must_use]
    pub fn new(services: Arc<dyn EngineServices>) -> Self {
        Self {
            services,
            anilist_id: None,
            loaded: None,
            section: LibrarySection::Episodes,
            range: None,
            layout: EpisodeLayout::default(),
            load_task: None,
        }
    }

    /// Opens the Library of AniList `anilist_id`. Reopening the same media
    /// keeps the selected sub-tab and range.
    pub fn open(&mut self, anilist_id: u32, cx: &mut Context<'_, Self>) {
        if self.anilist_id != Some(anilist_id) {
            self.anilist_id = Some(anilist_id);
            self.loaded = None;
            self.section = LibrarySection::Episodes;
            self.range = None;
        }
        self.load(anilist_id, cx);
    }

    /// Reloads the open media after the library or the list entry changed.
    pub fn apply(&mut self, event: &AppEvent, cx: &mut Context<'_, Self>) {
        let reload = matches!(
            event,
            AppEvent::LibraryChanged
                | AppEvent::LibraryScanFinished { .. }
                | AppEvent::PlaybackStopped { .. }
        );
        if let (true, Some(anilist_id)) = (reload, self.anilist_id) {
            self.load(anilist_id, cx);
        }
    }

    /// Loads the media's files, `AniZip` data, list progress and resume
    /// positions. The last good library stays on screen until it lands.
    fn load(&mut self, anilist_id: u32, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.load_task = Some(cx.spawn(async move |store, cx| {
            let ids = [ExternalId::AniList(anilist_id)];
            let media = call(cx, services.local_media(MediaKind::Anime, &ids)).await;
            let loaded = match media {
                Ok(Some(media)) => {
                    let files = call(cx, services.library_files(media))
                        .await
                        .unwrap_or_default();
                    // Titles, progress and positions decorate the files; a
                    // failed read still lists what is on disk.
                    let metadata = if files.is_empty() {
                        None
                    } else {
                        call(cx, services.episode_metadata(anilist_id))
                            .await
                            .ok()
                            .flatten()
                    };
                    let progress = call(cx, services.local_entry(media))
                        .await
                        .ok()
                        .flatten()
                        .map_or(0, |entry| entry.progress);
                    let continuity = call(cx, services.continuity(media))
                        .await
                        .unwrap_or_default();
                    Loaded {
                        media,
                        episodes: join(&files, metadata.as_ref(), progress, &continuity),
                    }
                }
                // No media identity: nothing has been matched or recorded.
                _ => Loaded {
                    media: MediaId::new(0),
                    episodes: Vec::new(),
                },
            };
            let _ = store.update(cx, |store, cx| {
                if store.anilist_id == Some(anilist_id) {
                    store.loaded = Some(loaded);
                    cx.notify();
                }
            });
        }));
    }

    /// Whether the open media has any local file.
    #[must_use]
    pub fn has_files(&self) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|loaded| !loaded.episodes.is_empty())
    }

    /// The folder of the media's first file, for "Open local folder".
    #[must_use]
    pub fn folder(&self) -> Option<PathBuf> {
        self.all()
            .first()
            .and_then(|episode| episode.path.parent())
            .map(std::path::Path::to_path_buf)
    }

    /// Every loaded episode, of all sections.
    fn all(&self) -> &[LibraryEpisode] {
        self.loaded
            .as_ref()
            .map_or(&[], |loaded| loaded.episodes.as_slice())
    }

    /// The loaded episodes of `section`.
    fn of(&self, section: LibrarySection) -> impl Iterator<Item = &LibraryEpisode> {
        self.all()
            .iter()
            .filter(move |episode| episode.section == section)
    }

    /// Whether the media's files are still loading.
    #[must_use]
    pub fn is_loading(&self) -> bool {
        self.loaded.is_none()
    }

    /// The sub-tabs that have files, in display order.
    #[must_use]
    pub fn sections(&self) -> Vec<LibrarySection> {
        [
            LibrarySection::Episodes,
            LibrarySection::Specials,
            LibrarySection::NonCredit,
        ]
        .into_iter()
        .filter(|section| self.of(*section).next().is_some())
        .collect()
    }

    /// The selected sub-tab.
    #[must_use]
    pub fn section(&self) -> LibrarySection {
        self.section
    }

    /// Selects a sub-tab.
    pub fn select_section(&mut self, section: LibrarySection, cx: &mut Context<'_, Self>) {
        self.section = section;
        cx.notify();
    }

    /// The range chips of the Episodes sub-tab; empty up to 100 episodes.
    #[must_use]
    pub fn ranges(&self) -> Vec<EpisodeRange> {
        if self.section != LibrarySection::Episodes {
            return Vec::new();
        }
        let mut ranges: Vec<EpisodeRange> = Vec::new();
        for episode in self.of(LibrarySection::Episodes) {
            let range = EpisodeRange::holding(episode.number);
            if ranges.last() != Some(&range) {
                ranges.push(range);
            }
        }
        if ranges.last().is_some_and(|last| last.end <= RANGE_SIZE) {
            return Vec::new();
        }
        ranges
    }

    /// The range on screen.
    #[must_use]
    pub fn range(&self) -> Option<EpisodeRange> {
        let ranges = self.ranges();
        let picked = self.range.filter(|range| ranges.contains(range));
        let next = self
            .next_episode()
            .map(|episode| EpisodeRange::holding(episode.number))
            .filter(|range| ranges.contains(range));
        picked.or(next).or_else(|| ranges.first().copied())
    }

    /// Selects a range chip.
    pub fn select_range(&mut self, range: EpisodeRange, cx: &mut Context<'_, Self>) {
        self.range = Some(range);
        cx.notify();
    }

    /// The episodes of the selected sub-tab and range.
    #[must_use]
    pub fn episodes(&self) -> Vec<LibraryEpisode> {
        let range = self.range();
        self.of(self.section)
            .filter(|episode| range.is_none_or(|range| range.contains(episode.number)))
            .cloned()
            .collect()
    }

    /// The episode Play next starts: the first unwatched regular episode
    /// that can play. Its resume position, if any, is applied by playback.
    #[must_use]
    pub fn next_episode(&self) -> Option<&LibraryEpisode> {
        self.of(LibrarySection::Episodes)
            .find(|episode| !episode.watched && !episode.shelved)
    }

    /// The request that plays `episode`, or `None` when it can't play.
    #[must_use]
    pub fn play_request(
        &self,
        episode: &LibraryEpisode,
        media_title: &str,
        poster: Option<String>,
    ) -> Option<PlayRequest> {
        let media = self.loaded.as_ref()?.media;
        if episode.shelved {
            return None;
        }
        // Progress and resume positions are keyed by regular episode
        // number; specials and creditless files play without either.
        let number = match episode.section {
            LibrarySection::Episodes => episode.number,
            LibrarySection::Specials | LibrarySection::NonCredit => 0,
        };
        Some(
            PlayRequest::new(media, number, StreamSource::local(&episode.path)).with_details(
                PlaybackDetails::new(media_title, Some(episode.title.clone()), poster),
            ),
        )
    }

    /// The layout of the episode list.
    #[must_use]
    pub fn layout(&self) -> EpisodeLayout {
        self.layout
    }

    /// Switches between the grid and the rows.
    pub fn toggle_layout(&mut self, cx: &mut Context<'_, Self>) {
        self.layout = match self.layout {
            EpisodeLayout::Grid => EpisodeLayout::Rows,
            EpisodeLayout::Rows => EpisodeLayout::Grid,
        };
        cx.notify();
    }
}

/// Joins `files` with their `AniZip` entries, the list `progress` and the
/// saved resume positions. Ignored files are left out.
fn join(
    files: &[LocalFile],
    metadata: Option<&metadata::Media>,
    progress: u32,
    continuity: &[(u32, Continuity)],
) -> Vec<LibraryEpisode> {
    let mut episodes: Vec<LibraryEpisode> = files
        .iter()
        .filter(|file| !file.ignored)
        .filter_map(|file| {
            let section = match file.kind.as_str() {
                "episode" | "movie" => LibrarySection::Episodes,
                "special" => LibrarySection::Specials,
                "ncop" | "nced" => LibrarySection::NonCredit,
                _ => return None,
            };
            // A movie or a single-episode release names no number.
            let number = file.episode.unwrap_or(1);
            let key = match section {
                LibrarySection::Episodes => Some(number.to_string()),
                LibrarySection::Specials => Some(format!("S{number}")),
                LibrarySection::NonCredit => None,
            };
            let entry = key.and_then(|key| metadata?.episodes.get(&key));
            let regular = section == LibrarySection::Episodes;
            Some(LibraryEpisode {
                section,
                number,
                title: entry
                    .and_then(episode_title)
                    .unwrap_or_else(|| fallback_title(file, number)),
                thumbnail: entry.and_then(|entry| entry.image.clone()),
                path: file.path.clone(),
                filler: file.filler,
                locked: file.locked,
                shelved: file.shelved,
                watched: regular && number <= progress,
                progress: continuity
                    .iter()
                    .find(|(episode, _)| regular && *episode == number)
                    .and_then(|(_, position)| fraction(position)),
            })
        })
        .collect();
    episodes.sort_by(|left, right| {
        (left.section, left.number, &left.path).cmp(&(right.section, right.number, &right.path))
    });
    episodes
}

/// The English title, else the romanized one, else any.
fn episode_title(entry: &metadata::Episode) -> Option<String> {
    ["en", "x-jat"]
        .iter()
        .find_map(|language| entry.titles.get(*language))
        .or_else(|| entry.titles.values().next())
        .filter(|title| !title.trim().is_empty())
        .cloned()
}

/// The title of a file `AniZip` doesn't describe.
fn fallback_title(file: &LocalFile, number: u32) -> String {
    match file.kind.as_str() {
        "special" => format!("Special {number}"),
        "ncop" => format!("NCOP {number}"),
        "nced" => format!("NCED {number}"),
        "movie" => "Movie".to_owned(),
        _ => format!("Episode {number}"),
    }
}

/// A resume position as a fraction of the duration, when it has one.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "a progress bar fraction needs no f64 precision"
)]
fn fraction(position: &Continuity) -> Option<f32> {
    let duration = position.duration.filter(|duration| *duration > 0.0)?;
    Some((position.position / duration).clamp(0.0, 1.0) as f32)
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "a test that indexes past the end fails, which is the assertion"
)]
mod tests {
    use gpui_kit::{AppContext, Entity, TestAppContext};
    use tokio::runtime::{Builder, Runtime};

    use crate::{
        bridge,
        test_support::{EntryLibraryServices, library_file},
    };

    use super::*;

    const ID: u32 = 21;

    fn runtime(cx: &mut TestAppContext) -> Runtime {
        cx.executor().allow_parking();
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        runtime
    }

    /// Opens the store on [`ID`] and waits for the load to land.
    fn open(cx: &mut TestAppContext, services: EntryLibraryServices) -> Entity<EntryLibraryStore> {
        let store = cx.new(|_| EntryLibraryStore::new(Arc::new(services)));
        cx.update(|cx| store.update(cx, |store, cx| store.open(ID, cx)));
        settle(cx, &store);
        store
    }

    /// Runs the bridge until the store has loaded.
    fn settle(cx: &mut TestAppContext, store: &Entity<EntryLibraryStore>) {
        for _ in 0..100 {
            cx.run_until_parked();
            if !store.read_with(cx, |store, _| store.is_loading()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            !store.read_with(cx, |store, _| store.is_loading()),
            "the library never loaded"
        );
    }

    fn anizip_episode(key: &str, title: &str, image: &str) -> (String, metadata::Episode) {
        let mut episode = metadata::Episode::default();
        episode.key = key.to_owned();
        episode.image = Some(image.to_owned());
        episode.titles.insert("en".to_owned(), title.to_owned());
        (key.to_owned(), episode)
    }

    #[gpui_kit::test]
    fn files_are_joined_with_their_titles_progress_and_resume_positions(cx: &mut TestAppContext) {
        let _runtime = runtime(cx);
        let media = MediaId::new(7);
        let metadata = metadata::Media {
            episodes: [
                anizip_episode("1", "The Beginning", "https://img.test/1.jpg"),
                anizip_episode("2", "The Middle", "https://img.test/2.jpg"),
            ]
            .into_iter()
            .collect(),
            ..metadata::Media::default()
        };
        let mut shelved = library_file("/shows/ep3.mkv", media, "episode", Some(3));
        shelved.shelved = true;
        let services = EntryLibraryServices::new(
            media,
            vec![
                library_file("/shows/ep2.mkv", media, "episode", Some(2)),
                library_file("/shows/ep1.mkv", media, "episode", Some(1)),
                shelved,
            ],
        )
        .with_metadata(metadata)
        .with_progress(1)
        .with_continuity(vec![(
            2,
            Continuity {
                position: 600.0,
                duration: Some(1200.0),
            },
        )]);

        let store = open(cx, services);

        let episodes = store.read_with(cx, |store, _| store.episodes());
        let shown: Vec<_> = episodes
            .iter()
            .map(|episode| {
                (
                    episode.number,
                    episode.title.as_str(),
                    episode.thumbnail.as_deref(),
                    episode.watched,
                    episode.progress,
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (
                    1,
                    "The Beginning",
                    Some("https://img.test/1.jpg"),
                    true,
                    None
                ),
                (
                    2,
                    "The Middle",
                    Some("https://img.test/2.jpg"),
                    false,
                    Some(0.5)
                ),
                (3, "Episode 3", None, false, None),
            ],
            "files in episode order, with AniZip titles and a fallback"
        );
        assert!(episodes[2].shelved, "the shelved file is marked");
        let next = store.read_with(cx, |store, _| store.next_episode().map(|e| e.number));
        assert_eq!(
            next,
            Some(2),
            "Play next resumes the episode after progress"
        );
        let request = store
            .read_with(cx, |store, _| {
                store.play_request(&episodes[1], "Show", None)
            })
            .unwrap();
        assert_eq!(
            (request.media, request.episode, request.source),
            (media, 2, StreamSource::local("/shows/ep2.mkv"))
        );
        assert_eq!(
            store.read_with(cx, |store, _| store.play_request(
                &episodes[2],
                "Show",
                None
            )),
            None,
            "a shelved file never plays"
        );
    }

    #[gpui_kit::test]
    fn a_long_media_opens_on_the_range_of_its_next_unwatched_episode(cx: &mut TestAppContext) {
        let _runtime = runtime(cx);
        let media = MediaId::new(9);
        let files = (1..=1_000)
            .map(|number| {
                library_file(
                    &format!("/long/{number:04}.mkv"),
                    media,
                    "episode",
                    Some(number),
                )
            })
            .collect();
        let services = EntryLibraryServices::new(media, files).with_progress(437);

        let store = open(cx, services);

        let ranges = store.read_with(cx, |store, _| store.ranges());
        assert_eq!(ranges.len(), 10, "one chip per hundred episodes");
        assert_eq!(ranges[0].label(), "1–100");
        assert_eq!(ranges[9].label(), "901–1000");
        let range = store.read_with(cx, |store, _| store.range()).unwrap();
        assert_eq!(range.label(), "401–500", "opens where episode 438 is");
        let numbers: Vec<u32> = store
            .read_with(cx, |store, _| store.episodes())
            .iter()
            .map(|episode| episode.number)
            .collect();
        assert_eq!(numbers, (401..=500).collect::<Vec<_>>());

        cx.update(|cx| store.update(cx, |store, cx| store.select_range(ranges[9], cx)));
        let first = store.read_with(cx, |store, _| store.episodes().first().map(|e| e.number));
        assert_eq!(first, Some(901), "a picked chip shows its range");
    }

    #[gpui_kit::test]
    fn the_specials_sub_tab_appears_only_with_specials(cx: &mut TestAppContext) {
        let _runtime = runtime(cx);
        let media = MediaId::new(11);
        let episodes_only = EntryLibraryServices::new(
            media,
            vec![library_file("/s/01.mkv", media, "episode", Some(1))],
        );
        let store = open(cx, episodes_only);
        assert_eq!(
            store.read_with(cx, |store, _| store.sections()),
            [LibrarySection::Episodes],
            "no specials, no sub-tabs beyond Episodes"
        );
        assert!(
            store.read_with(cx, |store, _| store.ranges()).is_empty(),
            "a short media has no range chips"
        );

        let with_extras = EntryLibraryServices::new(
            media,
            vec![
                library_file("/s/01.mkv", media, "episode", Some(1)),
                library_file("/s/sp1.mkv", media, "special", Some(1)),
                library_file("/s/ncop.mkv", media, "ncop", Some(1)),
            ],
        );
        let store = open(cx, with_extras);
        assert_eq!(
            store.read_with(cx, |store, _| store.sections()),
            [
                LibrarySection::Episodes,
                LibrarySection::Specials,
                LibrarySection::NonCredit
            ]
        );
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.select_section(LibrarySection::Specials, cx);
            });
        });
        let specials = store.read_with(cx, |store, _| store.episodes());
        assert_eq!(specials.len(), 1);
        assert_eq!(specials[0].title, "Special 1");
        assert!(!specials[0].watched, "progress never covers specials");
    }
}
