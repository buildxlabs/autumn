# AnimeTosho is searched through the live `feed.animetosho.net` JSON API

The original AnimeTosho `animetosho.org` feed is retired and its API 404s, while the successor at `feed.animetosho.net` serves a public JSON API keyed by AniDB `aid`. Neither indexer publishes a numeric rate limit. We query `feed.animetosho.net/json`, short-circuit keyword variants when `aid` is known, use `eid` only as an optional hint, and keep a `q=` fallback; Nyaa stays keyword-only.

## Consequences

- The budget stays conservative: at most three title variants and eight requests per indexer, one page per request and one in-flight request, one backoff retry, a 60-second result cache plus the existing metadata caches, and dedupe by infohash.
- The indexer is replaceable, and Phase 4 extension providers can be added later; mirror and proxy settings are handled in Network settings.

## Considered options

- **Torznab `/api`.** It works as a query filter but is heavier than the JSON endpoint for this use.
- **The original AnimeTosho API.** It no longer resolves.
- **No request cap.** Neither indexer publishes a limit, and Nyaa has been observed resetting heavy clients.
