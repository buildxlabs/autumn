//! The one active tracking platform (ADR 0004).
//!
//! Both platforms are kept in this enum because `Platform` is not object
//! safe: its async methods return `impl Future`. Local stays loaded while
//! AniList is active, but nothing writes to it; signing out simply makes it
//! active again, with every entry exactly as it was left.

use domain::{ListEntry, MediaId, MediaKind};

use crate::{AniListPlatform, Collection, Local, Platform, PlatformError};

/// Which platform reads and writes list entries right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the set of tracking platforms is fixed for Phase 1"
)]
pub enum ActiveKind {
    /// The account-free local platform.
    Local,
    /// The connected AniList account.
    AniList,
}

/// The active platform and its dormant counterpart.
#[derive(Debug, Clone)]
pub struct ActivePlatform {
    local: Local,
    anilist: Option<AniListPlatform>,
    active: ActiveKind,
}

impl ActivePlatform {
    /// Starts with the Local platform active.
    #[must_use]
    pub fn new(local: Local) -> Self {
        Self {
            local,
            anilist: None,
            active: ActiveKind::Local,
        }
    }

    /// Connects AniList and makes it active. Local entries are kept, not
    /// deleted or updated (ADR 0004).
    pub fn connect(&mut self, anilist: AniListPlatform) {
        self.anilist = Some(anilist);
        self.active = ActiveKind::AniList;
    }

    /// Signs out of AniList and restores Local as the active platform. The
    /// token and dormant account entries are dropped; Local is untouched.
    pub fn sign_out(&mut self) {
        self.anilist = None;
        self.active = ActiveKind::Local;
    }

    /// Which platform is active.
    #[must_use]
    pub fn active(&self) -> ActiveKind {
        self.active
    }

    /// The dormant Local platform, for the reviewed list transfer.
    #[must_use]
    pub fn local(&self) -> &Local {
        &self.local
    }
}

impl Platform for ActivePlatform {
    async fn collection(&self, kind: MediaKind) -> Result<Collection, PlatformError> {
        match self.active {
            ActiveKind::Local => self.local.collection(kind).await,
            ActiveKind::AniList => match &self.anilist {
                Some(anilist) => anilist.collection(kind).await,
                None => Err(PlatformError::NotConnected),
            },
        }
    }

    async fn save_entry(&self, entry: &ListEntry) -> Result<ListEntry, PlatformError> {
        match self.active {
            ActiveKind::Local => self.local.save_entry(entry).await,
            ActiveKind::AniList => match &self.anilist {
                Some(anilist) => anilist.save_entry(entry).await,
                None => Err(PlatformError::NotConnected),
            },
        }
    }

    async fn update_progress(
        &self,
        media: MediaId,
        progress: u32,
    ) -> Result<ListEntry, PlatformError> {
        match self.active {
            ActiveKind::Local => self.local.update_progress(media, progress).await,
            ActiveKind::AniList => match &self.anilist {
                Some(anilist) => anilist.update_progress(media, progress).await,
                None => Err(PlatformError::NotConnected),
            },
        }
    }

    async fn delete_entry(&self, media: MediaId) -> Result<(), PlatformError> {
        match self.active {
            ActiveKind::Local => self.local.delete_entry(media).await,
            ActiveKind::AniList => match &self.anilist {
                Some(anilist) => anilist.delete_entry(media).await,
                None => Err(PlatformError::NotConnected),
            },
        }
    }
}
