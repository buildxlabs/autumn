//! Extension lanes at the search seam: deadline, cache, dedupe and badges.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use domain::ExtensionId;
use extensions::{ExtensionError, ProviderRequest, ProviderTorrent};
use search::{
    FakeClock, Indexer, LaneFilter, ProviderMeta, ProviderSet, Query, SearchRequest, Searcher,
    Transport,
};

const EMPTY_JSON: &str = "[]";
const RSS_ONE: &str = r"<rss><channel><item>
<title>[Group] Frieren - 01</title><link>https://nyaa.si/download/1.torrent</link>
<nyaa:infoHash>aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</nyaa:infoHash>
<nyaa:size>100 MiB</nyaa:size><nyaa:seeders>10</nyaa:seeders>
</item></channel></rss>";
const HEX: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";

/// Nyaa always answers; `AnimeTosho` is empty.
#[derive(Clone, Debug, Default)]
struct BuiltIns;

impl Transport for BuiltIns {
    fn fetch(&self, query: &Query) -> impl Future<Output = Result<Vec<u8>, search::Error>> + Send {
        let body = if matches!(query, Query::Nyaa { .. }) {
            RSS_ONE
        } else {
            EMPTY_JSON
        };
        std::future::ready(Ok(body.as_bytes().to_vec()))
    }
}

type Replies = Arc<Mutex<HashMap<ExtensionId, Vec<Result<Vec<ProviderTorrent>, ExtensionError>>>>>;

/// A provider set recording every call, with scripted replies and a hang flag.
#[derive(Clone, Debug, Default)]
struct FakeProviders {
    metas: Arc<Mutex<Vec<ProviderMeta>>>,
    replies: Replies,
    calls: Arc<Mutex<Vec<(ExtensionId, ProviderRequest)>>>,
    hang: bool,
}

impl FakeProviders {
    fn with(metas: Vec<ProviderMeta>) -> Self {
        Self {
            metas: Arc::new(Mutex::new(metas)),
            ..Self::default()
        }
    }

    fn reply(&self, id: &str, rows: Vec<ProviderTorrent>) {
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(ExtensionId::new(id), vec![Ok(rows)]);
    }

    fn calls(&self) -> Vec<(ExtensionId, ProviderRequest)> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_version_of(&self, id: &str, version: &str) {
        let id = ExtensionId::new(id);
        for meta in self
            .metas
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter_mut()
        {
            if meta.id == id {
                version.clone_into(&mut meta.version);
            }
        }
    }
}

impl ProviderSet for FakeProviders {
    fn providers(&self) -> Vec<ProviderMeta> {
        self.metas
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn search(
        &self,
        id: ExtensionId,
        request: ProviderRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ProviderTorrent>, ExtensionError>> + Send + 'static>>
    {
        let me = self.clone();
        Box::pin(async move {
            me.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((id.clone(), request));
            if me.hang {
                std::future::pending::<()>().await;
            }
            let mut replies = me.replies.lock().unwrap_or_else(PoisonError::into_inner);
            match replies.get_mut(&id) {
                Some(queue) if queue.len() > 1 => queue.remove(0),
                Some(queue) => queue.first().cloned().unwrap_or(Ok(Vec::new())),
                None => Ok(Vec::new()),
            }
        })
    }
}

fn meta(id: &str, name: &str, version: &str) -> ProviderMeta {
    ProviderMeta::new(
        ExtensionId::new(id),
        name,
        version,
        vec!["example.org".to_owned()],
    )
}

fn row(hash: &str, name: &str) -> ProviderTorrent {
    ProviderTorrent {
        name: Some(name.to_owned()),
        infohash: Some(hash.to_owned()),
        ..ProviderTorrent::default()
    }
}

fn request() -> SearchRequest {
    SearchRequest::new(vec!["Frieren".to_owned()], 1, 5)
}

#[tokio::test]
async fn a_never_answering_provider_is_dropped_at_the_deadline_while_nyaa_returns() {
    let providers = FakeProviders {
        hang: true,
        ..FakeProviders::with(vec![meta("slow", "Slow", "1.0.0")])
    };
    let searcher = Searcher::new(BuiltIns)
        .with_providers(Arc::new(providers))
        .with_provider_deadline(Duration::from_millis(50));

    let outcome = searcher
        .search(&request())
        .await
        .expect("Nyaa still answers");

    assert_eq!(outcome.torrents.len(), 1, "the built-in lane survives");
    assert_eq!(
        outcome
            .torrents
            .first()
            .map(|torrent| torrent.infohash.as_str()),
        Some("a".repeat(40).as_str())
    );
    assert_eq!(outcome.failures.len(), 1, "the stalled lane is reported");
    assert!(
        matches!(
            outcome.failures.first().map(|failure| &failure.indexer),
            Some(Indexer::Extension { id, .. }) if id.as_str() == "slow"
        ),
        "the failed lane is named: {:?}",
        outcome.failures
    );
    assert!(
        matches!(
            outcome.failures.first().map(|failure| &failure.error),
            Some(search::Error::ProviderTimeout { .. })
        ),
        "the lane reports a timeout, not a search failure"
    );
}

#[tokio::test]
async fn the_cache_key_includes_the_version_and_purge_removes_only_that_id() {
    let clock = FakeClock::default();
    let id = ExtensionId::new("provider");
    let providers = FakeProviders::with(vec![
        meta("provider", "Provider", "1.0.0"),
        meta("other", "Other", "1.0.0"),
    ]);
    providers.reply("provider", vec![row(HEX, "[Grp] Frieren - 05")]);
    providers.reply("other", vec![row(HEX, "[Grp] Frieren - 05")]);
    let searcher =
        Searcher::with_clock(BuiltIns, clock.clone()).with_providers(Arc::new(providers.clone()));

    searcher.search(&request()).await.expect("first search");
    searcher.search(&request()).await.expect("cached search");
    assert_eq!(
        providers.calls().len(),
        2,
        "both lanes cached within the TTL"
    );

    providers.set_version_of("provider", "2.0.0");
    searcher.search(&request()).await.expect("new version");
    assert_eq!(providers.calls().len(), 3, "version is part of the key");
    assert_eq!(
        providers.calls().last().map(|(id, _)| id.as_str()),
        Some("provider"),
        "only the updated provider was called"
    );

    searcher.purge_provider(&id);
    searcher.search(&request()).await.expect("after purge");
    assert_eq!(
        providers.calls().len(),
        4,
        "only this id's entries are purged"
    );
    assert_eq!(
        providers.calls().last().map(|(id, _)| id.as_str()),
        Some("provider"),
        "the other extension stays cached"
    );

    clock.advance(Duration::from_secs(61));
    searcher.search(&request()).await.expect("expired");
    assert_eq!(providers.calls().len(), 6, "the 60 s TTL still applies");
}

#[tokio::test]
async fn dedupe_keeps_built_ins_first_and_labels_clashing_provider_names() {
    let providers = FakeProviders::with(vec![
        meta("zeta", "Mirror", "1.0.0"),
        meta("alpha", "Mirror", "1.0.0"),
    ]);
    providers.reply(
        "alpha",
        vec![
            row("a".repeat(40).as_str(), "[Grp] Frieren duplicate"),
            row("b".repeat(40).as_str(), "[Grp] Frieren - 05"),
        ],
    );
    providers.reply(
        "zeta",
        vec![row("c".repeat(40).as_str(), "[Grp] Frieren - 06")],
    );
    let searcher = Searcher::new(BuiltIns).with_providers(Arc::new(providers));

    let outcome = searcher.search(&request()).await.expect("search succeeds");

    assert_eq!(outcome.torrents.len(), 3, "the duplicate is dropped");
    assert_eq!(
        outcome
            .torrents
            .first()
            .map(|torrent| torrent.infohash.as_str()),
        Some("a".repeat(40).as_str()),
        "Nyaa is merged first"
    );
    assert_eq!(
        outcome.torrents.first().and_then(|torrent| torrent.seeders),
        Some(10),
        "the built-in result wins the duplicate"
    );
    assert_eq!(
        outcome
            .torrents
            .get(1)
            .map(|torrent| torrent.infohash.as_str()),
        Some("b".repeat(40).as_str())
    );
    assert_eq!(
        outcome
            .torrents
            .get(1)
            .map(|torrent| torrent.indexer.label()),
        Some("Mirror (alpha)"),
        "clashing display names are disambiguated"
    );
    assert_eq!(
        outcome
            .torrents
            .get(2)
            .map(|torrent| torrent.indexer.label()),
        Some("Mirror (zeta)")
    );
}

#[tokio::test]
async fn availability_badges_never_call_a_provider() {
    let providers = FakeProviders::with(vec![meta("provider", "Provider", "1.0.0")]);
    providers.reply("provider", vec![row(HEX, "[Grp] Frieren - 05")]);
    let shared = Arc::new(providers.clone());
    let searcher = Searcher::new(BuiltIns).with_providers(shared);

    let outcome = searcher
        .search_with_filter(&request(), LaneFilter::BuiltIn)
        .await
        .expect("badge search");

    assert!(providers.calls().is_empty(), "no provider was called");
    assert!(outcome.failures.is_empty());
    assert_eq!(outcome.torrents.len(), 1, "Nyaa's result is returned");
}

#[tokio::test]
async fn structured_and_keyword_searches_send_the_provider_the_right_request() {
    let providers = FakeProviders::with(vec![meta("provider", "Provider", "1.0.0")]);
    providers.reply(
        "provider",
        vec![
            row(HEX, "[Grp] Frieren - 05"),
            row(HEX, "[Grp] Frieren - 05"),
        ],
    );
    let searcher = Searcher::new(BuiltIns).with_providers(Arc::new(providers.clone()));
    let search = SearchRequest::new(
        vec!["Sousou no Frieren S2!".to_owned(), "Frieren".to_owned()],
        2,
        5,
    )
    .with_absolute_episode(30)
    .with_anidb_ids(17_617, Some(271_422));

    searcher.search(&search).await.expect("structured");
    searcher
        .search_keyword("frieren batch", false)
        .await
        .expect("keyword");

    let calls = providers.calls();
    let (_, structured) = calls.first().expect("structured call");
    assert_eq!(structured.keyword, "sousou no frieren");
    let episode = structured.episode.as_ref().expect("episode fields");
    assert_eq!(episode.titles, ["sousou no frieren", "frieren"]);
    assert_eq!((episode.season, episode.episode), (2, 5));
    assert_eq!(structured.absolute_episode, Some(30));
    assert_eq!(
        (structured.anidb_aid, structured.anidb_eid),
        (Some(17_617), Some(271_422))
    );

    let (_, keyword) = calls.get(1).expect("keyword call");
    assert_eq!(keyword.keyword, "frieren batch");
    assert!(
        keyword.episode.is_none(),
        "a keyword search has no episode fields"
    );
}
