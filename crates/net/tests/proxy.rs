//! One global outbound proxy on the shared client (ADR-0010), loopback only.

#![expect(clippy::unwrap_used, reason = "test harness panics on setup failure")]

use axum::{Router, response::IntoResponse};
use net::{Client, Method, Request};
use tokio::{net::TcpListener, task::JoinHandle};

fn marker(body: &'static str) -> impl IntoResponse {
    (axum::http::StatusCode::OK, body)
}

async fn serve(app: Router) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), server)
}

#[tokio::test]
async fn a_proxy_serves_requests_instead_of_the_target() {
    // Target would answer "direct"; proxy answers "proxied".
    let (target, target_server) =
        serve(Router::new().fallback(|| async { marker("direct") })).await;
    let (proxy, proxy_server) = serve(Router::new().fallback(|| async { marker("proxied") })).await;

    let client = Client::builder().proxy(proxy).unwrap().build().unwrap();
    let response = client
        .read(Request::new(Method::Get, format!("{target}/episode")))
        .await
        .unwrap();
    let body = response.bytes().await.unwrap();
    assert_eq!(String::from_utf8(body).unwrap(), "proxied");

    target_server.abort();
    proxy_server.abort();
}

#[tokio::test]
async fn set_proxy_rebuilds_and_clears() {
    let (target, target_server) =
        serve(Router::new().fallback(|| async { marker("direct") })).await;
    let (proxy, proxy_server) = serve(Router::new().fallback(|| async { marker("proxied") })).await;
    let client = Client::builder().build().unwrap();
    let url = format!("{target}/x");

    // Direct first.
    let body = client
        .read(Request::new(Method::Get, &url))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(String::from_utf8(body).unwrap(), "direct");

    // Set proxy live.
    client.set_proxy(Some(&proxy)).unwrap();
    let body = client
        .read(Request::new(Method::Get, &url))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(String::from_utf8(body).unwrap(), "proxied");

    // Clear it.
    client.set_proxy(None).unwrap();
    let body = client
        .read(Request::new(Method::Get, &url))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(String::from_utf8(body).unwrap(), "direct");

    target_server.abort();
    proxy_server.abort();
}

#[test]
fn proxy_urls_with_embedded_credentials_are_rejected() {
    assert!(
        Client::builder()
            .proxy("http://user:pass@127.0.0.1:3128")
            .is_err()
    );
    assert!(
        Client::builder()
            .proxy("http://user@127.0.0.1:3128")
            .is_err()
    );
    assert!(
        Client::builder()
            .proxy("socks5://user:pass@127.0.0.1:1080")
            .is_err()
    );

    let client = Client::builder().build().unwrap();
    assert!(
        client
            .set_proxy(Some("http://user:pass@127.0.0.1:3128"))
            .is_err()
    );
}

#[test]
fn only_http_and_socks5_schemes_are_accepted() {
    assert!(Client::builder().proxy("http://127.0.0.1:3128").is_ok());
    assert!(Client::builder().proxy("socks5://127.0.0.1:1080").is_ok());
    assert!(Client::builder().proxy("not a url").is_err());
    assert!(Client::builder().proxy("ftp://127.0.0.1").is_err());
}
