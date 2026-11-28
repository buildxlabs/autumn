//! Torrent providers as search lanes (ADR 0011, #264 Q7).
//!
//! `ProviderSet` is the seam `Searcher` uses: tests inject a fake, and the
//! real adapter wraps the `extensions` host handle. The host knows no `search`
//! types, so request mapping and row validation stay here.
//!
//! What an extension author can expect from search: one `search(request)` call
//! per search, with a 10 second deadline that cancels a slow call instead of
//! killing the VM. Rows without a name or a v1 infohash are dropped, bad
//! optional fields are cleared, unknown fields are ignored, and at most 200
//! rows are used. Results are cached for 60 seconds under the extension id,
//! manifest version and request. A failed or timed-out lane is reported to the
//! user while Nyaa and `AnimeTosho` results still return. Extensions can never
//! set `trusted`, `remake` or `is_seadex`.

use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

use domain::ExtensionId;
use extensions::{EpisodeQuery, ExtensionError, ExtensionHost, ProviderRequest, ProviderTorrent};

use crate::SearchRequest;
use crate::query::ordered_titles;

/// One runnable torrent provider, copied when a search starts.
///
/// The `version` is part of the 60-second result cache key, so an update can
/// never serve stale rows. `hosts` are the manifest's declared hosts: an
/// extension's `torrent_url` link is kept only when it is http(s) on one of
/// them. The name is what the release-list chip shows, with `(id)` appended
/// when two installed providers share a display name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProviderMeta {
    /// The manifest slug, used for lane order and the cache key.
    pub id: ExtensionId,
    /// Display name, or `name (id)` when display names clash.
    pub name: Arc<str>,
    /// Manifest version, part of the cache key.
    pub version: String,
    /// Host patterns from the manifest that links may point to.
    pub hosts: Vec<String>,
}

impl ProviderMeta {
    /// Describe one runnable provider for a search lane.
    #[must_use]
    pub fn new(
        id: ExtensionId,
        name: impl Into<Arc<str>>,
        version: impl Into<String>,
        hosts: Vec<String>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            version: version.into(),
            hosts,
        }
    }
}

/// The provider calls a search needs, with no VM in the interface.
///
/// Each call is one lane: search allows it 10 seconds and makes no retry of
/// its own. Its rows are validated before they join the results, at most 200
/// are kept, and a timeout or failure becomes a lane notice for the user
/// rather than a failed search. Successful rows are cached for 60 seconds
/// under `(id, version, request)`; failures are never cached.
pub trait ProviderSet: std::fmt::Debug + Send + Sync {
    /// The runnable providers, read once when a search starts.
    fn providers(&self) -> Vec<ProviderMeta>;

    /// Run one provider's `search(request)` call.
    ///
    /// # Errors
    ///
    /// Returns the provider's load or call error, which becomes a failed
    /// lane while the other lanes still return.
    fn search(
        &self,
        id: ExtensionId,
        request: ProviderRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ProviderTorrent>, ExtensionError>> + Send + 'static>>;
}

/// A searcher without an extension host, for built-in-only uses.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProviders;

impl ProviderSet for NoProviders {
    fn providers(&self) -> Vec<ProviderMeta> {
        Vec::new()
    }

    fn search(
        &self,
        id: ExtensionId,
        _request: ProviderRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ProviderTorrent>, ExtensionError>> + Send + 'static>>
    {
        Box::pin(async move { Err(ExtensionError::UnknownExtension(id)) })
    }
}

/// The real provider set, wrapping an extension host handle.
///
/// Search calls each provider once per search and waits at most 10 seconds;
/// a slow call is cancelled, not killed. Returned rows are validated here:
/// no name or v1 infohash drops the row, an invalid optional field is cleared,
/// links must be http(s) on the manifest's declared `hosts`, and only the
/// first 200 rows count. Results are cached for 60 seconds under the id,
/// version and request. A failed lane is shown to the user as a notice while
/// Nyaa and `AnimeTosho` results still return; trust flags are never mapped.
///
/// `hosts` maps each extension id to the host patterns in its manifest. The
/// host API does not expose manifests, so the caller supplies them when it
/// builds the searcher.
#[derive(Debug, Clone)]
pub struct ExtensionProviders {
    host: ExtensionHost,
    hosts: HashMap<ExtensionId, Vec<String>>,
}

impl ExtensionProviders {
    /// Wrap `host` with the manifest host patterns in `hosts`.
    #[must_use]
    pub fn new(host: ExtensionHost, hosts: HashMap<ExtensionId, Vec<String>>) -> Self {
        Self { host, hosts }
    }
}

impl ProviderSet for ExtensionProviders {
    fn providers(&self) -> Vec<ProviderMeta> {
        self.host
            .providers()
            .map(|info| ProviderMeta {
                id: info.id.clone(),
                name: Arc::from(info.name.as_str()),
                version: info.version.clone(),
                hosts: self.hosts.get(&info.id).cloned().unwrap_or_default(),
            })
            .collect()
    }

    fn search(
        &self,
        id: ExtensionId,
        request: ProviderRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ProviderTorrent>, ExtensionError>> + Send + 'static>>
    {
        let host = self.host.clone();
        Box::pin(async move { host.search(&id, request).await })
    }
}

/// The lanes to add, in dedupe order: by id, with clashing display names
/// disambiguated so the release-list chip reads `name (id)` (#264 Q1).
pub(crate) fn lane_metas(providers: &dyn ProviderSet) -> Vec<ProviderMeta> {
    let mut metas = providers.providers();
    metas.sort_by(|a, b| a.id.cmp(&b.id));
    let mut names: HashMap<String, usize> = HashMap::new();
    for meta in &metas {
        *names.entry(meta.name.to_string()).or_default() += 1;
    }
    for meta in &mut metas {
        if names
            .get(meta.name.as_ref())
            .is_some_and(|count| *count > 1)
        {
            meta.name = Arc::from(format!("{} ({})", meta.name, meta.id).as_str());
        }
    }
    metas
}

/// A structured search's provider request: `keyword` is the first cleaned
/// title and the episode fields are all present (#268 Q2).
pub(crate) fn structured_request(request: &SearchRequest) -> ProviderRequest {
    let titles = ordered_titles(request);
    let keyword = titles.first().cloned().unwrap_or_default();
    ProviderRequest {
        keyword,
        episode: Some(EpisodeQuery {
            titles,
            season: request.season,
            episode: request.episode,
        }),
        absolute_episode: request.absolute_episode,
        anidb_aid: request.anidb_aid,
        anidb_eid: request.anidb_eid,
    }
}

/// A keyword-only search's provider request: no episode fields at all.
pub(crate) fn keyword_request(keyword: &str) -> ProviderRequest {
    ProviderRequest::keyword(keyword.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Fixed(Vec<ProviderMeta>);

    impl ProviderSet for Fixed {
        fn providers(&self) -> Vec<ProviderMeta> {
            self.0.clone()
        }
        fn search(
            &self,
            id: ExtensionId,
            _request: ProviderRequest,
        ) -> Pin<
            Box<dyn Future<Output = Result<Vec<ProviderTorrent>, ExtensionError>> + Send + 'static>,
        > {
            Box::pin(async move { Err(ExtensionError::UnknownExtension(id)) })
        }
    }

    fn meta(name: &str, id: &str) -> ProviderMeta {
        ProviderMeta {
            id: ExtensionId::new(id),
            name: Arc::from(name),
            version: "1.0.0".to_owned(),
            hosts: Vec::new(),
        }
    }

    #[test]
    fn lane_order_is_by_id_and_clashing_display_names_are_disambiguated() {
        let providers = vec![
            meta("SubsPlease", "zeta"),
            meta("Same", "beta"),
            meta("Same", "alpha"),
        ];
        let lanes = lane_metas(&Fixed(providers));

        assert_eq!(
            lanes
                .iter()
                .map(|lane| lane.id.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta", "zeta"]
        );
        assert_eq!(
            lanes.first().map(|lane| lane.name.as_ref()),
            Some("Same (alpha)")
        );
        assert_eq!(
            lanes.get(1).map(|lane| lane.name.as_ref()),
            Some("Same (beta)")
        );
        assert_eq!(
            lanes.get(2).map(|lane| lane.name.as_ref()),
            Some("SubsPlease")
        );
    }

    #[test]
    fn a_structured_request_carries_titles_and_ids_and_a_keyword_one_does_not() {
        let request = SearchRequest::new(
            vec!["Sousou no Frieren S2!".to_owned(), "Frieren".to_owned()],
            2,
            5,
        )
        .with_absolute_episode(30)
        .with_anidb_ids(17_617, Some(271_422));

        let structured = structured_request(&request);
        let episode = structured
            .episode
            .as_ref()
            .expect("structured episode fields");

        assert_eq!(structured.keyword, "sousou no frieren");
        assert_eq!(episode.titles, ["sousou no frieren", "frieren"]);
        assert_eq!(episode.season, 2);
        assert_eq!(episode.episode, 5);
        assert_eq!(structured.absolute_episode, Some(30));
        assert_eq!(structured.anidb_aid, Some(17_617));
        assert_eq!(structured.anidb_eid, Some(271_422));

        let keyword = keyword_request("  frieren batch  ");
        assert_eq!(keyword.keyword, "frieren batch");
        assert!(keyword.episode.is_none());
        assert!(keyword.anidb_aid.is_none());
    }
}
