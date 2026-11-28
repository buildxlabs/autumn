//! `autumn.http.fetch` against loopback servers (#305).
//!
//! `net`'s test hooks let `api.test` and `other.test` resolve to 127.0.0.1;
//! only `api.test` is declared, so `other.test` is refused by the guard.

#![expect(clippy::unwrap_used, reason = "test setup panics when it fails")]

use std::{
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};

use axum::{
    Router,
    extract::RawQuery,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use domain::ExtensionId;
use extensions::{ExtensionError, ExtensionHost, ProviderRequest, ProviderSource};
use net::{DeclaredHost, GuardedClient};
use tokio::{io::AsyncReadExt, net::TcpListener, sync::oneshot};

fn client() -> GuardedClient {
    let loopback = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
    GuardedClient::builder()
        .allow_loopback()
        .static_dns([("api.test", loopback.clone()), ("other.test", loopback)])
        .build()
        .unwrap()
}

/// A host running one provider that may reach `api.test`.
fn host(lua: &str) -> ExtensionHost {
    host_as("fetcher", lua)
}

/// [`host`] under another id, which gets its own rate-limit bucket.
fn host_as(id: &str, lua: &str) -> ExtensionHost {
    let source = ProviderSource {
        id: ExtensionId::new(id),
        name: "Fetcher".to_owned(),
        version: "1.0.0".to_owned(),
        source: lua.to_owned(),
        hosts: vec![DeclaredHost::parse("api.test").unwrap()],
    };
    ExtensionHost::start(vec![source], &client())
}

/// Calls `search` with `keyword` and returns the first row's name.
async fn name(host: &ExtensionHost, keyword: &str) -> Result<String, ExtensionError> {
    let id = host.providers().next().map(|info| info.id.clone()).unwrap();
    let rows = host.search(&id, ProviderRequest::keyword(keyword)).await?;
    Ok(rows.into_iter().next().and_then(|row| row.name).unwrap())
}

/// Serves `/search` (echoes the query, sets cookies and a repeated header)
/// and `/missing` (404) on a random port; returns `http://api.test:<port>`.
async fn serve() -> String {
    async fn search(RawQuery(query): RawQuery, request: HeaderMap) -> impl IntoResponse {
        let mut headers = HeaderMap::new();
        headers.append(header::SET_COOKIE, "a=1; Path=/".parse().unwrap());
        headers.append(header::SET_COOKIE, "b=2".parse().unwrap());
        headers.append("X-Tag", "one".parse().unwrap());
        headers.append("X-Tag", "two".parse().unwrap());
        let agent = request
            .get("x-client")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-");
        let body = format!("query={} client={agent}", query.unwrap_or_default());
        (headers, body)
    }
    let app = Router::new().route("/search", get(search)).route(
        "/missing",
        get(|| async { (StatusCode::NOT_FOUND, "nope") }),
    );
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://api.test:{port}")
}

#[tokio::test]
async fn a_get_with_a_query_returns_the_response_table() {
    let base = serve().await;
    let lua = format!(
        r#"
        return {{ search = function(request)
            local res = autumn.http.fetch({{
                url = "{base}/search",
                query = {{ q = request.keyword, page = 2 }},
                headers = {{ ["X-Client"] = "autumn-test" }},
            }})
            return {{ {{ name = table.concat({{
                tostring(res.status), res.url, res.body, res.headers["x-tag"],
                table.concat(res.set_cookies, "|"), tostring(res.headers["content-type"]),
            }}, " ; ") }} }}
        end }}
        "#
    );
    let got = name(&host(&lua), "one piece").await.unwrap();
    assert_eq!(
        got,
        format!(
            "200 ; {base}/search?page=2&q=one+piece ; query=page=2&q=one+piece client=autumn-test ; \
             one, two ; a=1; Path=/|b=2 ; text/plain; charset=utf-8"
        )
    );
}

#[tokio::test]
async fn a_404_is_a_status_not_an_error() {
    let base = serve().await;
    let lua = format!(
        r#"return {{ search = function()
            local res = autumn.http.fetch({{ url = "{base}/missing" }})
            return {{ {{ name = res.status .. " " .. res.body }} }}
        end }}"#
    );
    assert_eq!(name(&host(&lua), "x").await.unwrap(), "404 nope");
}

#[tokio::test]
async fn an_undeclared_host_raises_blocked() {
    let base = serve().await;
    let other = base.replace("api.test", "other.test");
    let caught = format!(
        r#"return {{ search = function()
            local ok, err = pcall(autumn.http.fetch, {{ url = "{other}/search" }})
            local kind = if string.find(err, ": blocked: ", 1, true) then "blocked" else err
            return {{ {{ name = tostring(ok) .. " " .. kind .. " | " .. err }} }}
        end }}"#
    );
    let got = name(&host(&caught), "x").await.unwrap();
    assert!(
        got.starts_with("false blocked | autumn.http.fetch: blocked: other.test"),
        "{got}"
    );

    let uncaught = format!(
        r#"return {{ search = function()
            autumn.http.fetch({{ url = "{other}/search" }})
            return {{}}
        end }}"#
    );
    let error = name(&host(&uncaught), "x").await.unwrap_err();
    assert!(
        matches!(&error, ExtensionError::NetworkBlocked(message)
            if message.starts_with("autumn.http.fetch: blocked: other.test")),
        "an uncaught block is typed for health: {error:?}"
    );
}

#[tokio::test]
async fn bad_requests_raise_invalid_argument() {
    let lua = r"return { search = function(request)
        local ok, err = pcall(autumn.http.fetch, load_case(request.keyword))
        return { { name = err } }
    end }";
    let cases = [
        ("{}", "autumn.http.fetch: invalid_argument: url: missing"),
        (
            "{ url = 5 }",
            "autumn.http.fetch: invalid_argument: url: expected string, got integer",
        ),
        (
            "{ url = 'nope' }",
            "autumn.http.fetch: invalid_argument: url: relative URL without a base",
        ),
        (
            "{ url = 'https://api.test', method = 'TRACE' }",
            "autumn.http.fetch: invalid_argument: method: \"TRACE\" is not GET, POST, PUT, PATCH or DELETE",
        ),
        (
            "{ url = 'https://api.test', query = { q = {} } }",
            "autumn.http.fetch: invalid_argument: query.q: expected string, number or boolean, got table",
        ),
        (
            "{ url = 'https://api.test', headers = { ['bad name'] = 'x' } }",
            "autumn.http.fetch: invalid_argument: headers.bad name: ",
        ),
    ];
    for (request, expected) in cases {
        let source = format!("local function load_case() return {request} end\n{lua}");
        let got = name(&host(&source), "x").await.unwrap();
        assert!(got.starts_with(expected), "{request}: {got}");
    }
}

#[tokio::test]
async fn the_ninth_fetch_kills_the_call_even_under_pcall() {
    let base = serve().await;
    // Each extension's rate limit bursts to 10, so each host stays under it.
    let lua = format!(
        r#"
        local calls = 0
        return {{ search = function(request)
            calls += 1
            local fetched = 0
            for _ = 1, tonumber(request.keyword) do
                pcall(autumn.http.fetch, {{ url = "{base}/missing" }})
                fetched += 1
            end
            return {{ {{ name = calls .. ":" .. fetched }} }}
        end }}
        "#
    );
    let eight = host_as("eight", &lua);
    assert_eq!(
        name(&eight, "8").await.unwrap(),
        "1:8",
        "8 requests are allowed"
    );

    let nine = host_as("nine", &lua);
    assert_eq!(name(&nine, "9").await, Err(ExtensionError::RequestLimit));
    assert_eq!(name(&nine, "0").await.unwrap(), "1:0", "the VM was rebuilt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_call_drops_its_request() {
    // A bare TCP server that never answers and reports when the client hangs up.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (accepted_tx, accepted) = oneshot::channel();
    let (closed_tx, closed) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0_u8; 1024];
        let _ = socket.read(&mut buffer).await.unwrap();
        accepted_tx.send(()).unwrap();
        while socket.read(&mut buffer).await.unwrap() > 0 {}
        closed_tx.send(()).unwrap();
    });
    let lua = format!(
        r#"return {{ search = function()
            autumn.http.fetch({{ url = "http://api.test:{port}/hang" }})
            return {{}}
        end }}"#
    );
    let host = host(&lua);
    let call = tokio::spawn({
        let host = host.clone();
        async move { name(&host, "x").await }
    });
    accepted.await.unwrap();
    call.abort();
    tokio::time::timeout(Duration::from_secs(5), closed)
        .await
        .expect("the server saw the connection close")
        .unwrap();
}
