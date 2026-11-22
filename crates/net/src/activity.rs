//! Cancellation-safe observation of requests shared by all client clones.

use std::{collections::BTreeMap, time::Instant};

use tokio::sync::watch;

pub(crate) type ActiveRequests = BTreeMap<Instant, usize>;

/// Observes in-flight requests, including waits and retries.
#[derive(Debug)]
pub struct RequestActivity(watch::Receiver<ActiveRequests>);

impl RequestActivity {
    pub(crate) fn new(receiver: watch::Receiver<ActiveRequests>) -> Self {
        Self(receiver)
    }

    /// Wait for activity to change. Returns false when the client and its responses are gone.
    pub async fn changed(&mut self) -> bool {
        self.0.changed().await.is_ok()
    }

    /// When the longest-running request started.
    #[must_use]
    pub fn oldest_request(&mut self) -> Option<Instant> {
        self.0
            .borrow_and_update()
            .first_key_value()
            .map(|(&started, _)| started)
    }
}

#[derive(Debug)]
pub(crate) struct InFlight {
    activity: watch::Sender<ActiveRequests>,
    started: Instant,
}

impl InFlight {
    pub(crate) fn begin(activity: &watch::Sender<ActiveRequests>) -> Self {
        let started = Instant::now();
        activity.send_modify(|activity| *activity.entry(started).or_default() += 1);
        Self {
            activity: activity.clone(),
            started,
        }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.activity.send_modify(|activity| {
            if let std::collections::btree_map::Entry::Occupied(mut entry) =
                activity.entry(self.started)
            {
                *entry.get_mut() -= 1;
                if *entry.get() == 0 {
                    entry.remove();
                }
            }
        });
    }
}
