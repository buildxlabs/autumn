//! Test fakes for the playback manager: a launcher whose sessions take
//! their player events from a channel the test holds. Shared by the
//! playback tests and the Phase 2 end-to-end test.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use domain::PlaybackControl;
use futures::{StreamExt, channel::mpsc};
use player::{Error as PlayerError, PlayerEvent};
use tokio::sync::{Mutex as AsyncMutex, oneshot};

use super::{PlaybackSession, PlayerLaunch, PlayerLauncher, SessionFuture, lock};
use crate::BoxFuture;

#[derive(Debug, Default, Clone)]
pub(crate) struct FakeLauncher {
    pub(crate) seeks: Arc<Mutex<Vec<f64>>>,
    pub(crate) pauses: Arc<Mutex<Vec<PlaybackControl>>>,
    pub(crate) quits: Arc<AtomicUsize>,
    pub(crate) launches: Arc<AtomicUsize>,
    pub(crate) gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    /// Keeps each session's event sender alive until its test ends.
    pub(crate) senders: Arc<Mutex<Vec<mpsc::UnboundedSender<PlayerEvent>>>>,
    pub(crate) messages: Arc<Mutex<Vec<String>>>,
    /// Files replaced in the same window, as with mpv's `loadfile`.
    pub(crate) loads: Arc<Mutex<Vec<PathBuf>>>,
    /// Acts like VLC or MPC-HC, which need a fresh launch per file.
    pub(crate) relaunch: Arc<AtomicBool>,
    /// Makes every seek fail, as a player that rejects the command.
    pub(crate) fail_seeks: Arc<AtomicBool>,
}

impl FakeLauncher {
    pub(crate) fn session(&self) -> (Box<dyn PlaybackSession>, mpsc::UnboundedSender<PlayerEvent>) {
        let (tx, rx) = mpsc::unbounded();
        lock(&self.senders).push(tx.clone());
        let session = FakeSession {
            rx: AsyncMutex::new(rx),
            seeks: Arc::clone(&self.seeks),
            pauses: Arc::clone(&self.pauses),
            messages: Arc::clone(&self.messages),
            quits: Arc::clone(&self.quits),
            loads: Arc::clone(&self.loads),
            relaunch: Arc::clone(&self.relaunch),
            fail_seeks: Arc::clone(&self.fail_seeks),
        };
        (Box::new(session), tx)
    }

    /// The event sender of the `index`th session launched so far.
    pub(crate) fn sender(&self, index: usize) -> mpsc::UnboundedSender<PlayerEvent> {
        lock(&self.senders)
            .get(index)
            .cloned()
            .expect("a session was launched")
    }

    pub(crate) fn loads(&self) -> Vec<PathBuf> {
        lock(&self.loads).clone()
    }

    pub(crate) fn launches(&self) -> usize {
        self.launches.load(Ordering::SeqCst)
    }

    pub(crate) fn seeks(&self) -> Vec<f64> {
        lock(&self.seeks).clone()
    }

    pub(crate) fn pauses(&self) -> Vec<PlaybackControl> {
        lock(&self.pauses).clone()
    }

    pub(crate) fn set_gate(&self, gate: oneshot::Receiver<()>) {
        *lock(&self.gate) = Some(gate);
    }

    pub(crate) async fn wait_for_launch(&self) {
        while self.launches.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    }
}

impl PlayerLauncher for FakeLauncher {
    fn launch(
        &self,
        _launch: PlayerLaunch,
    ) -> BoxFuture<Result<Box<dyn PlaybackSession>, PlayerError>> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let (session, _) = self.session();
        let gate = lock(&self.gate).take();
        Box::pin(async move {
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            Ok(session)
        })
    }
}

#[derive(Debug)]
pub(crate) struct FakeSession {
    rx: AsyncMutex<mpsc::UnboundedReceiver<PlayerEvent>>,
    seeks: Arc<Mutex<Vec<f64>>>,
    pauses: Arc<Mutex<Vec<PlaybackControl>>>,
    messages: Arc<Mutex<Vec<String>>>,
    quits: Arc<AtomicUsize>,
    loads: Arc<Mutex<Vec<PathBuf>>>,
    relaunch: Arc<AtomicBool>,
    fail_seeks: Arc<AtomicBool>,
}

impl PlaybackSession for FakeSession {
    fn next_event(&mut self) -> SessionFuture<'_, Option<PlayerEvent>> {
        Box::pin(async move { self.rx.lock().await.next().await })
    }

    fn seek(&self, seconds: f64) -> SessionFuture<'_, Result<(), PlayerError>> {
        let seeks = Arc::clone(&self.seeks);
        let fail = self.fail_seeks.load(Ordering::SeqCst);
        Box::pin(async move {
            lock(&seeks).push(seconds);
            if fail {
                Err(PlayerError::Endpoint("seek rejected".to_owned()))
            } else {
                Ok(())
            }
        })
    }

    fn set_playback_state(
        &self,
        state: PlaybackControl,
    ) -> SessionFuture<'_, Result<(), PlayerError>> {
        let seen = Arc::clone(&self.pauses);
        Box::pin(async move {
            lock(&seen).push(state);
            Ok(())
        })
    }

    fn show_text<'a>(&'a self, text: &'a str) -> SessionFuture<'a, Result<(), PlayerError>> {
        let messages = Arc::clone(&self.messages);
        let text = text.to_owned();
        Box::pin(async move {
            lock(&messages).push(text);
            Ok(())
        })
    }

    fn quit(&self) -> SessionFuture<'_, Result<(), PlayerError>> {
        let quits = Arc::clone(&self.quits);
        Box::pin(async move {
            quits.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn load_file<'a>(&'a self, path: &'a Path) -> SessionFuture<'a, Result<bool, PlayerError>> {
        let relaunch = self.relaunch.load(Ordering::SeqCst);
        if !relaunch {
            lock(&self.loads).push(path.to_path_buf());
        }
        Box::pin(async move { Ok(!relaunch) })
    }
}
