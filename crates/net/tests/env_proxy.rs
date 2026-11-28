//! With no proxy configured, the guarded client ignores proxy environment variables.
//!
//! The variables must be set before the process starts, so the parent test
//! re-runs this binary with them and the `#[ignore]`d child test does the check.

#![expect(clippy::unwrap_used, reason = "test harness panics on setup failure")]

use std::net::IpAddr;

use axum::Router;
use net::{Client, DeclaredHost, ExtensionHosts, GuardedClient, Method, Request};
use tokio::{net::TcpListener, process::Command, task::JoinHandle};

const CHILD: &str = "child_ignores_environment_proxies";
const TARGET_PORT: &str = "NET_TEST_TARGET_PORT";

async fn serve(body: &'static str) -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().fallback(move || async move { body });
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, task)
}

#[tokio::test]
async fn environment_proxies_are_ignored_without_a_configured_proxy() {
    let (target, target_task) = serve("direct").await;
    let (proxy, proxy_task) = serve("proxied").await;
    let proxy_url = format!("http://127.0.0.1:{proxy}");

    let output = Command::new(std::env::current_exe().unwrap())
        .args([CHILD, "--exact", "--ignored", "--nocapture"])
        .env("HTTP_PROXY", &proxy_url)
        .env("http_proxy", &proxy_url)
        .env("HTTPS_PROXY", &proxy_url)
        .env("ALL_PROXY", &proxy_url)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .env(TARGET_PORT, target.to_string())
        .output()
        .await
        .unwrap();
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "child failed:\n{report}");
    assert!(report.contains("1 passed"), "child did not run:\n{report}");

    target_task.abort();
    proxy_task.abort();
}

#[tokio::test]
#[ignore = "run by environment_proxies_are_ignored_without_a_configured_proxy"]
async fn child_ignores_environment_proxies() {
    let port = std::env::var(TARGET_PORT).unwrap();

    // Control: the app client does honour the environment, so the variables are live.
    let app = Client::builder().build().unwrap();
    let via_app = app
        .read(Request::new(
            Method::Get,
            format!("http://api.test:{port}/"),
        ))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(via_app, "proxied");

    let guarded = GuardedClient::builder()
        .allow_loopback()
        .static_dns([("api.test", vec![IpAddr::from([127, 0, 0, 1])])])
        .build()
        .unwrap();
    let hosts = ExtensionHosts::new("ext.test", vec![DeclaredHost::parse("api.test").unwrap()]);
    let via_guard = guarded
        .send(
            &hosts,
            Request::new(Method::Get, format!("http://api.test:{port}/")),
        )
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(via_guard, "direct");
}
