//! The outbound guard on extension traffic (ADR 0015), against loopback servers only.
//!
//! Names resolve through the guard's static DNS table, so no test reaches a
//! live host. `allow_loopback` stands in for a public address where a test
//! needs a request to land.

#![expect(clippy::unwrap_used, reason = "test harness panics on setup failure")]
#![expect(
    clippy::panic,
    reason = "a test fails by panicking with the unexpected outcome"
)]

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{
    Router,
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use net::{
    AddressClass, BlockReason, DeclaredHost, Error, ExtensionHosts, GuardedClient,
    LanHostException, Method, Request,
};
use tokio::{net::TcpListener, task::JoinHandle};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const PUBLIC: IpAddr = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
const PRIVATE: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
const METADATA: IpAddr = IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254));

#[derive(Debug, Clone)]
struct Fixture {
    port: u16,
    hits: Arc<AtomicUsize>,
}

struct Server {
    port: u16,
    hits: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl Server {
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn ok(State(fixture): State<Fixture>) -> &'static str {
    fixture.hits.fetch_add(1, Ordering::SeqCst);
    "ok"
}

async fn to(State(fixture): State<Fixture>, Path(target): Path<String>) -> Response {
    fixture.hits.fetch_add(1, Ordering::SeqCst);
    let port = fixture.port;
    let location = match target.as_str() {
        "undeclared" => format!("http://other.test:{port}/ok"),
        "private" => format!("http://10.0.0.1:{port}/ok"),
        "mapped" => format!("http://[::ffff:127.0.0.1]:{port}/ok"),
        "localhost" => format!("http://localhost:{port}/ok"),
        "ftp" => "ftp://api.test/ok".to_owned(),
        _ => format!("http://api.test:{port}/ok"),
    };
    Redirect::temporary(&location).into_response()
}

/// `/chain/n` redirects n more times, then answers `ok`.
async fn chain(State(fixture): State<Fixture>, Path(remaining): Path<u32>) -> Response {
    fixture.hits.fetch_add(1, Ordering::SeqCst);
    match remaining.checked_sub(1) {
        Some(next) => Redirect::temporary(&format!("/chain/{next}")).into_response(),
        None => "ok".into_response(),
    }
}

async fn serve() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/ok", get(ok))
        .route("/to/{target}", get(to))
        .route("/chain/{remaining}", get(chain))
        .with_state(Fixture {
            port,
            hits: Arc::clone(&hits),
        });
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server { port, hits, task }
}

fn dns() -> Vec<(&'static str, Vec<IpAddr>)> {
    vec![
        ("api.test", vec![LOOPBACK]),
        ("other.test", vec![LOOPBACK]),
        ("nas.local", vec![LOOPBACK]),
        ("mixed.test", vec![PUBLIC, PRIVATE]),
        ("metadata.test", vec![METADATA]),
    ]
}

/// A guarded client where loopback stands in for a public address.
fn fixture_client() -> GuardedClient {
    GuardedClient::builder()
        .allow_loopback()
        .static_dns(dns())
        .build()
        .unwrap()
}

/// A guarded client with no test override: loopback is blocked.
fn strict_client() -> GuardedClient {
    GuardedClient::builder().static_dns(dns()).build().unwrap()
}

fn hosts(declared: &[&str]) -> ExtensionHosts {
    let declared = declared
        .iter()
        .map(|pattern| DeclaredHost::parse(pattern).unwrap())
        .collect();
    ExtensionHosts::new("ext.test", declared)
}

async fn get_body(client: &GuardedClient, hosts: &ExtensionHosts, url: &str) -> String {
    let response = client
        .send(hosts, Request::new(Method::Get, url))
        .await
        .unwrap();
    response.text().await.unwrap()
}

async fn blocked(
    client: &GuardedClient,
    hosts: &ExtensionHosts,
    url: &str,
) -> (String, BlockReason) {
    match client.send(hosts, Request::new(Method::Get, url)).await {
        Err(Error::Blocked { host, reason }) => (host, reason),
        Err(other) => panic!("{url}: expected a block, got {other}"),
        Ok(response) => panic!("{url}: expected a block, got HTTP {}", response.status()),
    }
}

#[tokio::test]
async fn a_declared_host_on_the_fixture_override_is_served() {
    let server = serve().await;
    let body = get_body(
        &fixture_client(),
        &hosts(&["api.test"]),
        &format!("http://api.test:{}/ok", server.port),
    )
    .await;
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn loopback_is_blocked_without_the_test_override() {
    let server = serve().await;
    let port = server.port;
    let client = strict_client();
    let declared = hosts(&["api.test", "127.0.0.1"]);

    assert_eq!(
        blocked(&client, &declared, &format!("http://127.0.0.1:{port}/ok")).await,
        (
            "127.0.0.1".to_owned(),
            BlockReason::Address(AddressClass::Loopback)
        )
    );
    // Through the resolver: the name is declared, its address is not allowed.
    assert_eq!(
        blocked(&client, &declared, &format!("http://api.test:{port}/ok")).await,
        (
            "api.test".to_owned(),
            BlockReason::Address(AddressClass::Loopback)
        )
    );
    assert_eq!(server.hits(), 0, "no request reached the server");
}

#[tokio::test]
async fn ip_literals_in_every_spelling_are_checked_before_sending() {
    let client = fixture_client();
    let declared = hosts(&["api.test"]);
    let table = [
        ("http://10.0.0.1/", "10.0.0.1", AddressClass::Private),
        ("http://0x0a000001/", "10.0.0.1", AddressClass::Private),
        ("http://167772161/", "10.0.0.1", AddressClass::Private),
        ("http://012.0.0.1/", "10.0.0.1", AddressClass::Private),
        ("http://0.0.0.0/", "0.0.0.0", AddressClass::Reserved),
        ("http://100.64.0.1/", "100.64.0.1", AddressClass::Private),
        (
            "http://169.254.169.254/",
            "169.254.169.254",
            AddressClass::LinkLocal,
        ),
        (
            "http://224.0.0.251/",
            "224.0.0.251",
            AddressClass::Multicast,
        ),
        (
            "http://255.255.255.255/",
            "255.255.255.255",
            AddressClass::Reserved,
        ),
        ("http://[::]/", "::", AddressClass::Reserved),
        ("http://[fd00::1]/", "fd00::1", AddressClass::Private),
        ("http://[fe80::1]/", "fe80::1", AddressClass::LinkLocal),
        ("http://[ff02::fb]/", "ff02::fb", AddressClass::Multicast),
        (
            "http://[::ffff:10.0.0.1]/",
            "::ffff:10.0.0.1",
            AddressClass::Private,
        ),
        ("http://[::10.0.0.1]/", "::a00:1", AddressClass::Private),
        (
            "http://[64:ff9b::a00:1]/",
            "64:ff9b::a00:1",
            AddressClass::Private,
        ),
        (
            "http://[2002:a00:1::1]/",
            "2002:a00:1::1",
            AddressClass::Private,
        ),
    ];
    for (url, host, class) in table {
        assert_eq!(
            blocked(&client, &declared, url).await,
            (host.to_owned(), BlockReason::Address(class)),
            "{url}"
        );
    }
}

#[tokio::test]
async fn local_names_are_blocked_even_when_declared() {
    let client = fixture_client();
    let declared = hosts(&["localhost", "foo.localhost", "nas.local"]);
    for (url, host) in [
        ("http://localhost/", "localhost"),
        ("http://LOCALHOST./", "localhost"),
        ("http://foo.localhost/", "foo.localhost"),
        ("http://nas.local/", "nas.local"),
        ("http://NAS.Local./", "nas.local"),
    ] {
        assert_eq!(
            blocked(&client, &declared, url).await,
            (host.to_owned(), BlockReason::LocalName),
            "{url}"
        );
    }
}

#[tokio::test]
async fn only_http_and_https_are_allowed() {
    let client = fixture_client();
    let declared = hosts(&["api.test"]);
    assert_eq!(
        blocked(&client, &declared, "ftp://api.test/file").await,
        ("api.test".to_owned(), BlockReason::Scheme)
    );
    assert_eq!(
        blocked(&client, &declared, "file:///etc/passwd").await.1,
        BlockReason::Scheme
    );
}

#[tokio::test]
async fn an_undeclared_host_is_refused_with_a_readable_reason() {
    let server = serve().await;
    let error = fixture_client()
        .send(
            &hosts(&["api.test", "*.cdn.example.com"]),
            Request::new(Method::Get, format!("http://other.test:{}/ok", server.port)),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "blocked: other.test is not a declared host"
    );
    assert_eq!(server.hits(), 0, "refused before connecting");
}

#[tokio::test]
async fn a_name_resolving_to_one_public_and_one_private_address_is_rejected() {
    let declared = hosts(&["mixed.test"]);
    assert_eq!(
        blocked(&fixture_client(), &declared, "http://mixed.test/").await,
        (
            "mixed.test".to_owned(),
            BlockReason::Address(AddressClass::Private)
        )
    );
}

#[tokio::test]
async fn redirects_are_rechecked_on_every_hop() {
    let server = serve().await;
    let port = server.port;
    let client = fixture_client();
    let declared = hosts(&["api.test"]);
    let at = |path: &str| format!("http://api.test:{port}{path}");

    assert_eq!(
        get_body(&client, &declared, &at("/to/declared")).await,
        "ok"
    );
    let table = [
        ("/to/undeclared", "other.test", BlockReason::Undeclared),
        (
            "/to/private",
            "10.0.0.1",
            BlockReason::Address(AddressClass::Private),
        ),
        ("/to/localhost", "localhost", BlockReason::LocalName),
        ("/to/ftp", "api.test", BlockReason::Scheme),
    ];
    for (path, host, reason) in table {
        assert_eq!(
            blocked(&client, &declared, &at(path)).await,
            (host.to_owned(), reason),
            "{path}"
        );
    }
}

#[tokio::test]
async fn ten_redirect_hops_are_followed_and_an_eleventh_is_refused() {
    let server = serve().await;
    let port = server.port;
    let client = fixture_client();
    let declared = hosts(&["api.test"]);

    let body = get_body(
        &client,
        &declared,
        &format!("http://api.test:{port}/chain/10"),
    )
    .await;
    assert_eq!(body, "ok");
    assert_eq!(
        blocked(
            &client,
            &declared,
            &format!("http://api.test:{port}/chain/11")
        )
        .await
        .1,
        BlockReason::TooManyRedirects
    );
}

#[tokio::test]
async fn a_lan_host_exception_allows_only_that_host_and_port() {
    let server = serve().await;
    let port = server.port;
    let client = strict_client();
    let with = |exception: &str| {
        hosts(&["api.test"]).with_lan_exceptions(vec![LanHostException::parse(exception).unwrap()])
    };

    let nas = format!("http://nas.local:{port}/ok");
    assert_eq!(
        get_body(&client, &with(&format!("nas.local:{port}")), &nas).await,
        "ok"
    );
    assert_eq!(get_body(&client, &with("nas.local"), &nas).await, "ok");
    assert_eq!(
        blocked(&client, &with("nas.local:1"), &nas).await,
        ("nas.local".to_owned(), BlockReason::LocalName)
    );
    // An exception for one host does not open another loopback name.
    assert_eq!(
        blocked(
            &client,
            &with("nas.local"),
            &format!("http://api.test:{port}/ok")
        )
        .await,
        (
            "api.test".to_owned(),
            BlockReason::Address(AddressClass::Loopback)
        )
    );
    // A literal IP exception allows exactly that address.
    let literal = format!("http://127.0.0.1:{port}/ok");
    assert_eq!(
        get_body(&client, &with(&format!("127.0.0.1:{port}")), &literal).await,
        "ok"
    );
}

#[tokio::test]
async fn a_lan_host_exception_by_name_never_reaches_link_local() {
    let declared =
        hosts(&[]).with_lan_exceptions(vec![LanHostException::parse("metadata.test").unwrap()]);
    assert_eq!(
        blocked(&strict_client(), &declared, "http://metadata.test/").await,
        (
            "metadata.test".to_owned(),
            BlockReason::Address(AddressClass::LinkLocal)
        )
    );
}

#[tokio::test]
async fn extensions_do_not_share_declared_hosts() {
    let server = serve().await;
    let url = format!("http://api.test:{}/ok", server.port);
    let client = fixture_client();
    let first = ExtensionHosts::new("first", vec![DeclaredHost::parse("api.test").unwrap()]);
    let second = ExtensionHosts::new("second", vec![DeclaredHost::parse("other.test").unwrap()]);

    assert_eq!(get_body(&client, &first, &url).await, "ok");
    assert_eq!(
        blocked(&client, &second, &url).await.1,
        BlockReason::Undeclared
    );
    // An update that declares the host takes effect on the next request.
    let widened = ExtensionHosts::new(
        "second",
        vec![
            DeclaredHost::parse("other.test").unwrap(),
            DeclaredHost::parse("api.test").unwrap(),
        ],
    );
    assert_eq!(get_body(&client, &widened, &url).await, "ok");
}

#[tokio::test]
async fn under_an_http_proxy_url_rules_still_hold_and_the_proxy_resolves_targets() {
    let proxy = serve().await;
    // The proxy answers every path; `/ok` stands for the proxied target's reply.
    let client = GuardedClient::builder()
        .static_dns([("proxy.test", vec![LOOPBACK])])
        .proxy(format!("http://proxy.test:{}", proxy.port))
        .unwrap()
        .build()
        .unwrap();
    let declared = hosts(&["public.test"]);

    // `public.test` is not in the DNS table: only the proxy resolves it.
    assert_eq!(
        get_body(&client, &declared, "http://public.test/ok").await,
        "ok"
    );
    assert_eq!(
        blocked(&client, &declared, "http://undeclared.test/ok")
            .await
            .1,
        BlockReason::Undeclared
    );
    assert_eq!(
        blocked(&client, &declared, "http://10.0.0.1/ok").await.1,
        BlockReason::Address(AddressClass::Private)
    );
    assert_eq!(
        blocked(&client, &declared, "http://localhost/ok").await.1,
        BlockReason::LocalName
    );
    assert_eq!(
        proxy.hits(),
        1,
        "only the allowed request reached the proxy"
    );
}
