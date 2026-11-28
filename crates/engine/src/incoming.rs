//! Incoming torrents: one pipeline for every user-brought torrent (#223,
//! #209 Q1-Q6, ADR-0009).
//!
//! Four entry points (magnet paste, `/magnet`, `/droptorrent`, drag-and-drop
//! of a `.torrent`) feed the same validation; a magnet is added for metadata
//! first; **match review** attaches a Media and Episode before anything
//! streams. Nothing here auto-commits a match: this module produces the
//! ranked suggestions, the caller confirms.
//!
//! No new crate edges: `engine` already reaches `torrent`, `search`,
//! `library`, `domain`, `metadata`.

use domain::{ExternalId, InfoHash, MediaId, TorrentLink, TorrentOrigin};
use library::{Candidate, Matcher, normalize};
use torrent::{TorrentFile, TorrentMeta, TorrentSource};

use crate::TorrentStreams;

/// A validated incoming magnet link (#209 Q6).
///
/// v1 infohash only: 40 hex or 32 base32, normalized to hex. v2-only
/// magnets are unsupported in Phase 3. Trackers are kept and passed to the
/// session (#217 known gap).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MagnetLink {
    /// The v1 infohash.
    pub info_hash: InfoHash,
    /// The magnet's `dn` display name, when present.
    pub name: Option<String>,
    /// Tracker URLs from `tr` parameters, in order.
    pub trackers: Vec<String>,
    /// The original `magnet:?…` string, with trackers intact.
    pub original: String,
}

impl MagnetLink {
    /// The source handed to the torrent session, trackers kept.
    #[must_use]
    pub fn source(&self) -> TorrentSource {
        TorrentSource::Magnet(self.original.clone())
    }
}

/// A validated incoming `.torrent` file (#209 Q6).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TorrentFileSource {
    /// The v1 infohash.
    pub info_hash: InfoHash,
    /// The metainfo's suggested name, when present.
    pub name: Option<String>,
    /// The raw `.torrent` bytes, handed to the session.
    pub bytes: Vec<u8>,
    /// Every file; at least one video, padding excluded.
    pub files: Vec<IncomingFile>,
    /// Tracker URLs from `announce`/`announce-list`, in order.
    pub trackers: Vec<String>,
}

impl TorrentFileSource {
    /// The source handed to the torrent session.
    #[must_use]
    pub fn source(&self) -> TorrentSource {
        TorrentSource::File(self.bytes.clone())
    }
}

/// One file inside an incoming torrent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IncomingFile {
    /// The index `StreamSource::Torrent` selects it by.
    pub index: usize,
    /// `/`-separated path inside the torrent.
    pub path: String,
    /// Size in bytes.
    pub len: u64,
    /// Whether it is a video a player can open (not padding, not extra).
    pub video: bool,
}

impl IncomingFile {
    /// A file at `index` with `path`, `len` bytes, `video` flag.
    #[must_use]
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "a single video flag mirrors the file's one boolean fact"
    )]
    pub fn new(index: usize, path: impl Into<String>, len: u64, video: bool) -> Self {
        Self {
            index,
            path: path.into(),
            len,
            video,
        }
    }
}

impl From<&TorrentFile> for IncomingFile {
    fn from(file: &TorrentFile) -> Self {
        Self {
            index: file.index,
            path: file.path.clone(),
            len: file.len,
            video: search::is_video_path(&file.path),
        }
    }
}

/// A validated incoming torrent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IncomingSource {
    /// A magnet link; metadata arrives from peers.
    Magnet(MagnetLink),
    /// A `.torrent` file; the file list is known up front.
    TorrentFile(TorrentFileSource),
}

impl IncomingSource {
    /// The v1 infohash.
    #[must_use]
    pub fn info_hash(&self) -> InfoHash {
        match self {
            Self::Magnet(link) => link.info_hash,
            Self::TorrentFile(file) => file.info_hash,
        }
    }

    /// The display name, when known.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Magnet(link) => link.name.as_deref(),
            Self::TorrentFile(file) => file.name.as_deref(),
        }
    }

    /// The source handed to the torrent session, trackers kept.
    #[must_use]
    pub fn source(&self) -> TorrentSource {
        match self {
            Self::Magnet(link) => link.source(),
            Self::TorrentFile(file) => file.source(),
        }
    }

    /// Known files; empty for a magnet until metadata arrives.
    #[must_use]
    pub fn files(&self) -> &[IncomingFile] {
        match self {
            Self::Magnet(_) => &[],
            Self::TorrentFile(file) => &file.files,
        }
    }

    /// The title strings release-parsing runs on: the torrent name, plus
    /// every `.torrent` file name (#209 Q2).
    #[must_use]
    pub fn title_sources(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Some(name) = self.name() {
            names.push(name.to_owned());
        }
        names.extend(self.files().iter().map(|file| {
            file.path
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&file.path)
                .to_owned()
        }));
        names
    }
}

/// Why an incoming torrent or magnet was rejected inline (#209 Q6): no
/// session is created.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum IncomingError {
    /// No `magnet:?`, missing/invalid infohash, or otherwise malformed.
    #[error("not a valid magnet link")]
    MalformedMagnet,
    /// A v2-only magnet (`urn:btmh`); v1 infohash required in Phase 3.
    #[error("v2-only torrents are not supported")]
    UnsupportedV2,
    /// Unreadable `.torrent`, or invalid metainfo.
    #[error("could not read .torrent")]
    UnreadableTorrent,
    /// A readable `.torrent` with no video files.
    #[error("no video files found")]
    NoVideoFiles,
    /// A torrent with this infohash is already in the session; jump to its
    /// existing row instead of adding a second copy.
    #[error("this torrent is already in the list")]
    Duplicate(InfoHash),
    /// The torrent session failed.
    #[error("the torrent session failed: {0}")]
    Torrent(String),
}

/// Validates and parses a pasted `magnet:` link (#209 Q6).
///
/// `magnet:?` prefix required; a **v1 infohash** (`xt=urn:btih:` + 40 hex or
/// 32 base32, normalized to hex); `dn` name and `tr` trackers preserved; a
/// v2-only magnet (`urn:btmh`, no v1) is `UnsupportedV2`. Malformed links
/// are rejected; nothing touches the session.
///
/// # Errors
///
/// [`IncomingError::MalformedMagnet`], [`IncomingError::UnsupportedV2`].
pub fn parse_magnet(text: &str) -> Result<MagnetLink, IncomingError> {
    let text = text.trim();
    let query = text
        .strip_prefix("magnet:?")
        .ok_or(IncomingError::MalformedMagnet)?;

    let mut info_hash = None;
    let mut name = None;
    let mut trackers = Vec::new();
    let mut saw_v2 = false;
    for field in query.split('&') {
        if field.is_empty() {
            continue;
        }
        let (key, value) = field.split_once('=').unwrap_or((field, ""));
        let key = percent_decode(key);
        let value = percent_decode(value);
        match key.to_ascii_lowercase().as_str() {
            "xt" => {
                if let Some(hash) = value.strip_prefix("urn:btih:") {
                    info_hash = Some(parse_infohash(hash)?);
                } else if value.starts_with("urn:btmh:") {
                    saw_v2 = true;
                }
            }
            "dn" => name = Some(value).filter(|name| !name.is_empty()),
            "tr" if !value.is_empty() => trackers.push(value),
            _ => {}
        }
    }

    match info_hash {
        Some(info_hash) => Ok(MagnetLink {
            info_hash,
            name,
            trackers,
            original: text.to_owned(),
        }),
        None if saw_v2 => Err(IncomingError::UnsupportedV2),
        None => Err(IncomingError::MalformedMagnet),
    }
}

/// A v1 infohash: 40 hex or 32 base32, normalized to hex (#209 Q6).
fn parse_infohash(hash: &str) -> Result<InfoHash, IncomingError> {
    if hash.len() == 40 {
        return hash.parse().map_err(|_| IncomingError::MalformedMagnet);
    }
    if hash.len() == 32 {
        return InfoHash::from_base32(hash).map_err(|_| IncomingError::MalformedMagnet);
    }
    Err(IncomingError::MalformedMagnet)
}

/// Validates `.torrent` bytes (#209 Q6): readable v1 metainfo with at least
/// one video file. Rejected inline; no session is created.
///
/// # Errors
///
/// [`IncomingError::UnreadableTorrent`], [`IncomingError::NoVideoFiles`].
pub fn parse_torrent_file(bytes: &[u8]) -> Result<TorrentFileSource, IncomingError> {
    let meta: TorrentMeta =
        torrent::parse_torrent(bytes).map_err(|_| IncomingError::UnreadableTorrent)?;
    let files: Vec<IncomingFile> = meta.files.iter().map(IncomingFile::from).collect();
    if !files.iter().any(|file| file.video) {
        return Err(IncomingError::NoVideoFiles);
    }
    Ok(TorrentFileSource {
        info_hash: meta.info_hash,
        name: meta.name,
        bytes: bytes.to_vec(),
        files,
        trackers: meta.trackers,
    })
}

/// One incoming title to try, already release-parsed.
///
/// Parse the torrent name and every `.torrent` file name with
/// `release-parser`, then run each `title` through `library::matcher`
/// (#209 Q2). Blank titles are dropped.
#[must_use]
pub fn match_titles(source: &IncomingSource) -> Vec<String> {
    search::match_titles(&source.title_sources())
}

/// The episode pre-selected for review (#209 Q3), parsed from the torrent
/// name first, then a file name.
#[must_use]
pub fn episode_suggestion(source: &IncomingSource) -> Option<u32> {
    search::episode_suggestion(&source.title_sources())
}

/// A media a torrent can match to, ordered for matching (#209 Q2): active
/// platform entries first, then AniList catalog results.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MatchCandidate {
    /// Autumn's media identity, when this candidate is already known locally.
    pub media: Option<MediaId>,
    /// The catalog title.
    pub title: String,
    /// Other titles (romaji/english/native, synonyms, per-language titles).
    pub aliases: Vec<String>,
    /// Every external id known for the media.
    pub external_ids: Vec<ExternalId>,
}

impl MatchCandidate {
    /// A candidate titled `title` with no aliases or ids.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            media: None,
            title: title.into(),
            aliases: Vec::new(),
            external_ids: Vec::new(),
        }
    }

    fn as_matcher_candidate(&self) -> Candidate {
        let mut candidate = Candidate::new(self.title.clone(), self.external_ids.clone());
        for alias in &self.aliases {
            candidate = candidate.with_alias(alias.clone());
        }
        candidate
    }
}

impl From<Candidate> for MatchCandidate {
    fn from(candidate: Candidate) -> Self {
        Self {
            media: None,
            title: candidate.title.clone(),
            aliases: candidate.aliases.clone(),
            external_ids: candidate.external_ids,
        }
    }
}

/// One ranked match.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct MatchSuggestion {
    /// The media it matched.
    pub candidate: MatchCandidate,
    /// The matcher score, strictly above `library::matcher::DEFAULT_THRESHOLD`.
    pub score: f64,
}

/// The match-review outcome (#209 Q3, ADR-0009): never auto-commits.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum MatchReview {
    /// Exactly one candidate above the threshold: preselected.
    Decisive(MatchSuggestion),
    /// Two or more above the threshold: a ranked list.
    Ambiguous(Vec<MatchSuggestion>),
    /// Nothing above the threshold (or no parseable title): manual media +
    /// episode pick.
    Failed,
}

/// Matches `titles` through `library::matcher` (threshold 0.5, strictly
/// above) against `candidates`, in the given order (active platform first,
/// then AniList). Every `title` is tried; the best score per candidate
/// wins. Ranked by score, ties keep input order.
#[must_use]
pub fn match_review(titles: &[String], candidates: &[MatchCandidate]) -> MatchReview {
    let matcher = Matcher::default();
    let matcher_candidates: Vec<Candidate> = candidates
        .iter()
        .map(MatchCandidate::as_matcher_candidate)
        .collect();

    let mut suggestions: Vec<MatchSuggestion> = candidates
        .iter()
        .zip(matcher_candidates.iter())
        .map(|(candidate, matcher_candidate)| {
            let score = titles
                .iter()
                .map(|title| matcher.score(&normalize(title), matcher_candidate))
                .fold(0.0_f64, f64::max);
            (candidate, score)
        })
        // Strictly above the threshold, as `Matcher::best`.
        .filter(|(_, score)| *score > library::DEFAULT_THRESHOLD)
        .map(|(candidate, score)| MatchSuggestion {
            candidate: candidate.clone(),
            score,
        })
        .collect();

    // Ranked; a stable sort keeps input order on ties (platform first).
    suggestions.sort_by(|a, b| b.score.total_cmp(&a.score));

    match suggestions.len() {
        0 => MatchReview::Failed,
        1 => match suggestions.into_iter().next() {
            Some(only) => MatchReview::Decisive(only),
            None => MatchReview::Failed,
        },
        _ => MatchReview::Ambiguous(suggestions),
    }
}

/// The file-level pick reused from #181: which `file_index` holds `episode`
/// of the media, skipping NCOP/NCED/samples/extras, accepting seasonal and
/// absolute numbering. `season` is the media's season (from the torrent or
/// media title; 1 otherwise). `None` when no video file matches.
#[must_use]
pub fn pick_episode_file(files: &[IncomingFile], season: u32, episode: u32) -> Option<usize> {
    let target = search::EpisodeTarget::new(season, episode);
    search::pick_file(
        files
            .iter()
            .filter(|file| file.video)
            .map(|file| (file.index, file.path.as_str())),
        &target,
    )
}

/// What [`add_incoming`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IncomingAdded {
    /// The v1 infohash now in the session.
    pub info_hash: InfoHash,
    /// Whether a torrent with this infohash was already in the session
    /// (#209 Q6 duplicate); no second copy was added.
    pub duplicate: bool,
    /// The torrent's files once metadata is known.
    pub files: Vec<IncomingFile>,
    /// The display name, when known.
    pub name: Option<String>,
}

/// Adds an incoming torrent to the session for metadata first (#209 Q4,
/// #223): paused, no file selected, origin `TorrentOrigin::Incoming`, no
/// match committed. A **duplicate infohash** returns the existing torrent's
/// state instead of adding a second copy. A magnet keeps its own trackers.
///
/// The caller shows "Finding files…"; **Cancel = drop** via
/// [`cancel_incoming`]. Once a file is chosen the caller selects it
/// (`update_only_files`) and resumes, or streams `StreamSource::Torrent`.
///
/// # Errors
///
/// [`IncomingError::Torrent`] when the session cannot read the metadata.
pub async fn add_incoming(
    streams: &TorrentStreams,
    source: &IncomingSource,
) -> Result<IncomingAdded, IncomingError> {
    let session = streams
        .session()
        .await
        .map_err(|error| IncomingError::Torrent(error.to_string()))?;
    let info_hash = source.info_hash();

    let already = session
        .list()
        .map_err(|error| IncomingError::Torrent(error.to_string()))?
        .iter()
        .any(|entry| entry.info_hash == info_hash);

    if !already {
        session
            .add_metadata(source.source())
            .await
            .map_err(|error| IncomingError::Torrent(error.to_string()))?;
    }
    // Origin for the Torrent List (#207, #223); no match is committed yet.
    // A duplicate keeps its existing registration (and any committed link).
    if !already {
        streams.register(info_hash, TorrentOrigin::Incoming, None);
    }

    let files = session
        .list_files(source.source())
        .await
        .map_err(|error| IncomingError::Torrent(error.to_string()))?;
    let name = source.name().map(str::to_owned);
    Ok(IncomingAdded {
        info_hash,
        duplicate: already,
        files: files.iter().map(IncomingFile::from).collect(),
        name,
    })
}

/// Cancel an incoming torrent before its match is confirmed (#209 Q4,
/// #180): drop it and delete partials. Best-effort; a torrent never added
/// is a no-op.
pub async fn cancel_incoming(streams: &TorrentStreams, info_hash: InfoHash) {
    let Ok(session) = streams.session().await else {
        return;
    };
    let listed = session
        .list()
        .is_ok_and(|entries| entries.iter().any(|entry| entry.info_hash == info_hash));
    if !listed {
        return;
    }
    let _ = streams.remove(info_hash, async {}).await;
}

/// Commits a confirmed match (#209 Q3): registers origin + (media, episode)
/// on the Torrent List. Never called automatically; the review step calls
/// it once the user picks.
pub fn commit_incoming(streams: &TorrentStreams, info_hash: InfoHash, link: Option<TorrentLink>) {
    streams.register(info_hash, TorrentOrigin::Incoming, link);
}

/// Percent-decodes a magnet query key/value (`+` is not a space in magnets;
/// `%20` is).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%'
            && let (Some(high), Some(low)) = (bytes.get(index + 1), bytes.get(index + 2))
            && let (Some(high), Some(low)) = (hex(*high), hex(*low))
        {
            out.push(high << 4 | low);
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal valid v1 single-file `.torrent`.
    fn torrent_bytes(name: &str, files: &[(&str, u64)]) -> Vec<u8> {
        fn ben(s: &str) -> Vec<u8> {
            let mut out = format!("{}:", s.len()).into_bytes();
            out.extend_from_slice(s.as_bytes());
            out
        }
        fn int(n: u64) -> Vec<u8> {
            format!("i{n}e").into_bytes()
        }

        let mut info = Vec::new();
        info.push(b'd');
        if files.len() == 1 && files.first().map(|(path, _)| *path) == Some(name) {
            info.extend(ben("length"));
            info.extend(int(files.first().map_or(0, |(_, len)| *len)));
        } else {
            info.extend(ben("files"));
            info.push(b'l');
            for (path, len) in files {
                info.push(b'd');
                info.extend(ben("length"));
                info.extend(int(*len));
                info.extend(ben("path"));
                info.push(b'l');
                info.extend(ben(path));
                info.push(b'e');
                info.push(b'e');
            }
            info.push(b'e');
        }
        info.extend(ben("name"));
        info.extend(ben(name));
        info.extend(ben("piece length"));
        info.extend(int(16384));
        info.extend(ben("pieces"));
        info.extend(ben("01234567890123456789"));
        info.push(b'e');

        let mut out = Vec::new();
        out.push(b'd');
        out.extend(ben("announce"));
        out.extend(ben("http://tracker.example/announce"));
        out.extend(ben("info"));
        out.extend(info);
        out.push(b'e');
        out
    }

    // ---------- magnet validation (#209 Q6) ----------

    const HEX: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
    const BASE32: &str = "VFFI7ZOMWGN2MHCMBBZ5HEPJQ6MC7O6T";

    #[test]
    fn a_valid_v1_magnet_parses_with_name_and_trackers() {
        let link = parse_magnet(&format!(
            "magnet:?xt=urn:btih:{HEX}&dn=My%20Show%20S01E05&tr=udp%3A%2F%2Ftracker%3A80&tr=https%3A%2F%2Ft.example%2Fannounce"
        ))
        .unwrap();
        assert_eq!(link.info_hash.to_string(), HEX);
        assert_eq!(link.name.as_deref(), Some("My Show S01E05"));
        assert_eq!(
            link.trackers,
            vec![
                "udp://tracker:80".to_owned(),
                "https://t.example/announce".to_owned()
            ]
        );
        assert!(link.original.contains("tr="), "trackers kept in original");
        assert!(matches!(link.source(), TorrentSource::Magnet(_)));
    }

    #[test]
    fn a_base32_infohash_is_normalized_to_hex() {
        let link = parse_magnet(&format!("magnet:?xt=urn:btih:{BASE32}")).unwrap();
        assert_eq!(link.info_hash.to_string(), HEX);
    }

    #[test]
    fn base32_is_case_insensitive() {
        let lower = BASE32.to_ascii_lowercase();
        assert!(parse_magnet(&format!("magnet:?xt=urn:btih:{lower}")).is_ok());
    }

    #[test]
    fn malformed_magnets_are_rejected_inline() {
        let cases = [
            "",
            "not a magnet",
            "magnet:",                                  // no `?`
            "magnet:?",                                 // no params
            "magnet:?xt=urn:btih:",                     // missing infohash
            "magnet:?xt=urn:btih:zz",                   // invalid infohash
            "magnet:?xt=urn:btih:not-a-valid-infohash", // not a v1 infohash
            "magnet:?dn=No%20hash",                     // no xt
            "http://example.com/x.torrent",             // not a magnet
        ];
        for case in cases {
            assert_eq!(
                parse_magnet(case),
                Err(IncomingError::MalformedMagnet),
                "{case:?}"
            );
        }
    }

    #[test]
    fn v2_only_magnets_are_unsupported() {
        let result = parse_magnet(
            "magnet:?xt=urn:btmh:1220caf1e1c3f9d0a94a8fe5ccb19ba61c4c0873d391e987982fbbd3a94a8fe5",
        );
        assert_eq!(result, Err(IncomingError::UnsupportedV2));
    }

    #[test]
    fn a_v1_infohash_alongside_v2_still_works() {
        let link = parse_magnet(&format!("magnet:?xt=urn:btmh:1220abc&xt=urn:btih:{HEX}")).unwrap();
        assert_eq!(link.info_hash.to_string(), HEX);
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert!(
            parse_magnet(&format!(
                "  magnet:?xt=urn:btih:{HEX}  
"
            ))
            .is_ok()
        );
    }

    // ---------- `.torrent` validation (#209 Q6) ----------

    #[test]
    fn a_video_torrent_parses() {
        let bytes = torrent_bytes(
            "[Group] My Show - 05 [1080p].mkv",
            &[("[Group] My Show - 05 [1080p].mkv", 1024)],
        );
        let source = parse_torrent_file(&bytes).unwrap();
        assert_eq!(source.info_hash.to_string().len(), 40);
        assert_eq!(
            source.name.as_deref(),
            Some("[Group] My Show - 05 [1080p].mkv")
        );
        assert_eq!(
            source.trackers,
            vec!["http://tracker.example/announce".to_owned()]
        );
        assert_eq!(source.files.len(), 1);
        assert!(source.files.first().unwrap().video);
        assert!(matches!(source.source(), TorrentSource::File(_)));
    }

    #[test]
    fn a_batch_torrent_lists_every_video() {
        let bytes = torrent_bytes(
            "[Group] My Show (01-12)",
            &[
                ("[Group] My Show - 01.mkv", 100),
                ("[Group] My Show - 02.mkv", 100),
                ("NCOP.mkv", 100),
                ("readme.txt", 10),
            ],
        );
        let source = parse_torrent_file(&bytes).unwrap();
        assert_eq!(source.files.len(), 4);
        assert_eq!(source.files.iter().filter(|f| f.video).count(), 3);
    }

    #[test]
    fn an_unreadable_torrent_is_rejected_inline() {
        assert_eq!(
            parse_torrent_file(b"this is not a torrent"),
            Err(IncomingError::UnreadableTorrent)
        );
        assert_eq!(
            parse_torrent_file(&[]),
            Err(IncomingError::UnreadableTorrent)
        );
    }

    #[test]
    fn a_non_video_torrent_is_rejected_inline() {
        let bytes = torrent_bytes("Some.Release", &[("notes.txt", 10), ("data.bin", 20)]);
        assert_eq!(parse_torrent_file(&bytes), Err(IncomingError::NoVideoFiles));
    }

    // ---------- release-parser title + episode (#209 Q2-Q3) ----------

    #[test]
    fn match_titles_parses_name_and_files() {
        let bytes = torrent_bytes(
            "[Group] Sousou no Frieren - 05 [1080p]",
            &[("[Group] Sousou no Frieren - 05 [1080p].mkv", 100)],
        );
        let source = IncomingSource::TorrentFile(parse_torrent_file(&bytes).unwrap());
        let titles = match_titles(&source);
        assert!(
            titles.iter().any(|t| t == "Sousou no Frieren"),
            "{titles:?}"
        );
    }

    #[test]
    fn episode_suggestion_reads_the_release_number() {
        let source = IncomingSource::Magnet(MagnetLink {
            info_hash: HEX.parse().unwrap(),
            name: Some("[Group] Sousou no Frieren - 05 [1080p]".to_owned()),
            trackers: vec![],
            original: String::new(),
        });
        assert_eq!(episode_suggestion(&source), Some(5));
    }

    // ---------- match review (#209 Q2-Q3, ADR-0009) ----------

    fn candidate(title: &str) -> MatchCandidate {
        MatchCandidate::new(title)
    }

    fn aliased(title: &str, aliases: &[&str]) -> MatchCandidate {
        MatchCandidate {
            aliases: aliases.iter().map(|a| (*a).to_owned()).collect(),
            ..MatchCandidate::new(title)
        }
    }

    #[test]
    fn decisive_match_is_preselected() {
        let titles = vec!["Sousou no Frieren".to_owned()];
        let review = match_review(
            &titles,
            &[candidate("Sousou no Frieren"), candidate("One Piece")],
        );
        let suggestion = match review {
            MatchReview::Decisive(suggestion) => Some(suggestion),
            _ => None,
        }
        .expect("expected a decisive match");
        assert_eq!(suggestion.candidate.title, "Sousou no Frieren");
        assert!(suggestion.score > library::DEFAULT_THRESHOLD);
    }

    #[test]
    fn aliases_can_win() {
        let titles = vec!["Frieren: Beyond Journey's End".to_owned()];
        let review = match_review(
            &titles,
            &[aliased(
                "Sousou no Frieren",
                &["Frieren: Beyond Journey's End"],
            )],
        );
        assert!(matches!(review, MatchReview::Decisive(_)), "{review:?}");
    }

    #[test]
    fn ambiguous_when_two_are_above_threshold_ranked() {
        let titles = vec!["My Show".to_owned()];
        let review = match_review(
            &titles,
            &[
                candidate("My Show"),
                candidate("My Show"),
                candidate("Other"),
            ],
        );
        let ranked = match review {
            MatchReview::Ambiguous(ranked) => ranked,
            _ => Vec::new(),
        };
        assert_eq!(ranked.len(), 2, "expected an ambiguous match");
        assert!(ranked.first().unwrap().score >= ranked.get(1).unwrap().score);
    }

    #[test]
    fn failed_when_nothing_is_above_threshold() {
        let titles = vec!["Totally Unrelated Release".to_owned()];
        let review = match_review(&titles, &[candidate("Sousou no Frieren")]);
        assert_eq!(review, MatchReview::Failed);
    }

    #[test]
    fn failed_when_no_titles_or_no_candidates() {
        assert_eq!(match_review(&[], &[candidate("X")]), MatchReview::Failed);
        assert_eq!(match_review(&["X".to_owned()], &[]), MatchReview::Failed);
    }

    #[test]
    fn active_platform_wins_a_tied_score() {
        let titles = vec!["My Show".to_owned()];
        let mut local = candidate("My Show");
        local.external_ids = vec![ExternalId::AniList(1)];
        let review = match_review(&titles, &[local, candidate("My Show")]);
        let ranked = match review {
            MatchReview::Ambiguous(ranked) => ranked,
            _ => Vec::new(),
        };
        assert!(!ranked.is_empty(), "expected an ambiguous match");
        assert_eq!(
            ranked.first().unwrap().candidate.external_ids,
            vec![ExternalId::AniList(1)],
            "first-listed (active platform) wins a tie"
        );
    }

    // ---------- file pick reused from #181 ----------

    #[test]
    fn pick_episode_file_reuses_181_mapping() {
        let files = vec![
            IncomingFile {
                index: 0,
                path: "NCOP.mkv".to_owned(),
                len: 1,
                video: true,
            },
            IncomingFile {
                index: 1,
                path: "[Group] My Show - 04.mkv".to_owned(),
                len: 1,
                video: true,
            },
            IncomingFile {
                index: 2,
                path: "[Group] My Show - 05.mkv".to_owned(),
                len: 1,
                video: true,
            },
            IncomingFile {
                index: 3,
                path: "sample/[Group] My Show - 05 sample.mkv".to_owned(),
                len: 1,
                video: true,
            },
        ];
        assert_eq!(pick_episode_file(&files, 1, 5), Some(2));
        assert_eq!(pick_episode_file(&files, 1, 4), Some(1));
        assert_eq!(pick_episode_file(&files, 1, 99), None);
    }
}
