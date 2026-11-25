//! Engine fakes for store and screen tests (#58, #61).
//!
//! Screens reach the engine only through ui-state stores, so their test
//! fakes live here with the stores rather than in `ui-screens`.

use std::sync::{Arc, Mutex};

use domain::{ExternalId, ListEntry, MediaId, MediaKind};
use engine::{BoxFuture, EngineError, EngineServices, ListedEntry, PlayRequest};
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

/// An engine fake that serves one Entry detail and records its saves. The
/// Home calls stay pending unless a test configures them.
#[derive(Debug, Clone)]
pub struct CannedServices {
    detail: MediaDetail,
    saved: Arc<Mutex<Vec<ListEntry>>>,
    /// Whether the Home catalog load fails; otherwise it stays pending.
    home_fails: bool,
    /// The Home collection, when a test serves one.
    collection: Option<Vec<ListedEntry>>,
}

impl CannedServices {
    /// Serves `detail` from the Entry store.
    #[must_use]
    pub fn new(detail: MediaDetail) -> Self {
        Self {
            detail,
            saved: Arc::new(Mutex::new(Vec::new())),
            home_fails: false,
            collection: None,
        }
    }

    /// Fails the Home catalog load, so its three sections show errors.
    #[must_use]
    pub fn home_fails(mut self) -> Self {
        self.home_fails = true;
        self
    }

    /// Serves `entries` as the Home collection, and no missed sequels.
    #[must_use]
    pub fn collection(mut self, entries: Vec<ListedEntry>) -> Self {
        self.collection = Some(entries);
        self
    }

    /// The entries handed to `save_entry`, oldest first.
    #[must_use]
    pub fn saved(&self) -> Vec<ListEntry> {
        self.saved
            .lock()
            .map_or_else(|_| Vec::new(), |saved| saved.clone())
    }
}

/// Returns a canned engine fake behind the service trait.
#[must_use]
pub fn canned_services(detail: MediaDetail) -> Arc<dyn EngineServices> {
    Arc::new(CannedServices::new(detail))
}

impl EngineServices for CannedServices {
    fn detail(&self, _anilist_id: u32) -> BoxFuture<Result<Load<MediaDetail>, EngineError>> {
        let detail = self.detail.clone();
        Box::pin(async move {
            Ok(Load {
                cached: metadata::Cached {
                    value: detail,
                    updated_at: std::time::SystemTime::now(),
                    offline: false,
                },
                refresh: None,
            })
        })
    }

    fn local_media(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<Option<MediaId>, EngineError>> {
        Box::pin(async move { Ok(Some(MediaId::new(1))) })
    }

    fn local_entry(&self, _media: MediaId) -> BoxFuture<Result<Option<ListEntry>, EngineError>> {
        Box::pin(async move { Ok(None) })
    }

    fn save_entry(&self, entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>> {
        let entry = entry.clone();
        let saved = Arc::clone(&self.saved);
        Box::pin(async move {
            if let Ok(mut saved) = saved.lock() {
                saved.push(entry.clone());
            }
            Ok(entry)
        })
    }

    fn mint(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<MediaId, EngineError>> {
        Box::pin(async move { Ok(MediaId::new(1)) })
    }

    fn home(&self) -> BoxFuture<Result<Load<HomeLoad>, EngineError>> {
        if self.home_fails {
            Box::pin(async { Err(EngineError::Metadata(metadata::Error::NotFound)) })
        } else {
            Box::pin(std::future::pending())
        }
    }

    fn collection(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListedEntry>, EngineError>> {
        match self.collection.clone() {
            Some(entries) => Box::pin(async move { Ok(entries) }),
            None => Box::pin(std::future::pending()),
        }
    }

    fn missed_sequels(
        &self,
        _kind: MediaKind,
    ) -> BoxFuture<Result<Vec<metadata::MediaSummary>, EngineError>> {
        if self.collection.is_some() {
            Box::pin(async { Ok(Vec::new()) })
        } else {
            Box::pin(std::future::pending())
        }
    }
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

    fn collection(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListedEntry>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn missed_sequels(
        &self,
        _kind: MediaKind,
    ) -> BoxFuture<Result<Vec<metadata::MediaSummary>, EngineError>> {
        Box::pin(std::future::pending())
    }
}

/// One playback command sent through a store, recorded for command tests.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlaybackCall {
    /// A play request.
    Play(PlayRequest),
    /// A stop request.
    Stop,
    /// A retry of a failed progress sync.
    Retry(MediaId, u32),
}

/// An engine fake that records playback commands; other calls stay pending.
#[derive(Debug, Default, Clone)]
pub struct PlaybackServices {
    calls: Arc<Mutex<Vec<PlaybackCall>>>,
}

impl PlaybackServices {
    /// The commands received, oldest first.
    #[must_use]
    pub fn calls(&self) -> Vec<PlaybackCall> {
        self.calls
            .lock()
            .map_or_else(|_| Vec::new(), |calls| calls.clone())
    }
}

impl EngineServices for PlaybackServices {
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

    fn collection(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListedEntry>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn missed_sequels(
        &self,
        _kind: MediaKind,
    ) -> BoxFuture<Result<Vec<metadata::MediaSummary>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn play(&self, request: &PlayRequest) -> BoxFuture<Result<(), EngineError>> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(PlaybackCall::Play(request.clone()));
        }
        Box::pin(async { Ok(()) })
    }

    fn stop(&self) -> BoxFuture<Result<(), EngineError>> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(PlaybackCall::Stop);
        }
        Box::pin(async { Ok(()) })
    }

    fn retry_sync(&self, media: MediaId, episode: u32) -> BoxFuture<Result<(), EngineError>> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(PlaybackCall::Retry(media, episode));
        }
        Box::pin(async { Ok(()) })
    }
}
