//! One single-caller Tokio actor per extension, and the host handle that
//! routes calls to them (ADR 0003, ADR 0014).
//!
//! Each actor owns its extension's [`Vm`] and runs one call at a time.
//! Different extensions never share a VM or a queue, so a call stuck in one
//! extension never delays another.

use std::sync::Arc;

use domain::ExtensionId;
use tokio::sync::{mpsc, oneshot};

use crate::{
    error::ExtensionError,
    provider::{ProviderRequest, ProviderTorrent},
    runtime::Vm,
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
/// dropped.
#[derive(Debug, Clone)]
pub struct ExtensionHost {
    providers: Arc<[Provider]>,
}

#[derive(Debug)]
struct Provider {
    info: ProviderInfo,
    calls: mpsc::Sender<Call>,
}

/// A queued `search` call and where its answer goes.
struct Call {
    request: ProviderRequest,
    reply: oneshot::Sender<Result<Vec<ProviderTorrent>, ExtensionError>>,
}

impl ExtensionHost {
    /// Starts one actor per provider. The first source with a given id wins;
    /// later duplicates are skipped with a warning.
    ///
    /// Must be called inside a Tokio runtime. Each VM is built on its first
    /// call, and rebuilt on the next call after its chunk failed to load.
    #[must_use]
    pub fn start(sources: Vec<ProviderSource>) -> Self {
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
            tokio::spawn(run_actor(source.id.clone(), source.source, queue));
            providers.push(Provider {
                info: ProviderInfo {
                    id: source.id,
                    name: source.name,
                    version: source.version,
                },
                calls,
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
    /// touching the VM.
    ///
    /// # Errors
    ///
    /// - [`ExtensionError::UnknownExtension`] when no provider has `id`.
    /// - [`ExtensionError::Stopped`] when its actor has stopped.
    /// - Otherwise the load or call error from the VM.
    pub async fn search(
        &self,
        id: &ExtensionId,
        request: ProviderRequest,
    ) -> Result<Vec<ProviderTorrent>, ExtensionError> {
        let provider = self
            .providers
            .iter()
            .find(|provider| &provider.info.id == id)
            .ok_or_else(|| ExtensionError::UnknownExtension(id.clone()))?;
        let (reply, answer) = oneshot::channel();
        provider
            .calls
            .send(Call { request, reply })
            .await
            .map_err(|_| ExtensionError::Stopped(id.clone()))?;
        answer
            .await
            .map_err(|_| ExtensionError::Stopped(id.clone()))?
    }
}

/// Serves one extension's calls in order, building its VM on demand.
async fn run_actor(id: ExtensionId, source: String, mut queue: mpsc::Receiver<Call>) {
    let mut vm: Option<Vm> = None;
    while let Some(Call { request, mut reply }) = queue.recv().await {
        if reply.is_closed() {
            continue;
        }
        let outcome = tokio::select! {
            outcome = call(&id, &source, &mut vm, &request) => outcome,
            () = reply.closed() => continue,
        };
        // The caller may have gone in the meantime; nobody is left to tell.
        let _ = reply.send(outcome);
    }
}

/// Loads the VM if needed, then runs `search` in it.
async fn call(
    id: &ExtensionId,
    source: &str,
    vm: &mut Option<Vm>,
    request: &ProviderRequest,
) -> Result<Vec<ProviderTorrent>, ExtensionError> {
    let vm = match vm {
        Some(vm) => vm,
        None => vm.insert(Vm::load(id.clone(), source).await?),
    };
    vm.search(request).await
}
