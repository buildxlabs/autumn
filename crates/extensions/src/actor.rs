//! One single-caller Tokio actor per extension, and the host handle that
//! routes calls to them (ADR 0003, ADR 0014).
//!
//! Each actor owns its extension's [`Vm`] and runs one call at a time under
//! [`Limits`]. Different extensions never share a VM or a queue, so a call
//! stuck in one extension never delays another. After any failure other than
//! `InvalidResult` (a clean return, #268 Q4) the VM is dropped and rebuilt,
//! re-running the chunk, on the next call.

use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};

use domain::ExtensionId;
use net::{DeclaredHost, ExtensionHosts, GuardedClient};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

use crate::{
    error::ExtensionError,
    host::Network,
    limits::{KillSwitch, Limits, Metered, Watch},
    provider::{ProviderRequest, ProviderTorrent},
    runtime::{Prepare, Vm},
};

/// How many calls may wait for one extension before callers wait to queue.
const QUEUE_DEPTH: usize = 32;

/// One torrent provider to run: its identity and Luau source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSource {
    /// The manifest slug.
    pub id: ExtensionId,
    /// The display name.
    pub name: String,
    /// The manifest version.
    pub version: String,
    /// The Luau payload.
    pub source: String,
    /// The hosts its requests may reach (`permissions.hosts`).
    pub hosts: Vec<DeclaredHost>,
}

/// A provider the host can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInfo {
    /// The manifest slug.
    pub id: ExtensionId,
    /// The display name.
    pub name: String,
    /// The manifest version.
    pub version: String,
}

/// The torrent providers' runtime: one actor per extension.
///
/// Cloning is cheap and shares the actors. They stop once every clone is
/// dropped, or when [`Self::stop`] is called.
#[derive(Debug, Clone)]
pub struct ExtensionHost {
    providers: Arc<[Provider]>,
}

#[derive(Debug)]
struct Provider {
    info: ProviderInfo,
    calls: mpsc::Sender<Call>,
    stop: watch::Sender<bool>,
    actor: Mutex<Option<JoinHandle<()>>>,
}

/// A queued `search` call and where its answer goes.
struct Call {
    request: ProviderRequest,
    reply: oneshot::Sender<Result<Vec<ProviderTorrent>, ExtensionError>>,
    /// Set when the caller drops its future, so the watchdog can interrupt
    /// Luau that is running.
    caller_gone: Arc<AtomicBool>,
}

/// Sets its flag when dropped.
struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl ExtensionHost {
    /// Starts one actor per provider. The first source with a given id wins;
    /// later duplicates are skipped with a warning.
    ///
    /// Every request goes through `http`, held to the source's declared
    /// hosts and rate-limited per extension id.
    ///
    /// Must be called inside a Tokio runtime. Each VM is built on its first
    /// call, and rebuilt on the next call after a failure.
    #[must_use]
    pub fn start(sources: Vec<ProviderSource>, http: &GuardedClient) -> Self {
        Self::start_with(sources, http, Limits::PHASE_4, |_| Ok(()))
    }

    /// [`Self::start`] under other caps and with extra globals, for tests.
    pub(crate) fn start_with(
        sources: Vec<ProviderSource>,
        http: &GuardedClient,
        limits: Limits,
        prepare: Prepare,
    ) -> Self {
        let mut providers: Vec<Provider> = Vec::with_capacity(sources.len());
        for source in sources {
            if providers
                .iter()
                .any(|provider| provider.info.id == source.id)
            {
                tracing::warn!(extension = %source.id, "skipping a duplicate extension id");
                continue;
            }
            let (calls, queue) = mpsc::channel(QUEUE_DEPTH);
            let (stop, stopping) = watch::channel(false);
            let network = Network {
                client: http.clone(),
                hosts: ExtensionHosts::new(source.id.as_str(), source.hosts),
            };
            let actor = Actor {
                id: source.id.clone(),
                source: source.source,
                network,
                limits,
                prepare,
                stopping,
                vm: None,
            };
            let actor = tokio::spawn(actor.run(queue));
            providers.push(Provider {
                info: ProviderInfo {
                    id: source.id,
                    name: source.name,
                    version: source.version,
                },
                calls,
                stop,
                actor: Mutex::new(Some(actor)),
            });
        }
        Self {
            providers: providers.into(),
        }
    }

    /// The providers this host can run, in the order they were given.
    pub fn providers(&self) -> impl Iterator<Item = &ProviderInfo> {
        self.providers.iter().map(|provider| &provider.info)
    }

    /// Runs one provider's `search(request)`.
    ///
    /// Dropping the future while the call waits in the queue skips it without
    /// touching the VM; dropping it mid-call kills the call.
    ///
    /// # Errors
    ///
    /// - [`ExtensionError::UnknownExtension`] when no provider has `id`.
    /// - [`ExtensionError::Stopped`] when its actor has stopped.
    /// - [`ExtensionError::Cancelled`] when the extension is stopped while
    ///   the call waits or runs.
    /// - Otherwise the load, call or kill error from the VM.
    pub async fn search(
        &self,
        id: &ExtensionId,
        request: ProviderRequest,
    ) -> Result<Vec<ProviderTorrent>, ExtensionError> {
        let provider = self.provider(id)?;
        let (reply, answer) = oneshot::channel();
        let caller_gone = Arc::new(AtomicBool::new(false));
        let _guard = SetOnDrop(Arc::clone(&caller_gone));
        provider
            .calls
            .send(Call {
                request,
                reply,
                caller_gone,
            })
            .await
            .map_err(|_| ExtensionError::Stopped(id.clone()))?;
        answer
            .await
            .map_err(|_| ExtensionError::Stopped(id.clone()))?
    }

    /// Stops one extension's actor (disable, remove and update, #266 §5).
    ///
    /// Kills the call in flight, fails queued callers with
    /// [`ExtensionError::Cancelled`], and resolves once the VM is dropped.
    /// Later calls fail with [`ExtensionError::Stopped`].
    ///
    /// # Errors
    ///
    /// [`ExtensionError::UnknownExtension`] when no provider has `id`.
    pub async fn stop(&self, id: &ExtensionId) -> Result<(), ExtensionError> {
        let provider = self.provider(id)?;
        provider.stop.send_replace(true);
        let actor = provider
            .actor
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(actor) = actor
            && let Err(error) = actor.await
        {
            tracing::warn!(extension = %id, %error, "extension actor ended abnormally");
        }
        Ok(())
    }

    fn provider(&self, id: &ExtensionId) -> Result<&Provider, ExtensionError> {
        self.providers
            .iter()
            .find(|provider| &provider.info.id == id)
            .ok_or_else(|| ExtensionError::UnknownExtension(id.clone()))
    }
}

/// The task that owns one extension's VM.
struct Actor {
    id: ExtensionId,
    source: String,
    network: Network,
    limits: Limits,
    prepare: Prepare,
    stopping: watch::Receiver<bool>,
    vm: Option<Vm>,
}

impl Actor {
    /// Serves calls in order until the host is dropped or stops this
    /// extension.
    async fn run(mut self, mut queue: mpsc::Receiver<Call>) {
        let mut stopping = self.stopping.clone();
        loop {
            let call = tokio::select! {
                biased;
                _ = stopping.wait_for(|stop| *stop) => break,
                call = queue.recv() => call,
            };
            let Some(Call {
                request,
                mut reply,
                caller_gone,
            }) = call
            else {
                return;
            };
            if reply.is_closed() {
                continue;
            }
            let outcome = tokio::select! {
                outcome = self.call(&request, caller_gone) => outcome,
                () = reply.closed() => Err(ExtensionError::Cancelled),
                _ = stopping.wait_for(|stop| *stop) => Err(ExtensionError::Cancelled),
            };
            if !matches!(outcome, Ok(_) | Err(ExtensionError::InvalidResult(_))) {
                self.vm = None;
            }
            // The caller may have gone in the meantime; nobody is left to tell.
            let _ = reply.send(outcome);
        }
        self.vm = None;
        queue.close();
        while let Some(call) = queue.recv().await {
            let _ = call.reply.send(Err(ExtensionError::Cancelled));
        }
    }

    /// Runs one call under the limits, loading the VM first if needed.
    async fn call(
        &mut self,
        request: &ProviderRequest,
        caller_gone: Arc<AtomicBool>,
    ) -> Result<Vec<ProviderTorrent>, ExtensionError> {
        let switch = self
            .vm
            .as_ref()
            .map_or_else(|| Arc::new(KillSwitch::default()), Vm::switch);
        let watch = Watch::start(
            Arc::clone(&switch),
            self.limits,
            caller_gone,
            self.stopping.clone(),
        )?;
        let run = Metered::new(&watch, async {
            let vm = match &mut self.vm {
                Some(vm) => vm,
                vm @ None => vm.insert(
                    Vm::load(
                        self.id.clone(),
                        &self.source,
                        self.limits.memory,
                        Arc::clone(&switch),
                        self.network.clone(),
                        self.prepare,
                    )
                    .await?,
                ),
            };
            vm.search(request).await
        });
        let outcome = tokio::time::timeout(self.limits.wall, run)
            .await
            .unwrap_or(Err(ExtensionError::WallTimeout));
        // A kill decides the outcome, whatever the script returned.
        match switch.reason() {
            Some(kill) => Err(kill.into()),
            None => outcome,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::AtomicUsize,
        time::{Duration, Instant},
    };

    use mlua::Lua;

    use super::*;

    /// Short caps so kills land fast.
    const TEST_LIMITS: Limits = Limits {
        memory: 4 * 1024 * 1024,
        cpu: Duration::from_millis(200),
        wall: Duration::from_millis(400),
    };

    /// How many `hang()` futures have been dropped.
    static HANG_DROPS: AtomicUsize = AtomicUsize::new(0);

    struct DropProbe;

    impl Drop for DropProbe {
        fn drop(&mut self) {
            HANG_DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Installs `hang()`, a host function that never resolves.
    fn install_hang(lua: &Lua) -> mlua::Result<()> {
        let hang = lua.create_async_function(|_, ()| async {
            let _probe = DropProbe;
            std::future::pending::<()>().await;
            Ok(())
        })?;
        lua.globals().raw_set("hang", hang)
    }

    fn client() -> GuardedClient {
        GuardedClient::new().unwrap()
    }

    fn source(lua: &str) -> ProviderSource {
        ProviderSource {
            id: ExtensionId::new("t"),
            name: "T".to_owned(),
            version: "1.0.0".to_owned(),
            source: lua.to_owned(),
            hosts: Vec::new(),
        }
    }

    fn host(lua: &str) -> ExtensionHost {
        ExtensionHost::start_with(vec![source(lua)], &client(), TEST_LIMITS, install_hang)
    }

    /// Calls `search` with `keyword`.
    fn call(
        host: &ExtensionHost,
        keyword: &str,
    ) -> impl Future<Output = Result<Vec<ProviderTorrent>, ExtensionError>> + use<> {
        let host = host.clone();
        let request = ProviderRequest::keyword(keyword);
        async move { host.search(&ExtensionId::new("t"), request).await }
    }

    /// The first row's name: the VM's call count, which a rebuild resets.
    async fn count(host: &ExtensionHost) -> String {
        let rows = call(host, "count").await.unwrap();
        rows.into_iter().next().and_then(|row| row.name).unwrap()
    }

    /// Runs `work` when the keyword is `work`; every call returns the VM's
    /// call count.
    fn on_work(work: &str) -> String {
        format!(
            r#"
            local calls = 0
            return {{ search = function(request)
                calls += 1
                if request.keyword == "work" then
                    {work}
                end
                return {{ {{ name = tostring(calls) }} }}
            end }}
            "#
        )
    }

    #[tokio::test]
    async fn a_hung_host_future_times_out_and_is_dropped() {
        let host = host(&on_work("hang()"));
        assert_eq!(count(&host).await, "1");
        let started = Instant::now();
        assert_eq!(call(&host, "work").await, Err(ExtensionError::WallTimeout));
        assert!(
            started.elapsed() >= TEST_LIMITS.wall,
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            HANG_DROPS.load(Ordering::SeqCst),
            1,
            "the host future was dropped"
        );
        assert_eq!(count(&host).await, "1", "the VM was rebuilt");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn allocating_past_the_cap_is_a_memory_kill_even_under_pcall() {
        let grow = "local t = {} while true do table.insert(t, string.rep('x', 1024)) end";
        let cases = [
            grow.to_owned(),
            format!("pcall(function() {grow} end)"),
            format!("xpcall(function() {grow} end, function(e) return e end)"),
        ];
        for work in cases {
            let host = host(&on_work(&work));
            assert_eq!(count(&host).await, "1");
            assert_eq!(
                call(&host, "work").await,
                Err(ExtensionError::MemoryLimit),
                "{work}"
            );
            assert_eq!(count(&host).await, "1", "a clean call on a fresh VM");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_runtime_error_rebuilds_but_an_invalid_result_keeps_the_vm() {
        let lua = r#"
            local calls = 0
            return { search = function(request)
                calls += 1
                if request.keyword == "invalid" then return "not rows" end
                if request.keyword == "error" then error("broken") end
                return { { name = tostring(calls) } }
            end }
        "#;
        let host = host(lua);
        assert_eq!(count(&host).await, "1");
        assert!(matches!(
            call(&host, "invalid").await,
            Err(ExtensionError::InvalidResult(_))
        ));
        assert_eq!(
            count(&host).await,
            "3",
            "an invalid result keeps the VM warm"
        );
        assert!(matches!(
            call(&host, "error").await,
            Err(ExtensionError::Runtime { .. })
        ));
        assert_eq!(count(&host).await, "1", "the error rebuilt the VM");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_caller_mid_loop_kills_and_rebuilds() {
        let host = ExtensionHost::start_with(
            vec![source(&on_work("while true do end"))],
            &client(),
            Limits {
                cpu: Duration::from_secs(30),
                wall: Duration::from_secs(30),
                ..TEST_LIMITS
            },
            install_hang,
        );
        assert_eq!(count(&host).await, "1");
        let started = Instant::now();
        let dropped = tokio::time::timeout(Duration::from_millis(100), call(&host, "work")).await;
        assert!(dropped.is_err(), "the call was still running");
        assert_eq!(count(&host).await, "1", "the cancelled VM was rebuilt");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_kills_the_running_call_and_cancels_the_queue() {
        let host = ExtensionHost::start_with(
            vec![source(&on_work("while true do end"))],
            &client(),
            Limits {
                cpu: Duration::from_secs(30),
                wall: Duration::from_secs(30),
                ..TEST_LIMITS
            },
            install_hang,
        );
        let running = tokio::spawn(call(&host, "work"));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let queued = tokio::spawn(call(&host, "count"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let started = Instant::now();
        host.stop(&ExtensionId::new("t")).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(running.await.unwrap(), Err(ExtensionError::Cancelled));
        assert_eq!(queued.await.unwrap(), Err(ExtensionError::Cancelled));
        assert_eq!(
            call(&host, "count").await,
            Err(ExtensionError::Stopped(ExtensionId::new("t")))
        );
    }
}
