//! Progress sync through the same active-platform gateway the screens use.
//!
//! Playback must write to whichever platform is active (ADR 0004), and a
//! rejected AniList token makes that gateway read-only, which the engine
//! turns into a non-retryable toast.

use domain::{ListEntry, MediaId};
use engine::{BoxFuture, ProgressSync, SyncError};
use ui_state::{GatewayError, PlatformGateway};

#[derive(Debug, Clone)]
pub(crate) struct GatewaySync {
    gateway: PlatformGateway,
}

impl GatewaySync {
    pub(crate) fn new(gateway: PlatformGateway) -> Self {
        Self { gateway }
    }

    fn convert(error: &GatewayError) -> SyncError {
        // OD-5 shows a toast with Retry for every sync failure, including a
        // read-only account: re-login makes a retry succeed.
        SyncError::retryable(error.message().to_owned())
    }
}

impl ProgressSync for GatewaySync {
    fn entry(&self, media: MediaId) -> BoxFuture<Result<Option<ListEntry>, SyncError>> {
        let gateway = self.gateway.clone();
        Box::pin(async move {
            gateway
                .entry(media)
                .await
                .map_err(|error| Self::convert(&error))
        })
    }

    fn update_progress(
        &self,
        media: MediaId,
        progress: u32,
    ) -> BoxFuture<Result<ListEntry, SyncError>> {
        let gateway = self.gateway.clone();
        Box::pin(async move {
            gateway
                .update_progress(media, progress)
                .await
                .map_err(|error| Self::convert(&error))
        })
    }

    fn save_entry(&self, entry: &ListEntry) -> BoxFuture<Result<ListEntry, SyncError>> {
        let gateway = self.gateway.clone();
        let entry = entry.clone();
        Box::pin(async move {
            gateway
                .save_entry(&entry)
                .await
                .map_err(|error| Self::convert(&error))
        })
    }
}

#[cfg(test)]
mod tests {
    use domain::MediaId;

    use super::{GatewaySync, ProgressSync};

    #[tokio::test(flavor = "current_thread")]
    async fn a_read_only_account_still_offers_retry() {
        let dir = tempfile::tempdir().expect("a temp directory");
        let db = db::Db::open(&dir.path().join("autumn.db"))
            .await
            .expect("a database");
        let gateway = ui_state::PlatformGateway::new(db);
        gateway.mark_read_only();
        let sync = GatewaySync::new(gateway);

        let error = sync
            .update_progress(MediaId::new(1), 2)
            .await
            .expect_err("a read-only platform refuses the write");
        assert!(error.is_retryable(), "OD-5 offers Retry for every failure");
    }
}
