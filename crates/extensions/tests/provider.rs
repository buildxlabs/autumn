//! A Luau torrent provider run end to end through `ExtensionHost` (#296).

#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test helpers panic when the case under test fails"
)]

use std::{
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use domain::ExtensionId;
use extensions::{
    EpisodeQuery, ExtensionError, ExtensionHost, ProviderRequest, ProviderSource, ProviderTorrent,
};

const MAGNET: &str = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";

fn source(id: &str, lua: &str) -> ProviderSource {
    ProviderSource {
        id: ExtensionId::new(id),
        name: format!("{id} name"),
        version: "1.0.0".to_owned(),
        source: lua.to_owned(),
    }
}

/// Runs `search` once on a host holding just this provider.
async fn run(lua: &str, request: ProviderRequest) -> Result<Vec<ProviderTorrent>, ExtensionError> {
    let host = ExtensionHost::start(vec![source("my-ext", lua)]);
    host.search(&ExtensionId::new("my-ext"), request).await
}

/// The `name` of the single row a provider returned.
async fn single_name(lua: &str, request: ProviderRequest) -> String {
    let rows = run(lua, request).await.unwrap();
    assert_eq!(rows.len(), 1, "one row: {rows:?}");
    rows.into_iter().next().and_then(|row| row.name).unwrap()
}

#[tokio::test]
async fn a_fixed_magnet_row_round_trips() {
    let lua = format!(
        r#"
        return {{
            search = function(request)
                return {{ {{
                    name = "[Group] Show - 01 (1080p).mkv",
                    magnet_url = "{MAGNET}",
                    size = 1073741824,
                    seeders = 12,
                    leechers = 3,
                    downloads = 40,
                    published = 1700000000,
                    is_batch = false,
                    anidb_eid = 271828,
                    num_files = 1,
                    files = {{ "Show - 01.mkv" }},
                    trusted = true,
                }} }}
            end,
            unknown_export = 42,
        }}
        "#
    );
    let rows = run(&lua, ProviderRequest::keyword("show")).await.unwrap();
    assert_eq!(
        rows,
        vec![ProviderTorrent {
            name: Some("[Group] Show - 01 (1080p).mkv".to_owned()),
            magnet_url: Some(MAGNET.to_owned()),
            size: Some(1_073_741_824.0),
            seeders: Some(12.0),
            leechers: Some(3.0),
            downloads: Some(40.0),
            published: Some(1_700_000_000.0),
            is_batch: Some(false),
            anidb_eid: Some(271_828.0),
            num_files: Some(1.0),
            files: Some(vec!["Show - 01.mkv".to_owned()]),
            ..ProviderTorrent::default()
        }],
        "unknown row fields and provider keys are ignored"
    );
}

#[tokio::test]
async fn the_host_lists_its_providers() {
    let host = ExtensionHost::start(vec![
        source("a", "return {}"),
        source("b", "return {}"),
        source("a", "return {}"),
    ]);
    let ids: Vec<_> = host
        .providers()
        .map(|info| (info.id.as_str(), info.name.as_str(), info.version.as_str()))
        .collect();
    assert_eq!(
        ids,
        [("a", "a name", "1.0.0"), ("b", "b name", "1.0.0")],
        "duplicates are skipped"
    );
    let missing = host
        .search(&ExtensionId::new("c"), ProviderRequest::keyword("x"))
        .await;
    assert_eq!(
        missing,
        Err(ExtensionError::UnknownExtension(ExtensionId::new("c")))
    );
}

#[tokio::test]
async fn sandbox_removes_require_loadstring_and_fenv() {
    let lua = r#"
        return {
            search = function()
                return { { name = table.concat({
                    typeof(require), typeof(loadstring), typeof(getfenv), typeof(setfenv),
                }, ",") } }
            end,
        }
    "#;
    let name = single_name(lua, ProviderRequest::keyword("x")).await;
    assert_eq!(name, "nil,nil,nil,nil");
}

/// Collects formatted `tracing` output in memory.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn print_goes_to_tracing_tagged_with_the_extension() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    // The current-thread test runtime polls the actor on this thread.
    let _guard = tracing::subscriber::set_default(subscriber);

    let lua = r#"
        print("loaded")
        return { search = function() print("searching", 1, true); return {} end }
    "#;
    assert_eq!(run(lua, ProviderRequest::keyword("x")).await, Ok(vec![]));

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("extension: loaded extension=my-ext"),
        "load-time print is logged: {log}"
    );
    assert!(
        log.contains("extension: searching\t1\ttrue extension=my-ext"),
        "arguments are tab-joined: {log}"
    );
}

#[tokio::test]
async fn bad_chunk_shapes_are_invalid_providers() {
    let cases = [
        ("return nil", "provider: expected table, got nil"),
        ("return 'search'", "provider: expected table, got string"),
        ("return {}", "search: expected function, got nil"),
        (
            "return { search = 5 }",
            "search: expected function, got integer",
        ),
        (
            "return { search = {} }",
            "search: expected function, got table",
        ),
    ];
    for (lua, expected) in cases {
        assert_eq!(
            run(lua, ProviderRequest::keyword("x")).await,
            Err(ExtensionError::InvalidProvider(expected.to_owned())),
            "{lua}"
        );
    }
}

/// The message and traceback of a `Runtime` error.
fn runtime(result: Result<Vec<ProviderTorrent>, ExtensionError>) -> (String, String) {
    match result {
        Err(ExtensionError::Runtime { message, traceback }) => (message, traceback),
        other => panic!("expected Runtime, got {other:?}"),
    }
}

#[tokio::test]
async fn an_uncaught_error_is_runtime_with_an_id_line_frame() {
    let lua = "local function fail()\n    error('site changed')\nend\nreturn { search = function() fail() end }\n";
    let (message, traceback) = runtime(run(lua, ProviderRequest::keyword("x")).await);
    assert_eq!(message, "my-ext:2: site changed");
    assert!(
        traceback.contains("my-ext:2"),
        "the frame names the chunk and line: {traceback}"
    );
    assert!(
        traceback.contains("my-ext:4"),
        "the caller frame is kept: {traceback}"
    );
}

#[tokio::test]
async fn a_table_error_uses_its_message_field() {
    let lua = "return { search = function() error({ message = 'rate limited', code = 429 }) end }";
    let (message, _) = runtime(run(lua, ProviderRequest::keyword("x")).await);
    assert_eq!(message, "rate limited");
}

#[tokio::test]
async fn a_raising_or_broken_chunk_is_a_runtime_error() {
    let (message, traceback) = runtime(run("error('boom')", ProviderRequest::keyword("x")).await);
    assert_eq!(message, "my-ext:1: boom");
    assert!(traceback.contains("my-ext:1"), "{traceback}");

    let (message, traceback) = runtime(run("return {", ProviderRequest::keyword("x")).await);
    assert!(message.starts_with("my-ext:1:"), "{message}");
    assert_eq!(traceback, "", "a compile error has no traceback");
}

#[tokio::test]
async fn a_non_array_return_is_an_invalid_result() {
    let cases = [
        ("'rows'", "results: expected array, got string"),
        ("nil", "results: expected array, got nil"),
        (
            "{ name = 'x' }",
            "results: expected array, got table with key \"name\"",
        ),
        (
            "{ [1] = {}, [3] = {} }",
            "results: expected array, got table with key 3",
        ),
        (
            "{ [1.5] = {} }",
            "results: expected array, got table with key 1.5",
        ),
    ];
    for (results, expected) in cases {
        let lua = format!("return {{ search = function() return {results} end }}");
        assert_eq!(
            run(&lua, ProviderRequest::keyword("x")).await,
            Err(ExtensionError::InvalidResult(expected.to_owned())),
            "{results}"
        );
    }
    let empty = "return { search = function() return {} end }";
    assert_eq!(run(empty, ProviderRequest::keyword("x")).await, Ok(vec![]));
}

#[tokio::test]
async fn wrong_typed_fields_and_rows_decode_as_absent() {
    let lua = r#"
        return { search = function()
            return {
                { name = "ok", seeders = "12", is_batch = 1, files = { "a.mkv", 5, "b.mkv" } },
                "not a row",
            }
        end }
    "#;
    let rows = run(lua, ProviderRequest::keyword("x")).await.unwrap();
    assert_eq!(
        rows,
        vec![
            ProviderTorrent {
                name: Some("ok".to_owned()),
                files: Some(vec!["a.mkv".to_owned(), "b.mkv".to_owned()]),
                ..ProviderTorrent::default()
            },
            ProviderTorrent::default(),
        ]
    );
}

#[tokio::test]
async fn requests_encode_as_snake_case_tables_with_nil_for_absent() {
    let lua = r#"
        return { search = function(r)
            local titles = if r.titles then table.concat(r.titles, "|") else "-"
            return { { name = table.concat({
                r.keyword, titles, tostring(r.season), tostring(r.episode),
                tostring(r.absolute_episode), tostring(r.anidb_aid), tostring(r.anidb_eid),
            }, ",") } }
        end }
    "#;
    let keyword = single_name(lua, ProviderRequest::keyword("frieren")).await;
    assert_eq!(keyword, "frieren,-,nil,nil,nil,nil,nil");

    let structured = ProviderRequest {
        keyword: "Sousou no Frieren".to_owned(),
        episode: Some(EpisodeQuery {
            titles: vec!["Sousou no Frieren".to_owned(), "Frieren".to_owned()],
            season: 1,
            episode: 0,
        }),
        absolute_episode: Some(12),
        anidb_aid: Some(17_617),
        anidb_eid: None,
    };
    let name = single_name(lua, structured).await;
    assert_eq!(
        name,
        "Sousou no Frieren,Sousou no Frieren|Frieren,1,0,12,17617,nil"
    );
}

/// A provider that spins for `seconds` of wall time when asked to.
fn spinner(seconds: f64) -> String {
    format!(
        r#"
        local calls = 0
        return {{ search = function(request)
            calls += 1
            if request.keyword == "spin" then
                local start = os.clock()
                while os.clock() - start < {seconds} do end
            end
            return {{ {{ name = tostring(calls) }} }}
        end }}
        "#
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_busy_extension_does_not_delay_another() {
    let host = ExtensionHost::start(vec![
        source("slow", &spinner(2.0)),
        source("fast", &spinner(0.0)),
    ]);
    let slow = tokio::spawn({
        let host = host.clone();
        async move {
            host.search(&ExtensionId::new("slow"), ProviderRequest::keyword("spin"))
                .await
        }
    });
    // Let the slow call start spinning first.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let started = Instant::now();
    let fast = host
        .search(&ExtensionId::new("fast"), ProviderRequest::keyword("x"))
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "fast answered in {:?}",
        started.elapsed()
    );
    assert_eq!(fast.len(), 1);
    assert!(!slow.is_finished(), "the slow call is still spinning");
    assert_eq!(slow.await.unwrap().map(|rows| rows.len()), Ok(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_queued_call_never_reaches_the_vm() {
    let host = ExtensionHost::start(vec![source("my-ext", &spinner(0.5))]);
    let id = ExtensionId::new("my-ext");
    let first = tokio::spawn({
        let host = host.clone();
        let id = id.clone();
        async move { host.search(&id, ProviderRequest::keyword("spin")).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Queues behind the spinning call, then gives up.
    let dropped = tokio::time::timeout(
        Duration::from_millis(50),
        host.search(&id, ProviderRequest::keyword("x")),
    )
    .await;
    assert!(dropped.is_err(), "the second call was still queued");

    let first = first.await.unwrap().unwrap();
    let third = host
        .search(&id, ProviderRequest::keyword("x"))
        .await
        .unwrap();
    assert_eq!(first.first().and_then(|row| row.name.as_deref()), Some("1"));
    assert_eq!(
        third.first().and_then(|row| row.name.as_deref()),
        Some("2"),
        "the dropped call was skipped"
    );
}
