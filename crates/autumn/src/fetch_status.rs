//! Route the shared HTTP client's activity into the UI event pump.

use domain::{AniListWait, AppEvent};
use net::{ANILIST_HOST, Client, RateLimit};
use ui_state::events::Sender;

#[cfg(debug_assertions)]
pub(crate) mod debug;

pub(crate) async fn observe(client: &Client, events: Sender) {
    let mut activity = client.request_activity();
    let Some(mut anilist) = client.rate_limit(ANILIST_HOST) else {
        return;
    };
    #[cfg(debug_assertions)]
    let mut loopback = client.rate_limit(debug::HOST);
    #[cfg(not(debug_assertions))]
    let mut loopback = None;

    // Send lifecycle transitions promptly so the loader can cancel its 300ms timer.
    let mut oldest = None;
    let mut deadline = None;
    loop {
        let started = activity.oldest_request();
        if started != oldest {
            oldest = started;
            if events
                .send_async(AppEvent::FetchActivity(started))
                .await
                .is_err()
            {
                return;
            }
        }
        let primary = anilist.snapshot();
        let debug = loopback.as_mut().and_then(RateLimit::snapshot);
        let states = primary.into_iter().chain(debug);
        let queued = states.clone().any(|state| state.queued);
        let wait = states.map(|state| state.until).max().map(|until| {
            if queued {
                AniListWait::queued(until)
            } else {
                AniListWait::lockout(until)
            }
        });
        if wait != deadline {
            deadline = wait;
            if events
                .send_async(AppEvent::AniListRateLimit(wait))
                .await
                .is_err()
            {
                return;
            }
        }

        let alive = tokio::select! {
            changed = activity.changed() => changed,
            changed = anilist.changed() => changed,
            changed = debug_rate_changed(loopback.as_mut()) => changed,
        };
        if !alive {
            return;
        }
    }
}

async fn debug_rate_changed(rate: Option<&mut RateLimit>) -> bool {
    match rate {
        Some(rate) => rate.changed().await,
        None => std::future::pending().await,
    }
}
