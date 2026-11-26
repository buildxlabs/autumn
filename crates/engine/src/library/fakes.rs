//! Test fakes for library scans: a candidate source with an optional gate,
//! and a wait for the next finished scan. Shared by the library tests and
//! the Phase 2 end-to-end test.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use ::library::Candidate;
use domain::AppEvent;
use tokio::time::timeout;

use super::CandidateSource;
use crate::{BoxFuture, EngineError};

#[derive(Debug)]
pub(crate) struct FakeCandidates {
    pub(crate) titles: Vec<Candidate>,
    pub(crate) gate: Arc<Mutex<Option<flume::Receiver<()>>>>,
}

impl CandidateSource for FakeCandidates {
    fn load(&self) -> BoxFuture<Result<Vec<Candidate>, EngineError>> {
        let titles = self.titles.clone();
        let gate = Arc::clone(&self.gate);
        Box::pin(async move {
            let receiver = gate.lock().ok().and_then(|mut slot| slot.take());
            if let Some(receiver) = receiver {
                let _ = receiver.recv_async().await;
            }
            Ok(titles)
        })
    }
}

/// Waits for the next finished scan, skipping progress events.
pub(crate) async fn next_finish(
    receiver: &flume::Receiver<AppEvent>,
    wait: Duration,
) -> Option<(usize, usize, usize, Option<String>)> {
    timeout(wait, async {
        loop {
            let event = receiver.recv_async().await.ok()?;
            if let AppEvent::LibraryScanFinished {
                files_walked,
                files_matched,
                files_unmatched,
                error,
                ..
            } = event
            {
                return Some((files_walked, files_matched, files_unmatched, error));
            }
        }
    })
    .await
    .ok()
    .flatten()
}
