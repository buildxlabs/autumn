//! Torrent indexer search and episode matching.
//!
//! Two indexers, `Nyaa` and `AnimeTosho`, share one plain-data [`Torrent`] result.
//! Query construction, the request budget and episode matching are pure and
//! fixture-testable; HTTP goes through `net`'s shared client. See ADR 0007
//! and ADR 0008, and ARCHITECTURE §2–§3.

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
pub use file_pick::pick_file;
pub use matching::{EpisodeTarget, matches, matches_torrent};
pub use query::{
    MAX_REQUESTS_PER_INDEXER, MAX_TITLES, Query, QueryPlan, SearchRequest, clean_title,
    episode_season, plan,
};
pub use rank::rank;
pub use result::{Indexer, Torrent};
pub use search::{Config, FakeClock, NetTransport, Searcher, SystemClock, Transport, query_url};
