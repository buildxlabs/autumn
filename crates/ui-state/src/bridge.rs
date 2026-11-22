//! Runs engine futures on Tokio and hands their results back to GPUI.
//!
//! GPUI's executors are not a Tokio runtime, and `reqwest`, `sqlx` and
//! `librqbit` need one. `autumn` builds the runtime and passes its handle to
//! [`init`]. [`run`] spawns a future there and returns a GPUI [`Task`] that
//! resolves to the future's output. See ARCHITECTURE §4.

use std::future::Future;

use gpui_kit::{App, Global, Task};
use tokio::{runtime::Handle, task::JoinHandle};

/// Why a [`run`] call produced no value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BridgeError {
    /// [`init`] was never called, so there is no runtime to spawn on.
    #[error("the Tokio bridge is not initialized")]
    NotInitialized,
    /// The runtime shut down before the future finished.
    #[error("the Tokio task was cancelled")]
    Cancelled,
    /// The future panicked on its Tokio worker.
    #[error("the Tokio task panicked")]
    Panicked,
}

/// The Tokio runtime handle, stored as a GPUI global by [`init`].
struct TokioHandle(Handle);

impl Global for TokioHandle {}

/// Installs the Tokio runtime that [`run`] spawns on.
///
/// Call it once at boot, before any view issues a command. The runtime
/// itself stays owned by the caller and must outlive the app.
pub fn init(cx: &mut App, handle: Handle) {
    cx.set_global(TokioHandle(handle));
}

/// Spawns `future` on the Tokio runtime and returns a task for its output.
///
/// Store the returned task on the entity that issued the command. Dropping
/// it aborts the Tokio task, so a newer request cancels an older one.
///
/// # Errors
///
/// The task resolves to [`BridgeError::NotInitialized`] when [`init`] was
/// never called, [`BridgeError::Panicked`] when the future panicked, and
/// [`BridgeError::Cancelled`] when the runtime shut down first.
pub fn run<T, F>(cx: &App, future: F) -> Task<Result<T, BridgeError>>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let Some(TokioHandle(handle)) = cx.try_global::<TokioHandle>() else {
        return Task::ready(Err(BridgeError::NotInitialized));
    };
    cx.background_executor().spawn(spawn(handle, future))
}

/// Spawns work from a caller that already has the bridge's Tokio handle.
/// Dropping the returned future aborts the work.
///
/// # Errors
///
/// The future resolves to [`BridgeError::Panicked`] if the work panics or
/// [`BridgeError::Cancelled`] if the runtime shuts down first.
pub fn spawn<T, F>(
    handle: &Handle,
    future: F,
) -> impl Future<Output = Result<T, BridgeError>> + Send + use<T, F>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let join = AbortOnDrop(handle.spawn(future));
    async move {
        let mut join = join;
        (&mut join.0).await.map_err(|err| {
            if err.is_panic() {
                BridgeError::Panicked
            } else {
                BridgeError::Cancelled
            }
        })
    }
}

/// Aborts the Tokio task when the GPUI task awaiting it is dropped.
///
/// Dropping a bare `JoinHandle` detaches the task instead of stopping it.
struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use gpui_kit::TestAppContext;
    use tokio::runtime::{Builder, Handle, Runtime};

    use super::{BridgeError, init, run};

    fn runtime() -> Runtime {
        Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap()
    }

    #[gpui_kit::test]
    async fn command_resolves_with_the_tokio_futures_output(cx: &mut TestAppContext) {
        let runtime = runtime();
        cx.executor().allow_parking();
        cx.update(|cx| init(cx, runtime.handle().clone()));

        let task = cx.update(|cx| run(cx, async { Handle::try_current().is_ok() }));

        assert_eq!(task.await, Ok(true), "the future must run inside Tokio");
    }

    #[gpui_kit::test]
    async fn command_without_init_fails(cx: &mut TestAppContext) {
        let task = cx.update(|cx| run(cx, async { 1 }));

        assert_eq!(
            task.await,
            Err(BridgeError::NotInitialized),
            "no runtime was installed"
        );
    }

    #[gpui_kit::test]
    async fn panicking_command_reports_the_panic(cx: &mut TestAppContext) {
        let runtime = runtime();
        cx.executor().allow_parking();
        cx.update(|cx| init(cx, runtime.handle().clone()));

        let task = cx.update(|cx| {
            run(cx, async {
                #[expect(clippy::panic, reason = "the test needs a panicking future")]
                {
                    panic!("engine failure")
                }
            })
        });

        assert_eq!(
            task.await,
            Err::<(), _>(BridgeError::Panicked),
            "the panic must surface"
        );
    }

    #[gpui_kit::test]
    fn dropping_the_task_cancels_the_tokio_work(cx: &mut TestAppContext) {
        let runtime = runtime();
        cx.update(|cx| init(cx, runtime.handle().clone()));
        let (started_tx, started_rx) = mpsc::channel();
        let (dropped_tx, dropped_rx) = mpsc::channel();

        let task = cx.update(|cx| {
            run(cx, async move {
                let _signal = SignalOnDrop(dropped_tx);
                started_tx.send(()).unwrap();
                std::future::pending::<()>().await;
            })
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(task);
        // Drains the GPUI side; the abort itself happens inside `drop`.
        cx.run_until_parked();

        assert!(
            dropped_rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "the Tokio future must be dropped once the task is"
        );
    }

    /// Sends on its channel when dropped, to observe the future's drop.
    struct SignalOnDrop(mpsc::Sender<()>);

    impl Drop for SignalOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
}
