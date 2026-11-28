//! Torrent indexer search and episode matching.
//!
//! Two indexers, `Nyaa` and `AnimeTosho`, share one plain-data [`Torrent`] result.
//! Each runs in an isolated lane: a failed lane is reported in
//! [`SearchOutcome::failures`] while the other lanes still return results.
//! Query construction, the request budget and episode matching are pure and
//! fixture-testable; HTTP goes through `net`'s shared client. See ADR 0007,
//! ADR 0008 and ADR 0011, and ARCHITECTURE §2–§3.

pub mod animetosho;
mod availability;
mod error;
mod file_pick;
mod matching;
pub mod nyaa;
mod query;
mod rank;
mod result;
pub mod search;
mod util;

pub use availability::{
    Availability, AvailabilityTracker, FRESH_WINDOW, FakeWallClock, MAX_IN_FLIGHT,
    RECHECK_INTERVAL, SystemWallClock, WallClock, parse_air_date,
};
pub use error::Error;
pub use file_pick::{is_playable_file, is_video_path, pick_file};
pub use matching::{
    EpisodeTarget, episode_suggestion, match_rejection, match_titles, matches, matches_torrent,
};
pub use query::{
    MAX_REQUESTS_PER_INDEXER, MAX_TITLES, Query, QueryPlan, SearchRequest, clean_title,
    episode_season, plan,
};
pub use rank::{Ranked, rank, rank_with_status};
pub use result::{Indexer, LaneFailure, SearchOutcome, Torrent};
pub use search::{
    Config, FakeClock, LaneFilter, NetTransport, Searcher, SystemClock, Transport, query_url,
};
pub use util::trackers_from_magnet;
