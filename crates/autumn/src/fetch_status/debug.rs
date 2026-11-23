//! Local HTTP responses for exercising the shell in debug builds.

use std::time::Duration;

use domain::{AppEvent, SectionFetch};
use gpui_kit::{App, Entity};
use net::{Client, Error, Method, Request};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::{sleep, timeout},
};
use ui_state::{
    bridge,
    events::Sender,
    fetch_status::{DebugFetch, FetchCommand, FetchStatus, HOME_POSTERS},
};

pub(crate) const HOST: &str = engine::DEBUG_HOST;

pub(crate) fn bind(store: &Entity<FetchStatus>, client: Client, events: Sender, cx: &mut App) {
    let mut last = None;
    let mut running = None;
    cx.subscribe(store, move |_, command, cx| {
        let fetch = match *command {
            FetchCommand::Debug(fetch) => Some(fetch),
            FetchCommand::Retry(HOME_POSTERS) => last,
            _ => None,
        };
        let Some(fetch) = fetch else { return };
        if fetch != DebugFetch::RateLimited {
            last = Some(fetch);
        }
        let client = client.clone();
        let events = events.clone();
        running.replace(bridge::run(cx, async move {
            let state = match run(&client, fetch).await {
                Ok(Ok(_)) => SectionFetch::Ready,
                Ok(Err(Error::AniListUnavailable)) => SectionFetch::AniListUnavailable,
                Ok(Err(Error::RateLimited { .. })) => return,
                Ok(Err(error)) => SectionFetch::Failed(error.to_string()),
                Err(error) => {
                    tracing::warn!(%error, "debug fetch failed");
                    SectionFetch::Failed("The debug request failed.".into())
                }
            };
            let _ = events
                .send_async(AppEvent::SectionFetch {
                    section: HOME_POSTERS,
                    state,
                })
                .await;
        }));
    })
    .detach();
}

async fn run(client: &Client, fetch: DebugFetch) -> anyhow::Result<Result<net::Response, Error>> {
    let listener = TcpListener::bind((HOST, 0)).await?;
    let request = Request::new(Method::Get, format!("http://{}/", listener.local_addr()?));
    let reply = async move {
        let (mut stream, _) = listener.accept().await?;
        let mut input = BufReader::new(&mut stream).take(4096);
        let mut line = String::new();
        loop {
            if input.read_line(&mut line).await? == 0 || line == "\r\n" {
                break;
            }
            line.clear();
        }
        let response = match fetch {
            DebugFetch::RateLimited => "429 Too Many Requests\r\nRetry-After: 5\r\n",
            DebugFetch::Unavailable => "403 Forbidden\r\n",
            _ => {
                sleep(Duration::from_secs(3)).await;
                "200 OK\r\n"
            }
        };
        let header = format!("HTTP/1.1 {response}Content-Length: 0\r\nConnection: close\r\n\r\n");
        stream.write_all(header.as_bytes()).await
    };
    // One response per action. A simulated 429 must exercise the lockout
    // without consuming the shared read retry budget.
    let (result, reply) = timeout(Duration::from_secs(12), async {
        tokio::join!(client.mutate(request), reply)
    })
    .await?;
    reply?;
    Ok(result)
}
