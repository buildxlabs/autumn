//! Presentational widgets that render plain view models.
//!
//! See ARCHITECTURE §2 and §5.

pub mod carousel;
pub mod episode_card;
pub mod fetch_status;
pub mod hero;
mod image_cache;
mod media_card;
pub mod playback_hud;
mod poster_grid;
mod rail;

pub use carousel::{
    CarouselWheel, MediaCarousel, PageDirection, WHEEL_PAGE_THRESHOLD, alt_wheel_delta, page,
};
pub use episode_card::{
    EPISODE_CARD_WIDTH, EPISODE_ROW_WIDTH, EpisodeActions, EpisodeCard, EpisodeCardView, EpisodeRow,
};
pub use image_cache::{LruImageCache, POSTER_IMAGE_CAPACITY};
pub use media_card::{
    MEDIA_CARD_HEIGHT, MEDIA_POSTER_HEIGHT, MediaCard, MediaCardView, MediaFormat,
};
pub use playback_hud::{
    ChapterMark, PLAYBACK_HUD_HEIGHT, PlaybackHud, PlaybackHudActions, PlaybackHudView,
};
pub use poster_grid::{POSTER_GAP, POSTER_WIDTH, PosterGrid, poster_columns};
pub use rail::{ACCOUNT_ID, RAIL_EXPANDED_WIDTH, RAIL_WIDTH, Rail, RailExpansion, RailItem};
