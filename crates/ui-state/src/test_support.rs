//! Engine fakes for store and screen tests (#58, #61).
//!
//! Screens reach the engine only through ui-state stores, so their test
//! fakes live here with the stores rather than in `ui-screens`.

use std::sync::Arc;

use domain::{ExternalId, ListEntry, MediaId, MediaKind};
use engine::{BoxFuture, EngineError, EngineServices};
use metadata::{HomeLoad, Load, MediaDetail};

/// An engine service whose calls never resolve, for tests that only need a
/// store to construct without loading data.
#[derive(Debug)]
pub struct PendingServices;

impl Default for PendingServices {
    fn default() -> Self {
        Self
    }
}

/// Returns a pending engine fake behind the service trait.
#[must_use]
pub fn pending_services() -> Arc<dyn EngineServices> {
    Arc::new(PendingServices)
}

impl EngineServices for PendingServices {
    fn detail(&self, _anilist_id: u32) -> BoxFuture<Result<Load<MediaDetail>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn local_media(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<Option<MediaId>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn local_entry(&self, _media: MediaId) -> BoxFuture<Result<Option<ListEntry>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn save_entry(&self, _entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn mint(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<MediaId, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn home(&self) -> BoxFuture<Result<Load<HomeLoad>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn collection(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListEntry>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn missed_sequels(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListEntry>, EngineError>> {
        Box::pin(std::future::pending())
    }
}
