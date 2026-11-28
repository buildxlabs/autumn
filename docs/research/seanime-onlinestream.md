# seanime onlinestream: how an episode becomes a playable stream

## Verdict

seanime resolves an episode through **three provider calls**: `Search(media + query)` picks a provider-side anime slug, `FindEpisodes(slug)` lists episodes, and `FindEpisodeServer(episode, serverName)` returns HTTP headers plus `m3u8`/`mp4` video sources with quality, label and subtitles. A SQLite `(provider, mediaId) → slug` row short-circuits search; the episode list is cached 24 h and per-episode servers 15 min in a file cache. **The backend does not detect expired URLs.** The web player sees an error or a stall in the first second of playback, calls the same source endpoint once with `refresh: true` (replacing only the 15-minute source cache), then walks the provider's servers and finally the other installed providers, ending in a toast when none work. Sources reach the player as a loopback `/api/v1/proxy` URL carrying the source headers as JSON and a route-scoped HMAC token; the proxy injects the headers, forwards `Range` and rewrites HLS segment/key URLs back through itself. Autumn should copy this **shape**, not the code: seanime is GPL-3.0, and #268 already established the pattern of defining the contract in Rust and validating at the host boundary.

## Sources read

- **seanime** `v3.10.5`, commit `d4d58b22fa690093da572083d163f60cc8725104` (2026-11-21), shallow-cloned outside the repo to `/private/var/folders/zj/_dgmsz8d20x2ch2gvvvtkkcc0000gn/T/opencode/autumn-research/seanime`. `LICENSE` is GNU GPL-3.0: behavior and contract shape only, no code ported. All citations below are `seanime@<commit>:<path>:<line>`.
- **Spyou/zangetsu-providers** commit `1e4e39d887991455832a9f72856af8e44ada6d3f`: has **no LICENSE file**, and its `providers/hianime.js` has none of seanime's `findEpisodeServer`/`videoSources` surface. Untyped/behavior-only, not used as a source. (The map's "Zangetsu providers (GPL-3.0)" likely refers to the separate `Spyou/Zangetsu` app, whose repo is GPL-3.0.)
- AGPL references (`reiverr`, `bookorbit`) are behavior-only and out of scope for this question.
- Repo side: `ROADMAP.md:127-134`, `ARCHITECTURE.md:58-62,88-104,167,171-176`, `GLOSSARY.md:177-242`, ADRs 0002, 0005, 0006, 0007, 0010, 0011-0015, and the #268 resolution (torrent-provider contract).

## 1. The provider contract

### Go interface (the host's view)

`seanime@commit:internal/extension/hibike/onlinestream/types.go:4-13` defines the interface:

```go
Provider interface {
    Search(opts SearchOptions) ([]*SearchResult, error)
    FindEpisodes(id string) ([]*EpisodeDetails, error)
    FindEpisodeServer(episode *EpisodeDetails, server string) (*EpisodeServer, error)
    GetSettings() Settings
}
```

`extension.OnlinestreamProviderExtension` (`internal/extension/onlinestream_provider.go:7-9,12-24`) wraps it with the common extension identity methods and `GetProvider()`. The manifest type is `onlinestream-provider` (`internal/extension/extension.go:16`); built-ins may be Go implementations of the same interface or JS/TS extensions loaded through Goja (`internal/extension_repo/builtin.go:103-117,136-149`).

### Request and result types

- `SearchOptions` (`types.go:15-25`): `Media` (AniList id, MAL id, status, format, English/romaji titles, episode count, synonyms, isAdult, fuzzy start date, `types.go:27-59`), `Query`, `Dub bool`, `Year int` (0 when unknown).
- `Settings` (`types.go:61-64`): `EpisodeServers []string` and `SupportsDub bool`. The host iterates `EpisodeServers` to fan out `FindEpisodeServer` calls (`repository_actions.go:186`); the client uses `supportsDub` to gate the dub toggle and to filter providers during fallback (`internal/extension_repo/repository.go:97-103,395-410`; `use-onlinestream-auto-provider-cycler.ts:94-98`; `onlinestream-page.tsx:388`).
- `SearchResult` (`types.go:66-75`): `id` (the anime slug), `title`, `url`, `subOrDub` (`"sub" | "dub" | "both"`, `types.go:139-143`).
- `EpisodeDetails` (`types.go:79-93`): `provider` (set by the host), `id` (episode slug), `number` (0..n), `url`, optional `title`.
- `EpisodeServer` (`types.go:95-106`): `server` (e.g. `vidcloud`), `headers map[string]string`, `videoSources`.
- `VideoSource` (`types.go:112-124`): `url`, `type` (`mp4`/`m3u8`/`unknown`, `types.go:145-149`), `label`, free-form `quality` (`default`/`auto`/`1080p`…), `subtitles`; `VideoSubtitle` has `id`, `url`, `language`, `isDefault` (`types.go:126-132`). `VideoExtractor.Extract(uri)` is the helper interface a provider may use to resolve an embed URL (`types.go:134-136`).

### JS/TS extension surface

`internal/extension_repo/goja_onlinestream_test/onlinestream-provider.d.ts:74-82` declares an `AnimeProvider` class with `search(opts)`, `findEpisodes(id)`, `findEpisodeServer(episode, server)` and `getSettings()`; the types at lines 1-72 mirror the Go structs. The Goja bridge calls those class methods and unmarshals into the Go types (`internal/extension_repo/goja_onlinestream_provider.go:31-117`), stamping `Provider = ext.ID` onto episodes and servers (`:76-78,96`). All four functions are effectively required: the host calls `GetSettings()` to build the provider list, `Search` for mapping, `FindEpisodes` for the list and `FindEpisodeServer` per server. There is no separate `api_version` on this contract: compatibility rides on seanime's extension manifest/version, unlike #268's single integer that versions both the host API and provider contract.

## 2. Episode → playable stream, step by step

1. **Entry point.** `POST /api/v1/onlinestream/episode-source` takes `mediaId`, `episodeNumber`, `provider`, `dubbed`, `refresh` (`internal/handlers/onlinestream.go:65-98`, routes at `internal/handlers/routes.go:364-371`) and calls `Repository.GetEpisodeSources` (`internal/onlinestream/repository.go:238-291`).
2. **Media.** `getMedia` fetches the AniList media through the platform, memoized in `anilist.BaseAnimeCache` (`repository.go:125-141`). The episode list endpoint additionally hydrates filler data and titles (`internal/handlers/onlinestream.go:18-63`; `repository.go:152-236`).
3. **Map media to a provider slug.** `getProviderEpisodeList` (`repository_actions.go:201-325`) first checks the manual mapping in SQLite (`:224-229`). With no mapping it builds `SearchOptions` from the media and calls `Search` twice, once with the romaji title and once with the English title, deduping by result id (`:239-301`). `GetBestSearchResult` (`:327-358`) keeps only results with a Levenshtein match against the media's titles and picks the smallest distance; no year/status/type is used to score. A miss gives `ErrNoAnimeFound` ("anime not found, try manual matching", `:14-18`). The chosen slug goes to `FindEpisodes` (`:314-324`).
4. **Episode list.** The list is loaded from the file cache when present (`repository_actions.go:74-94`), otherwise from step 3 and then cached. For `GetMediaEpisodes` the host passes `from = to = 0` so no servers are fetched (`repository.go:181`), which is why the list view is cheap.
5. **Servers for the episode.** `getEpisodeContainer` (`repository_actions.go:41-169`) loops the provider's episode details in the requested number range. For each, it reads the source cache; on a hit with `refresh == false` it uses it and continues (`:102-119`). `getProviderEpisodeServers` (`:178-199`) looks up the extension, iterates `GetSettings().EpisodeServers` and calls `FindEpisodeServer` for each; a failing server is dropped, and only when *all* fail does the episode error with `errNoEpisodeSourceFound`. Zoro dubs get a one-off slug fix: an id ending in `"both"` is rewritten to `"dub"` before the call (`:121-130`).
6. **Flatten.** `GetEpisodeSources` walks `episodeData.Servers → VideoSources`, mapping each into the host's `VideoSource` with `server`, `headers`, `url`, `label`, `quality`, `type`, and subtitles converted by `subtitleFromProvider` (`repository.go:258-299`). No source yields `ErrNoVideoSourceFound` (`repository.go:36-38,286-288`).

## 3. Mappings and caches

| What | Where | Key | Lifetime |
| --- | --- | --- | --- |
| Manual mapping `(provider, mediaId) → anime slug` | SQLite model `OnlinestreamMapping` (`internal/database/models/models.go:555-560`), accessed via `Get/Insert/DeleteOnlinestreamMapping` (`internal/database/db/onlinestream.go:15-52`), plus an in-memory `result.Map` keyed `provider$mediaId` (`:9-13`) | `provider + "$" + mediaId` | Persists until `RemoveMapping`; no TTL |
| Provider episode list | File cache bucket `onlinestream_<provider>_episode-list_<mediaId>` (`repository.go:115-121`) | `<mediaId>$<provider>$<dubbed>` (`repository_actions.go:60`) | `episodeListCacheTTL = 24h` (`repository.go:40-43`) |
| Per-episode servers/sources | File cache bucket `onlinestream_<provider>_episode-data_<mediaId>` (`repository.go:107-113`) | `<mediaId>$<provider>$<episodeNumber>$<dubbed>` (`repository_actions.go:103`) | `episodeSourceCacheTTL = 15m` (`repository.go:40-43`) |
| Manual-search results | In-memory `searchResultCache` (`manual_mapping.go:13`) | `provider + normalizedQuery + dub` (`:29-46`) | Process lifetime; no TTL |
| Media metadata | `anilist.BaseAnimeCache` (`repository.go:99,125-137`) | AniList media id | Process lifetime |

`ManualMapping` writes the DB row and removes both file-cache buckets for that provider/media; `RemoveMapping` does the reverse (`manual_mapping.go:51-80,101-121`). `EmptyCache(mediaId)` deletes every `onlinestream_*` file whose name contains the media id **and clears all file stores** (`repository.go:143-150`); the UI exposes it as "Empty stream cache" (`onlinestream-page.tsx:826-848`). The test `repository_test.go:41-102` pins both TTLs and proves that `refresh` replaces only the source bucket, leaving the 24 h episode-list bucket untouched.

## 4. Expired or failing sources: refresh → server fallback → provider fallback

The backend's refresh is a **flag on the normal resolve call**, not a separate endpoint: `refresh: true` makes `getEpisodeContainer` skip the source-cache read, re-call `FindEpisodeServer` for each configured server and overwrite the source cache (`repository_actions.go:109-119,132-152`; `internal/handlers/onlinestream.go:72-92`). The decision to refresh and the fallback walk live in the web client, not in Go:

- `use-onlinestream-auto-provider-cycler.ts:47-48` sets `PROVIDER_TIMEOUT_MS = 15_000` and `PLAYBACK_TIMEOUT_MS = 20_000`.
- `shouldRecoverStartup(currentTime) = currentTime < 1` (`onlinestream-provider-trial.ts:1-3`): only failures in the first second trigger recovery; a failure after a second of playback is left alone.
- On error/stall, `onPlaybackError` (`:189-230`) first checks a per-key set: `getRefreshKey = provider:server:episode` and `markSourceRefreshed` allow exactly one refresh per key (`onlinestream-provider-trial.ts:5-13`); the set is cleared when media or dubbed changes (`:267-269`). It then calls the same endpoint with `refresh: true` via `useRefreshOnlineStreamEpisodeSource` (`onlinestream.hooks.ts:69-85`). Empty sources, a thrown error or a stale candidate → next candidate.
- `goToNextCandidate` (`:133-152`) tries the next server drawn from `episodeSource.videoSources[].server` (`:50-56`), then `goToNextProvider` (`:113-131`) moves to the next installed provider, with the current one tried first (`orderProviders`, `onlinestream-provider-trial.ts:15-19`). Dubbed mode filters out providers whose `supportsDub` is false and shows "No dubbed providers available" when none remain (`:94-98,154-159`).
- If a candidate produces metadata and the player advances past one second, the trial ends and a "Found working provider" success is logged (`:236-253`). If all candidates fail, `stopWithFailure` clears the URL, sets `playbackError = "No working providers found"` and raises that toast (`:105-111`). A manual "Try all available providers" / "Cancel trying" button drives the same walker (`:779-787`; `onlinestream-page.tsx:779-787`).
- Errors that feed the walker: episode-list error/timeout/empty, episode-source error/timeout/empty, "no video sources", stalled playback and playback timeout (`:262-471`).

No automatic server or provider switch happens in `onlinestream` itself: `find the servers for one episode` skips failing servers but does not try another provider; the provider walk is a UI concern.

## 5. How sources reach the player

- The page picks a `VideoSource` by server and a fixed quality ladder (1080p → 720p → 480p → 360p, then `default`) in `onlinestream-page.tsx:502-579`. A `VideoCore` HTML5 player plays it; HLS is handled by the web HLS stack, and `type: "unknown"` is verified with a HEAD probe (`:593-637`).
- If the source has any headers, the URL becomes `getServerBaseUrl() + /api/v1/proxy?url=<encoded>&headers=<JSON>&headers…` via `onlinestream-proxy.ts:1-2`, with an HMAC token for `/api/v1/proxy` appended (`onlinestream-page.tsx:590,604-605`). Subtitle URLs get the same treatment (`:607-613`). Without headers the URL goes straight to the player.
- The proxy (`internal/handlers/proxy.go:206-388`) reads `url`, `headers` and `token` query params, validates the target with `security.ValidateOutboundUrl`, JSON-decodes the header map, injects it into the upstream GET (`:446-470`), forwards the client's `Range` header (`:465-467`), strips hop and blocked headers (`:50-66,476+`), and forwards the response with CORS. HEAD requests are sent upstream as GET because some hosts mishandle HEAD (`:254-257`).
- HLS: if the response is a playlist, it decodes master and media playlists and rewrites variant, alternative, segment, partial-segment, preload-hint, map and key URIs back through the proxy with the same headers/token (`:262-388,393-444`). HLS key responses are capped at 64 KiB and base64-normalized when they decode to a 16-byte AES key (`:159-204`). A primary client with HTTP/1.1 and relaxed TLS falls back to an HTTP/2 client on specific handshake errors (`:68-157`) — relevant because many stream CDNs only work with one of the two.
- Difference to note for Autumn: seanime's token authenticates the *route*, not a single playback session, and the proxy does no segment prefetch. ADR 0005 gives every Autumn stream a 128-bit token revoked at session end; ADR 0010 requires fail-closed outbound checks; ADR 0015 forbids a shared cookie jar (per-stream cookies from the provider are a separate question).

## 6. Online tab controls

Provider `<Select>` with a "Find other providers" entry that opens the marketplace filtered to `type=onlinestream-provider` (`onlinestream-page.tsx:791-813`); server `<Select>`, disabled when there is only one server (`:814-825`); quality selection through `VideoCore` with a stored preference (`:651-655`; `onlinestream.atoms.ts:47`); sub/dub toggle button labelled "Switch to dub" / "Switch to subs" (`:897-906`), which only takes effect when the selected provider reports `supportsDub` (`:388`); "Manual match" opens the mapping modal (`:887-896`; `manual_mapping.go:15-80`); "Empty stream cache" (`:826-848`); the try-all/fallback button (`:779-787`). Provider, server, quality and per-media sub/dub preferences are stored in `atomWithStorage` localStorage keys (`onlinestream.atoms.ts:4,18,42,47`).

## What this means for Autumn

- Contract in Rust, not Luau-table-compatible: define `OnlineProviderRequest`/`OnlineSearchResult`/`OnlineEpisode`/`OnlineServer`/`OnlineSource` in `extensions`, one manifest `type` value, validate every returned row at the host (drop invalid, clear bad optionals), and version with the same single `api_version` as #268. Do not load seanime extensions (D10); port behavior by hand.
- Keep the three-call shape but make each call independently testable; `onlinestream` resolves through the `extensions` edge only (`ARCHITECTURE.md:95`) and hands the final `StreamSource { url, headers, subtitles }` to `player`/`relay` through `engine`.
- Refresh is a host operation on the source cache only; the episode list and the manual mapping survive it. Fallback order is source refresh → next server → next provider, with one refresh per `(provider, server, episode)` and a bounded timeout, and the user sees which provider/server is being tried.
- The relay owes header injection, `Range` forwarding and HLS rewrite, and should prefer Autumn's per-stream token over seanime's route token. DASH rewrite and segment prefetch are Autumn additions, not seanime behavior.

## Open questions surfaced

1. **Contract breadth:** does Autumn's online provider export all four operations (`search`, `find_episodes`, `find_episode_server`, `settings`), or should the host own search/mapping and require only episode→server resolution? What is optional, and can a declarative source (ADR 0016) satisfy it at all?
2. **Result validation:** which fields are required per row (slug/id, episode number, source URL and type), how are bad headers/subtitles handled, and does a stale or malformed source fail the call or get dropped like #268's torrent rows?
3. **Manual mapping storage:** persist `(provider, mediaId) → provider slug` as its own table, or fold it into the existing Cross-reference / `ExternalId::Custom` model? Is "until manually removed" the right lifetime, and how does it interact with ADR 0001?
4. **Refresh semantics:** is refresh a flag on one resolve call (seanime) or a separate host method, and does it invalidate only the source cache, the episode list too, or the mapping as well? What TTLs replace 24 h / 15 min?
5. **Who drives fallback:** seanime's web UI watches playback errors; Autumn's `PlaybackManager` owns the player over IPC. Does the engine detect stall/error and drive refresh/fallback, or does ui-state? Where does "advance past one second = working" live?
6. **Fallback budget:** how do seanime's 15 s/20 s timeouts and one-refresh-per-key rule map onto ADR 0014's 1 s CPU / 30 s wall / 8 HTTP caps and ADR 0011's per-lane deadline, when one resolve fans out over several servers?
7. **Header policy at the relay:** which provider-supplied headers may be injected (`Referer`, `Origin`, `User-Agent`, `Cookie`, `Authorization`)? How does per-stream cookie injection reconcile with ADR 0015's stateless, no-shared-cookie-jar rule and ADR 0012's keychain-only secrets?
8. **Sub/dub and server modelling:** is dub a request flag on search and server resolution (seanime) or a separate provider entry, and how should a provider that returns `"both"` slugs (Zoro) be handled without provider-specific hacks in `onlinestream`?
