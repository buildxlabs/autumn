//! Budget, dedupe, retry, cache and concurrency at the search seam.

use std::{collections::HashMap, sync::Arc, time::Duration};

use search::{Config, Error, Indexer, Query, SearchRequest, Searcher, Transport};
use tokio::sync::Barrier;

const EMPTY_RSS: &str = "<rss><channel></channel></rss>";
const RSS_ONE: &str = r"<rss><channel><item>
<title>[Group] Frieren - 01</title><link>https://nyaa.si/download/1.torrent</link>
<nyaa:infoHash>aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</nyaa:infoHash>
<nyaa:size>100 MiB</nyaa:size><nyaa:seeders>10</nyaa:seeders>
<nyaa:leechers>2</nyaa:leechers><nyaa:downloads>5</nyaa:downloads>
<nyaa:trusted>No</nyaa:trusted><nyaa:remake>No</nyaa:remake>
</item></channel></rss>";
const TOSHO_ONE: &str = r#"[{"title":"[Group] Frieren - 01","info_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","seeders":12,"leechers":3,"total_size":104857600,"is_seadex":false,"anidb_aid":17617,"num_files":1}]"#;

#[derive(Debug, Clone)]
enum Reply {
    Body(String),
    Fail(Error),
}

/// A transport with scripted replies per URL, shared across concurrent indexers.
#[derive(Clone, Default)]
struct MockTransport {
    replies: Arc<std::sync::Mutex<HashMap<String, Vec<Reply>>>>,
    calls: Arc<std::sync::Mutex<Vec<String>>>,
    barrier: Option<Arc<Barrier>>,
}

impl MockTransport {
    fn with(replies: &[(&str, Vec<Reply>)]) -> Self {
        let mut scripted = HashMap::new();
        for (url, replies) in replies {
            scripted.insert((*url).to_owned(), replies.clone());
        }
        // Batch queries are always issued alongside singles; empty by default.
        let batch_nyaa = search::query_url(
            &Query::Nyaa {
                keyword: "frieren batch".to_owned(),
            },
            &Config::default(),
        )
        .unwrap_or_default();
        scripted
            .entry(batch_nyaa)
            .or_insert_with(|| vec![Reply::Body(EMPTY_RSS.to_owned())]);
        let batch_tosho =
            search::query_url(&Query::anime_tosho_batch("frieren"), &Config::default())
                .unwrap_or_default();
        scripted
            .entry(batch_tosho)
            .or_insert_with(|| vec![Reply::Body("[]".to_owned())]);
        Self {
            replies: Arc::new(std::sync::Mutex::new(scripted)),
            calls: Arc::new(std::sync::Mutex::new(Vec::new())),
            barrier: None,
        }
    }

    fn calls(&self) -> Vec<String> {
        if let Ok(calls) = self.calls.lock() {
            calls.clone()
        } else {
            Vec::new()
        }
    }
}

impl Transport for MockTransport {
    async fn fetch(&self, query: &Query) -> Result<Vec<u8>, Error> {
        if let Some(barrier) = &self.barrier {
            barrier.wait().await;
        }
        let url = search::query_url(query, &Config::default())?;
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(url.clone());
        }
        let reply = {
            let Ok(mut replies) = self.replies.lock() else {
                return Err(Error::Transport {
                    indexer: query.indexer(),
                    message: "lock poisoned".to_owned(),
                });
            };
            let queue = replies.get_mut(&url);
            queue.and_then(|queue| {
                if queue.len() > 1 {
                    Some(queue.remove(0))
                } else {
                    queue.first().cloned()
                }
            })
        };
        match reply {
            Some(Reply::Body(body)) => Ok(body.into_bytes()),
            Some(Reply::Fail(error)) => Err(error),
            None => Err(Error::Transport {
                indexer: query.indexer(),
                message: format!("no scripted reply for {url}"),
            }),
        }
    }
}

fn nyaa_url(keyword: &str) -> String {
    search::query_url(
        &Query::Nyaa {
            keyword: keyword.to_owned(),
        },
        &Config::default(),
    )
    .unwrap_or_default()
}

fn tosho_url(keyword: &str) -> String {
    search::query_url(
        &Query::anime_tosho_keyword(keyword, 1, 5),
        &Config::default(),
    )
    .unwrap_or_default()
}

#[tokio::test]
async fn urls_carry_the_indexer_params_and_use_configured_base_urls() {
    let nyaa = nyaa_url("one piece 05");
    assert!(nyaa.starts_with("https://nyaa.si/?"));
    assert!(nyaa.contains("page=rss"));
    assert!(nyaa.contains("c=1_2"));
    assert!(nyaa.contains("q=one+piece+05"));

    let tosho = tosho_url("frieren");
    assert!(tosho_starts_ok(&tosho));
    assert!(tosho.contains("/json?"));
    assert!(tosho.contains("q=frieren"));
    assert!(tosho.contains("season=1"));
    assert!(tosho.contains("ep=5"));
    assert!(tosho.contains("limit=75"));

    let mirror = Config::default().with_nyaa_base_url("nyaa.land");
    let url = search::query_url(
        &Query::Nyaa {
            keyword: "x".to_owned(),
        },
        &mirror,
    )
    .expect("bare host normalizes");
    assert!(url.starts_with("https://nyaa.land/"));
}

fn tosho_starts_ok(url: &str) -> bool {
    url.starts_with("https://feed.animetosho.net/")
}

#[tokio::test]
async fn results_are_deduped_by_infohash_across_indexers() {
    let transport = MockTransport::with(&[
        (
            &nyaa_url("frieren 05"),
            vec![Reply::Body(RSS_ONE.to_owned())],
        ),
        (
            &tosho_url("frieren"),
            vec![Reply::Body(TOSHO_ONE.to_owned())],
        ),
    ]);
    let searcher = Searcher::new(transport.clone());
    let results = searcher
        .search(&SearchRequest::new(vec!["Frieren".to_owned()], 1, 5))
        .await
        .expect("search succeeds");

    assert_eq!(results.torrents.len(), 1, "the shared infohash is deduped");
    assert_eq!(
        results.torrents.first().expect("one result").seeders,
        Some(10),
        "the first indexer result wins"
    );
    assert_eq!(
        transport.calls().len(),
        4,
        "singles and batches on both indexers"
    );
}

#[tokio::test]
async fn keyword_fallback_runs_only_when_numbered_queries_find_nothing() {
    let fallback_url =
        search::query_url(&Query::anime_tosho_fallback("frieren"), &Config::default())
            .expect("valid URL");
    let transport = MockTransport::with(&[
        (
            &nyaa_url("frieren 05"),
            vec![Reply::Body(EMPTY_RSS.to_owned())],
        ),
        (&tosho_url("frieren"), vec![Reply::Body("[]".to_owned())]),
        (
            &search::query_url(
                &Query::Nyaa {
                    keyword: "frieren".to_owned(),
                },
                &Config::default(),
            )
            .expect("valid URL"),
            vec![Reply::Body(RSS_ONE.to_owned())],
        ),
        (&fallback_url, vec![Reply::Body(TOSHO_ONE.to_owned())]),
    ]);
    let searcher = Searcher::new(transport.clone());
    let results = searcher
        .search(&SearchRequest::new(vec!["Frieren".to_owned()], 1, 5))
        .await
        .expect("search succeeds");

    assert_eq!(results.torrents.len(), 1);
    let calls = transport.calls();
    assert!(calls.contains(&fallback_url), "AnimeTosho fallback ran");
}

#[tokio::test]
async fn one_backoff_retry_honours_retry_after() {
    let transport = MockTransport::with(&[
        (
            &nyaa_url("frieren 05"),
            vec![
                Reply::Fail(Error::RateLimited {
                    indexer: Indexer::Nyaa,
                    retry_after: Duration::ZERO,
                }),
                Reply::Body(RSS_ONE.to_owned()),
            ],
        ),
        (&tosho_url("frieren"), vec![Reply::Body("[]".to_owned())]),
        (
            &search::query_url(&Query::anime_tosho_fallback("frieren"), &Config::default())
                .expect("valid URL"),
            vec![Reply::Body("[]".to_owned())],
        ),
        (
            &search::query_url(
                &Query::Nyaa {
                    keyword: "frieren".to_owned(),
                },
                &Config::default(),
            )
            .expect("valid URL"),
            vec![Reply::Body(EMPTY_RSS.to_owned())],
        ),
    ]);
    let searcher = Searcher::new(transport.clone());
    let results = searcher
        .search(&SearchRequest::new(vec!["Frieren".to_owned()], 1, 5))
        .await
        .expect("retry succeeds");

    assert_eq!(results.torrents.len(), 1);
    assert_eq!(
        transport
            .calls()
            .iter()
            .filter(|url| *url == &nyaa_url("frieren 05"))
            .count(),
        2,
        "exactly one retry"
    );
}

#[tokio::test]
async fn a_query_result_is_cached_for_sixty_seconds() {
    let clock = search::FakeClock::default();
    let transport = MockTransport::with(&[
        (
            &nyaa_url("frieren 05"),
            vec![
                Reply::Body(RSS_ONE.to_owned()),
                Reply::Body(EMPTY_RSS.to_owned()),
            ],
        ),
        (
            &tosho_url("frieren"),
            vec![Reply::Body("[]".to_owned()), Reply::Body("[]".to_owned())],
        ),
        (
            &search::query_url(&Query::anime_tosho_fallback("frieren"), &Config::default())
                .expect("valid URL"),
            vec![Reply::Body("[]".to_owned())],
        ),
        (
            &search::query_url(
                &Query::Nyaa {
                    keyword: "frieren".to_owned(),
                },
                &Config::default(),
            )
            .expect("valid URL"),
            vec![Reply::Body(EMPTY_RSS.to_owned())],
        ),
    ]);
    let searcher = Searcher::with_clock(transport.clone(), clock.clone());
    let request = SearchRequest::new(vec!["Frieren".to_owned()], 1, 5);

    assert_eq!(
        searcher
            .search(&request)
            .await
            .expect("first")
            .torrents
            .len(),
        1
    );
    assert_eq!(
        searcher
            .search(&request)
            .await
            .expect("cached")
            .torrents
            .len(),
        1,
        "second search uses the cache"
    );
    assert_eq!(transport.calls().len(), 5);

    clock.advance(Duration::from_secs(61));
    assert_eq!(
        searcher
            .search(&request)
            .await
            .expect("expired")
            .torrents
            .len(),
        0,
        "expired cache refetches"
    );
    assert_eq!(transport.calls().len(), 11);
}

fn tosho_fallback_url(keyword: &str) -> String {
    search::query_url(&Query::anime_tosho_fallback(keyword), &Config::default()).unwrap_or_default()
}

const TOSHO_MIXED: &str = r#"[{"title":"[Group] Frieren - 05","info_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","seeders":999},{"title":"[Group] Frieren - 05 [1080p]","info_hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","seeders":12}]"#;

#[tokio::test]
async fn dedupe_keeps_lane_order_with_the_first_result_winning() {
    let transport = MockTransport::with(&[
        (
            &nyaa_url("frieren 05"),
            vec![Reply::Body(RSS_ONE.to_owned())],
        ),
        (
            &tosho_url("frieren"),
            vec![Reply::Body(TOSHO_MIXED.to_owned())],
        ),
    ]);
    let searcher = Searcher::new(transport);

    let outcome = searcher
        .search(&SearchRequest::new(vec!["Frieren".to_owned()], 1, 5))
        .await
        .expect("search succeeds");

    assert_eq!(outcome.torrents.len(), 2, "one duplicate is dropped");
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
        "the first result wins the duplicate"
    );
    assert_eq!(
        outcome
            .torrents
            .get(1)
            .map(|torrent| torrent.infohash.as_str()),
        Some("b".repeat(40).as_str())
    );
    assert!(outcome.failures.is_empty());
}

#[tokio::test]
async fn a_failed_lane_returns_the_other_lanes_results_and_a_failure() {
    let transport = MockTransport::with(&[
        (
            &nyaa_url("frieren"),
            vec![Reply::Fail(Error::Http {
                indexer: Indexer::Nyaa,
                status: 500,
            })],
        ),
        (
            &tosho_fallback_url("frieren"),
            vec![Reply::Body(TOSHO_ONE.to_owned())],
        ),
    ]);
    let searcher = Searcher::new(transport);

    let outcome = searcher
        .search_keyword("frieren", false)
        .await
        .expect("one lane still answers");

    assert_eq!(outcome.torrents.len(), 1, "AnimeTosho's result survives");
    assert_eq!(outcome.failures.len(), 1, "one failed lane is reported");
    assert_eq!(
        outcome
            .failures
            .first()
            .map(|failure| failure.indexer.clone()),
        Some(Indexer::Nyaa)
    );
}

#[tokio::test]
async fn a_search_fails_when_every_lane_fails() {
    let transport = MockTransport::with(&[
        (
            &nyaa_url("frieren"),
            vec![Reply::Fail(Error::Http {
                indexer: Indexer::Nyaa,
                status: 500,
            })],
        ),
        (
            &tosho_fallback_url("frieren"),
            vec![Reply::Fail(Error::Http {
                indexer: Indexer::AnimeTosho,
                status: 500,
            })],
        ),
    ]);
    let searcher = Searcher::new(transport);

    let error = searcher
        .search_keyword("frieren", false)
        .await
        .expect_err("every lane failed");

    assert!(matches!(error, Error::AllLanesFailed), "{error:?}");
}

#[tokio::test]
async fn indexers_are_queried_concurrently() {
    let barrier = Arc::new(Barrier::new(2));
    let mut transport = MockTransport::with(&[
        (
            &nyaa_url("frieren 05"),
            vec![Reply::Body(RSS_ONE.to_owned())],
        ),
        (
            &tosho_url("frieren"),
            vec![Reply::Body(TOSHO_ONE.to_owned())],
        ),
    ]);
    transport.barrier = Some(barrier);
    let searcher = Searcher::new(transport.clone());

    let results = tokio::time::timeout(
        Duration::from_secs(2),
        searcher.search(&SearchRequest::new(vec!["Frieren".to_owned()], 1, 5)),
    )
    .await
    .expect("indexers did not block on each other")
    .expect("search succeeds");
    assert_eq!(results.torrents.len(), 1);
}
