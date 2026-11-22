//! Loads GPUI's remote images through Autumn's shared poster cache.

use std::{future::Future, pin::Pin};

use anyhow::ensure;
use gpui_kit::http_client::{
    AsyncBody, HttpClient, Inner, Method, Request, Response, Url, http::HeaderValue,
};
use net::{Client, PosterCache};
use tokio::runtime::Handle;

type ResponseFuture = Pin<Box<dyn Future<Output = anyhow::Result<Response<AsyncBody>>> + Send>>;

/// GPUI's image loader uses bodyless GETs. The disk cache keys these requests
/// by URL, so callers cannot supply request headers or a body.
#[derive(Debug)]
pub(crate) struct PosterHttpClient {
    client: Client,
    cache: PosterCache,
    handle: Handle,
}

impl PosterHttpClient {
    pub(crate) fn new(client: Client, cache: PosterCache, handle: Handle) -> Self {
        Self {
            client,
            cache,
            handle,
        }
    }
}

impl HttpClient for PosterHttpClient {
    fn send(&self, request: Request<AsyncBody>) -> ResponseFuture {
        let client = self.client.clone();
        let cache = self.cache.clone();
        let handle = self.handle.clone();
        Box::pin(async move {
            ensure!(
                request.method() == Method::GET
                    && request.headers().is_empty()
                    && matches!(request.body().0, Inner::Empty),
                "the poster client supports only GET requests without headers or a body"
            );
            let url = request.uri().to_string();
            let bytes =
                ui_state::bridge::spawn(&handle, async move { cache.get(&client, &url).await })
                    .await??;
            Ok(Response::new(bytes.into()))
        })
    }

    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{Router, routing::get};
    use gpui_kit::{Asset, ImageAssetLoader, Resource, TestAppContext};
    use net::{Client, PosterCache};
    use tokio::{net::TcpListener, runtime::Builder};

    use super::PosterHttpClient;

    const IMAGE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1"/></svg>"#;

    #[gpui_kit::test]
    async fn gpui_loads_a_poster_again_after_reopening_the_disk_cache(cx: &mut TestAppContext) {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        cx.executor().allow_parking();

        let hits = Arc::new(AtomicUsize::new(0));
        let (url, server) = runtime.block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/poster", listener.local_addr().unwrap());
            let hits = Arc::clone(&hits);
            let app = Router::new().route(
                "/poster",
                get(move || async move {
                    hits.fetch_add(1, Ordering::Relaxed);
                    IMAGE
                }),
            );
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            (url, server)
        });

        let dir = tempfile::tempdir().unwrap();
        let source = Resource::Uri(url.into());
        for _ in 0..2 {
            let client = Client::builder().max_retries(0).build().unwrap();
            let cache = PosterCache::new(dir.path()).unwrap();
            let adapter = PosterHttpClient::new(client, cache, runtime.handle().clone());
            let load = cx.update(|cx| {
                cx.set_http_client(Arc::new(adapter));
                let executor = cx.background_executor().clone();
                executor.spawn(ImageAssetLoader::load(source.clone(), cx))
            });

            let image = load.await;
            assert!(image.is_ok(), "GPUI should decode the poster: {image:?}");
            assert_eq!(
                hits.load(Ordering::Relaxed),
                1,
                "only the cold load uses HTTP"
            );
        }
        server.abort();
    }
}
