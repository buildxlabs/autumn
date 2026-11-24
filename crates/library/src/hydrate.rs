//! Turns parsed numbers into the matched media's own numbering.
//!
//! `AniZip` and filler lists are looked up once per matched media per scan,
//! never per file. A number beyond the media's episode count is retried on a
//! sequel named by the winning candidate before the file is left where it was.

use std::collections::{HashMap, HashSet};

use db::Db;
use domain::{ExternalId, MediaId, MediaKind};
use metadata::{Aging, AniZip, AnimeFillerList, Media};
use release_parser::EpisodeKind;

use crate::{Candidate, LibraryError, Match, ScannedFile};

/// The metadata clients a scan uses to hydrate its matches.
#[derive(Debug, Clone)]
pub(crate) struct Hydration {
    pub(crate) anizip: AniZip,
    pub(crate) filler: AnimeFillerList,
}

/// The numbers and flags a scan stores for one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HydratedFile {
    /// The season within the matched media.
    pub(crate) season: Option<u32>,
    /// The episode within that season.
    pub(crate) episode: Option<u32>,
    /// The storage form of the episode kind.
    pub(crate) kind: String,
    /// Whether this numbered episode is filler.
    pub(crate) filler: bool,
    /// Whether the number lies beyond the matched media and a sequel should be tried.
    pub(crate) sequel_needed: bool,
}

impl HydratedFile {
    fn ready(self) -> Self {
        Self {
            sequel_needed: false,
            ..self
        }
    }
}

/// One media's recorded metadata, fetched at most once per scan.
#[derive(Debug, Clone, Default)]
struct MediaData {
    anizip: Option<Media>,
    filler: HashSet<u32>,
}

/// Hydrated files and any media reassigned to a sequel.
pub(crate) struct Hydrated {
    pub(crate) files: HashMap<usize, HydratedFile>,
    pub(crate) media: HashMap<usize, MediaId>,
}

pub(crate) async fn hydrate_all(
    db: &Db,
    hydration: &Hydration,
    scanned: &[ScannedFile],
    wins: &HashMap<usize, Match>,
    media: &HashMap<usize, MediaId>,
    candidates: &[Candidate],
) -> Result<Hydrated, LibraryError> {
    let mut groups: HashMap<MediaId, Vec<usize>> = HashMap::new();
    for (index, media) in media {
        groups.entry(*media).or_default().push(*index);
    }
    let mut cache: HashMap<u32, MediaData> = HashMap::new();
    let mut files = HashMap::new();
    let mut sequel_media = HashMap::new();
    for indexes in groups.values() {
        let Some(first) = indexes.first().copied() else {
            continue;
        };
        let Some(found) = wins.get(&first) else {
            continue;
        };
        let Some(candidate) = candidates.get(found.candidate) else {
            continue;
        };
        let Some(id) = anilist_id(candidate) else {
            continue;
        };
        let titles = std::iter::once(&candidate.title)
            .chain(candidate.aliases.iter())
            .cloned()
            .collect::<Vec<_>>();
        let data = fetch(hydration, id, &titles, &mut cache).await?;
        for index in indexes {
            let Some(file) = scanned.get(*index) else {
                continue;
            };
            let mut hydrated = hydrate_file(file, &data);
            if hydrated.sequel_needed {
                for sequel in candidate.sequels() {
                    let Some(sequel_candidate) = candidates
                        .iter()
                        .find(|other| anilist_id(other) == Some(*sequel))
                    else {
                        continue;
                    };
                    let Some(sequel_id) = anilist_id(sequel_candidate) else {
                        continue;
                    };
                    let sequel_titles = std::iter::once(&sequel_candidate.title)
                        .chain(sequel_candidate.aliases.iter())
                        .cloned()
                        .collect::<Vec<_>>();
                    let sequel_data =
                        fetch(hydration, sequel_id, &sequel_titles, &mut cache).await?;
                    let retry = hydrate_file(file, &sequel_data);
                    if retry.sequel_needed {
                        continue;
                    }
                    let minted = db
                        .media()
                        .find_or_mint(MediaKind::Anime, &sequel_candidate.external_ids)
                        .await?;
                    sequel_media.insert(*index, minted.media);
                    hydrated = retry;
                    break;
                }
            }
            files.insert(*index, hydrated.ready());
        }
    }
    Ok(Hydrated {
        files,
        media: sequel_media,
    })
}

/// The AniList id a candidate carries for its metadata lookups.
fn anilist_id(candidate: &Candidate) -> Option<u32> {
    candidate.external_ids.iter().find_map(|id| match id {
        ExternalId::AniList(id) => Some(*id),
        _ => None,
    })
}

/// Fetches one media's `AniZip` and filler data, using the scan-wide cache.
async fn fetch(
    hydration: &Hydration,
    anilist_id: u32,
    titles: &[String],
    cache: &mut HashMap<u32, MediaData>,
) -> Result<MediaData, LibraryError> {
    if let Some(data) = cache.get(&anilist_id) {
        return Ok(data.clone());
    }
    let anizip = hydration.anizip.media(anilist_id, Aging::Finished).await?;
    let filler = hydration
        .filler
        .filler_episodes(titles)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|number| number.parse().ok())
        .collect();
    let data = MediaData { anizip, filler };
    cache.insert(anilist_id, data.clone());
    Ok(data)
}

/// Applies one media's numbering to a parsed file.
fn hydrate_file(file: &ScannedFile, data: &MediaData) -> HydratedFile {
    let kind = crate::scanner::kind_name(file.release.kind).to_owned();
    let mut result = HydratedFile {
        season: file.release.season,
        episode: file.release.episode,
        kind,
        filler: false,
        sequel_needed: false,
    };
    if file.release.kind != EpisodeKind::Episode {
        return result;
    }
    let Some(number) = file.release.episode else {
        return result;
    };
    let entry = data.anizip.as_ref().and_then(|media| {
        media.episodes.values().find(|entry| {
            if let Some(season) = file.release.season {
                entry.season_number == Some(season) && entry.season_episode == Some(number)
            } else {
                entry.absolute_number == Some(number) || entry.number() == Some(number)
            }
        })
    });
    if let Some(entry) = entry {
        if file.release.season.is_none() {
            result.season = entry.season_number;
            result.episode = entry.season_episode;
        }
        let absolute = entry.absolute_number.unwrap_or(number);
        result.filler = data.filler.contains(&absolute);
        return result;
    }
    if file.release.season.is_none()
        && data
            .anizip
            .as_ref()
            .and_then(|media| media.episode_count)
            .is_some_and(|count| number > count)
    {
        result.sequel_needed = true;
    }
    result.filler = data.filler.contains(&number);
    result
}
