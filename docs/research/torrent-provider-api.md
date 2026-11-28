# Torrent-provider extensions: the seanime contract and its host API

Research for [#260](https://github.com/buildxlabs/autumn/issues/260), a research
ticket on the [Phase 4 wayfinder map](https://github.com/buildxlabs/autumn/issues/258).
It answers what a torrent-provider extension implements and calls in seanime,
what real providers need from the host, and how that maps onto Autumn's
`crates/search`.

Researched 2026-11-28 against **seanime** (`reference/seanime`, GPL-3.0) and
**mangayomi** (`reference/mangayomi`, Apache-2.0), plus seven community seanime
extension payloads fetched from GitHub. Both licenses were checked at
`reference/<repo>/LICENSE`. Autumn is GPL-3.0-only, so seanime and mangayomi code
may be ported with their notices kept; no code was ported for this note.

- `internal/…`, `lib/…` paths are relative to the reference repo root named in
  the section.
- `crates/search/src/…` paths are relative to this repository.
- Community extension citations are raw GitHub URLs, read directly.

## Verdict

A seanime torrent provider is **one global `Provider` class** with six methods:
`search`, `smartSearch`, `getTorrentInfoHash`, `getTorrentMagnetLink`,
`getLatest`, `getSettings`. The host wraps each method in panic recovery,
awaits its promise, and converts the return through JSON. Results from all
selected providers — built-in Go providers and extension scripts alike — are
merged concurrently, deduped by infohash (name as fallback), best releases
first, then sorted by seeders.

For Autumn's Phase 4 exit criterion (an installed provider's results in manual
search alongside Nyaa), the load-bearing subset is **search + infohash/magnet +
result identity**. `smartSearch` and `getLatest` have no matching request shape
in `SearchRequest` today. The main structural gaps are Autumn's closed `Indexer`
enum, its non-optional infohash, and its fail-the-whole-search `tokio::join!`.

Real providers need far less host API than seanime exposes: every surveyed
provider used `fetch` + native `JSON`/promises and `console`; HTML scrapers used
`LoadDoc`; smart-search providers used `$scannerUtils`/`$habari`; none used
`$store`, persistent storage, ChromeDP, `$sleep`, or the CryptoJS hash methods.

## Q1. The seanime torrent-provider contract

### The class and its six methods

`hibiketorrent.AnimeProvider` is the Go-side interface
(`internal/extension/hibike/torrent/types.go:35-50`); the TypeScript declaration
of the same class is `internal/extension_repo/goja_torrent_test/anime-torrent-provider.d.ts:68-88`.
The goja bridge implements that interface by calling methods on a JS global
`class Provider` (`internal/extension_repo/goja_anime_torrent_provider.go:30-134`):

| JS method                       | Called by                          | Returns                                                          |
| ------------------------------- | ---------------------------------- | ---------------------------------------------------------------- |
| `search(opts)`                  | `Search` (`:30-48`)                | `AnimeTorrent[]`                                                 |
| `smartSearch(opts)`             | `SmartSearch` (`:50-68`)           | `AnimeTorrent[]`                                                 |
| `getTorrentInfoHash(torrent)`   | `GetTorrentInfoHash` (`:70-84`)    | `string`                                                         |
| `getTorrentMagnetLink(torrent)` | `GetTorrentMagnetLink` (`:86-100`) | `string`                                                         |
| `getLatest()`                   | `GetLatest` (`:102-116`)           | `AnimeTorrent[]`                                                 |
| `getSettings()`                 | `GetSettings` (`:118-134`)         | `AnimeProviderSettings`; synchronous, defaults to empty on panic |

Bridge mechanics, all in `internal/extension_repo/goja_base.go`:

- The program is compiled once per extension (`:52-57`) and run in a **private VM
  pool per extension id** (`:71-90`), so two extensions cannot see each other's
  globals.
- Every call checks `typeof Provider`, then does `new Provider()` (`:112-135`),
  so a fresh instance is used each time. Options are passed via
  `structToMap` — JSON marshal/unmarshal into a plain map (`goja.go:279-295`).
- Return values are awaited and exported (`:189-220`); a rejected promise becomes
  an error (`:202-204`); the Go struct is filled through JSON
  (`unmarshalValue`, `:162-182`).
- A blocking `fetch` or a sleep inside the script is bounded only by the host's
  timeouts; `ctx` is forwarded into promise waiting (`:198`).

`Search` and `SmartSearch` stamp `Provider = ext.ID` on every returned torrent
(`goja_anime_torrent_provider.go:43-45`, `:63-65`). `GetLatest` does not (`:102-116`).
Each Go method has its own `HandlePanicInModuleWithError` so a throwing script
fails one call, not the app (`:31`, `:51`, `:71`, `:87`, `:103`, `:119-121`).

### Options passed in

Defined in `internal/extension/hibike/torrent/types.go` and declared in the
`.d.ts` (`:1-46`):

- `AnimeSearchOptions` (`:89-95`): `media` plus `query`. When the user submits a
  simple search with an empty query, the host substitutes the cleaned romaji
  title (`internal/torrents/torrent/search.go:228-230`).
- `AnimeSmartSearchOptions` (`:97-120`): `media`, `query`, `batch`,
  `episodeNumber`, `resolution`, `anidbAID`, `anidbEID`, `bestReleases`.
  Fields are zero/empty when the extension declares it does not support that
  filter.
- `Media` (`:52-81`): AniList `id`, optional MAL `idMal`, `status`, `format`,
  `englishTitle`, `romajiTitle`, `episodeCount`, `absoluteSeasonOffset`,
  `synonyms`, `isAdult`, `startDate` (`FuzzyDate`, `:83-87`). The host builds it
  from AniList plus anime metadata (`search.go:116-152`, `:191-204`).

### Result fields

`AnimeTorrent` (`types.go:122-169`, `.d.ts:48-66`). Fields relevant to a port:

- Required-ish identity and display: `provider`, `name`, `date` (RFC3339 string),
  `size` (bytes), `formattedSize`.
- Stats: `seeders`, `leechers`, `downloadCount`.
- Retrieval: `link` (torrent **page**), `downloadUrl` (`.torrent` file),
  `magnetLink`, `infoHash`. The `.d.ts` comments say to leave magnet/infohash
  empty if scraping would be needed — the host calls `getTorrentInfoHash` /
  `getTorrentMagnetLink` later instead.
- Parsing hints: `resolution`, `isBatch`, `episodeNumber` (`-1` = let the host
  parse), `releaseGroup`, `isBestRelease`, `confirmed`.

`AnimeTorrentFile` (`types.go:171-176`) is **not** returned by the provider
methods; it appears only in the host's preview/batch-file logic.

### Settings and user config

Two different things:

- `getSettings()` declares **capabilities**: `canSmartSearch`,
  `smartSearchFilters` (`batch`, `episodeNumber`, `resolution`, `query`,
  `bestReleases`), `supportsAdult`, `type` (`main` | `special`)
  (`types.go:15-33`, `.d.ts:1-10`). A `special` provider cannot be the default
  and should not return `getLatest` results (`types.go:6-13`). These flags drive
  control flow in `search.go:218-227`: a main provider without smart search
  errors when smart search is requested (`:221-224`), secondary providers fall
  back to simple search (`:225-227`).
- **User config** is manifest-declared (`UserConfig`, `ConfigField`,
  `SavedUserConfig`, `internal/extension/extension.go:195-233`) and read from the
  VM with `$getUserPreference(key)`, which falls back to the field default
  (`internal/extension_repo/goja.go:38-59`). Real providers use it for API keys,
  cookies and endpoints (see Q2).

### How results merge with built-in providers

Built-in providers implement the same Go interface and are registered through
`NewAnimeTorrentProviderExtension` (`internal/extension_repo/builtin.go:131-134`),
so the merge path cannot tell a built-in from an extension script.

`internal/torrents/torrent/search.go:206-397`:

1. Resolve requested provider ids, or the default provider
   (`:455-489`). The **first** provider is "main".
2. Run one goroutine per provider (`:206-284`); smart vs simple and the
   capability fallback are decided at `:218-230`. Context cancellation is
   checked before each call (`:234-238`, `:261-265`).
3. **Only the main provider's error is fatal** (`:286-288`); errors from
   secondary providers are dropped. Results are concatenated and a missing
   `Provider` is stamped (`:290-299`).
4. Empty infohash becomes the torrent name (`:302-308`), best releases move to
   the front (`:309-315`), then dedupe is by infohash via `lo.UniqBy`
   (`:317-319`).
5. Names are parsed with `habari` and matched against the media's titles by
   Levenshtein (`:321-349`), then sorted by best-release flag and seeders
   (`:351-360`). Smart search generates previews (`:369-376`); the result is
   cached by provider/query key (`:388-391`).
6. Debrid instant availability is looked up by the infohashes of the merged
   results (`internal/handlers/torrent_search.go:60-82`) — another consumer that
   assumes a hash exists.

## Q2. The host bindings a provider gets

### What is bound

A provider VM is assembled in `initializeProviderBase`
(`internal/extension_repo/goja_base.go:71-81`) plus `ShareBinds`
(`internal/extension_repo/goja.go:63-93`). Public declarations live in
`internal/extension_repo/goja_plugin_types/core.d.ts`.

| Global                                                                                                                    | Purpose                                                                                                                                                                  | Source                                                                                                     |
| ------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------- |
| `fetch(url, opts)`                                                                                                        | HTTP with `method`/`headers`/`body`/`timeout`/`redirect`/`signal`, response `.json()`/`.text()`, URL allowlist. Providers are bound with `["*"]`.                        | `internal/goja/goja_bindings/fetch.go:162-204`, `:279-567`; `goja_base.go:77`                              |
| `Doc(html)` / `LoadDoc(html)`                                                                                             | cheerio-like DOM via goquery; `find`, `attr`, `each`, `text`, `xpath`. Note: `ShareBinds` calls the binding `"document"`, but the injected names are `Doc`/`LoadDoc`.    | `internal/goja/goja_bindings/document.go:22-114`; `goja.go:83`; `core.d.ts:238-345`                        |
| `CryptoJS.AES`, `CryptoJS.enc.*`                                                                                          | AES plus Base64/Hex/Utf8 Latin1 encoders. `WordArray.toString()` defaults to Base64. Hash code exists (`crypto_hash.go:13-119`) but **`BindCrypto` never registers it**. | `internal/goja/goja_bindings/crypto.go:23-84`; `crypto_encoders.go:21-47`, `:142-144`; `core.d.ts:202-231` |
| `console.*`                                                                                                               | Logging, mirrored to zerolog and the in-app console over websocket.                                                                                                      | `internal/goja/goja_bindings/console.go:26-54`, `:151-159`                                                 |
| `$sleep(ms)`                                                                                                              | Blocking sleep.                                                                                                                                                          | `goja.go:157-159`                                                                                          |
| `$store`                                                                                                                  | Per-extension **in-memory** key/value store (`get`/`set`/`has`/`watch`/`getOrSet`/…); constructed with `nil` backing, so nothing is persisted.                           | `internal/plugin/store.go:17-19`, `:58-100`; `goja_base.go:66`, `:74`; `core.d.ts:666-758`                 |
| `$getUserPreference(key)`                                                                                                 | Manifest-declared user config values and defaults.                                                                                                                       | `goja.go:38-59`                                                                                            |
| `$torrentUtils.getMagnetLinkFromTorrentData(b64)`                                                                         | Convert base64 `.torrent` bytes to a magnet.                                                                                                                             | `internal/goja/goja_bindings/torrent.go:9-45`                                                              |
| `$scannerUtils.*`                                                                                                         | Title normalization, season/part extraction, smart-search query building, matching.                                                                                      | `internal/goja/goja_bindings/scanner_utils.go:15-30`                                                       |
| `$habari.parse(name)`, `$goFeed.parse(xml)`                                                                               | Anime filename metadata and RSS/Atom parsing.                                                                                                                            | `goja.go:208-226`                                                                                          |
| `JSON`, `Promise`, `Date`, …                                                                                              | Native goja ECMAScript builtins; no binding needed.                                                                                                                      | —                                                                                                          |
| `Buffer`, `URL`, `require`                                                                                                | Node-style modules enabled on the VM.                                                                                                                                    | `goja.go:64-65`, `:75-76`                                                                                  |
| `ChromeDP.*`                                                                                                              | Headless browser automation; bound for all VMs but not used by surveyed providers.                                                                                       | `internal/goja/goja_bindings/chromedp.go:87-106`, `:184-200`                                               |
| `$toString`, `$toBytes`, `$unmarshal`, `$toError`, `$arrayOf`, `$Context`, `$waitGroup`, `$unsafeGoroutine`, `$isOffline` | Conversion and runtime helpers.                                                                                                                                          | `goja.go:95-254`                                                                                           |

`$storage` (persistent key/value, `internal/plugin/storage.go:96-125`) is bound
only in the plugin runtime (`internal/extension_repo/goja_plugin.go:273`), **not**
in a provider VM. `AbortContext` is likewise plugin-UI-only, so a provider
cannot construct a `fetch` abort signal.

### What real providers actually call

The in-repo "built-in" goja provider is only a Go wrapper and its test payload
is a stub with no host calls (`internal/extension_repo/goja_anime_torrent_provider.go:14-134`,
`goja_anime_torrent_provider_test.go:16-77`); the torrent `.d.ts` declares only
the class, not the globals. So behavior was checked against seven community
payloads from the community marketplace index
(`https://raw.githubusercontent.com/Bas1874/Seanime-Marketplace/main/Marketplace/Main.json`,
30 anime-torrent-provider entries):

- `0utLunar/animetosho-provider` —
  https://raw.githubusercontent.com/0utLunar/animetosho-provider/main/animetosho.ts
  `fetch` (`:49`, `:62`, `:141`, `:161`), `.json()` (`:50`, `:63`, `:147`),
  `console` (`:53`, `:61`, `:66`, `:85`), `JSON.stringify` (`:144`).
- `Jhoorodre/seanime-provider` (torrentclaw) —
  https://raw.githubusercontent.com/Jhoorodre/seanime-provider/master/src/torrentclaw/torrentclaw-provider.ts
  `fetch`/`.ok`/`.status`/`.json()` (`:119-125`), native `encodeURIComponent`, `Date`.
- `Bas1874/ACG.RIP-Torrent-Provider-Seanime` —
  https://raw.githubusercontent.com/Bas1874/ACG.RIP-Torrent-Provider-Seanime/refs/heads/main/src/ACG.ts
  `fetch` (`:22`, `:33`, `:103`), **`LoadDoc`** (`:42`),
  **`$torrentUtils.getMagnetLinkFromTorrentData`** (`:110`), `console.error`.
- Island's Nyaa — https://island.clap.ing/api/extensions/anime-torrent-providers/nyaa/nyaa.ts
  `fetch` (`:40`, `:56`, `:83`, `:125`), `LoadDoc` (`:127`),
  `$scannerUtils` (`buildSmartSearchTitles :178`, `buildSeasonQuery :245`,
  `extractSeasonNumber :510`, `compareTitles :570`), `$habari.parse` (`:787`),
  `$getUserPreference("apiUrl"/"category")` (`:749`, `:755`).
- `aor-rex/seanime-extensions` (EXT Torrents) —
  https://raw.githubusercontent.com/aor-rex/seanime-extensions/master/src/EXT/main.ts
  `fetch` (`:481`, `:632`, `:649`), `LoadDoc` (`:493`), `$scannerUtils`
  (`:96-128`), `$getUserPreference("cf-cookie"/"user-agent")` (`:411`, `:452`).
  At `:299-302` the author notes that seanime exposes only `CryptoJS.AES`/`enc`,
  so SHA-256 is implemented in plain JS.
- `faddix/seanime-animetosho-new`, `Crashdaemon/Seanime-Torrentio`,
  `Bas1874/SubsPleaseSeanime`, `Lampe332/seanime-archive-org` — `fetch` (2–3
  calls each), `console`, three of them `$getUserPreference`, one `$habari`.

**Pattern.** Universal: `fetch`, response `.json()`, native `JSON`, `Promise`,
`Date`, `console`. HTML scrapers: `LoadDoc`. Smart-search providers:
`$scannerUtils`, `$habari`. `.torrent`-downloading providers: `$torrentUtils`.
Config-bearing providers: `$getUserPreference`. **None** of the seven used
`$store`, `$storage`, `Buffer`, `ChromeDP`, `$sleep`, `AbortContext`, or the
unbound CryptoJS hashes. The survey is the 30-provider marketplace index, not an
exhaustive census of GitHub.

### What this means for Autumn's host API

The minimum useful surface for a torrent provider is: outbound HTTP with
timeouts and the guard from ADR 0003 / ARCHITECTURE §6, a DOM parser,
Base64/hex encoding plus AES, native JSON and promises, logging, and a way to
read settings. A SHA-256 helper is worth providing because real providers
currently hand-roll one. Persistent storage is not demonstrated as a need.
Because seanime compiles once and reuses a private VM pool, a provider that
loops can saturate a call; Autumn's interrupt and per-call timeout must cover
promise waits too (see `mlua-luau-sandbox.md`).

## Q3. mangayomi's source host API (contrast)

mangayomi injects its JS source API into a QuickJS runtime (`flutter_qjs`) using
`sendMessage(name, JSON.stringify(args))` and `runtime.onMessage(...)`
(`lib/eval/javascript/service.dart:33`, `:41-98`). Sources subclass
`DefaultExtension extends MProvider` (`:41-82`). There are **no `.d.ts` files in
the tree**; the API is documented in JSDoc in
`lib/modules/browse/extension/widgets/extension_templates.dart:309-326`.

| Global                                                                                                               | Purpose                                                                    | Source                                             |
| -------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- | -------------------------------------------------- |
| `console.log/warn/error`                                                                                             | Routed to the app logger.                                                  | `lib/eval/javascript/js_common.dart:7-35`          |
| `Client`                                                                                                             | `head/get/post/put/delete/patch(url, headers, body)` over the host bridge. | `lib/eval/javascript/http.dart:50-104`, `:108-149` |
| `Document` / `Element`                                                                                               | `select`, `selectFirst`, `xpath`, `attr`, `text`, `outerHtml`.             | `lib/eval/javascript/dom_selector.dart:191-289`    |
| `SharedPreferences`                                                                                                  | **Persistent** source preferences: `get`, `getString`, `setString`.        | `lib/eval/javascript/preferences.dart:11-42`       |
| `cryptoHandler`, `encryptAESCryptoJS`, `decryptAESCryptoJS`, `decryptAESGCM`                                         | AES/GCM helpers; no generic MD5/SHA or Base64 global found.                | `lib/eval/javascript/utils.dart:30-42`, `:109-132` |
| `parseDates`, `deobfuscateJsPassword`, `unpackJs`, `evaluateJavascriptViaWebview`, `createHlsProxyUrl`, EPUB helpers | Date and content utilities.                                                | `lib/eval/javascript/utils.dart:43-180`            |
| `sibnetExtractor`, `okruExtractor`, `doodExtractor`, …                                                               | Video extractors.                                                          | `lib/eval/javascript/extractors.dart:110-174`      |
| Native `JSON`, string helpers                                                                                        | `substringAfter`, `substringBetween`, etc.                                 | `js_common.dart:36-70`                             |

Contrast worth keeping: mangayomi gives sources **persistent** preferences;
seanime gives providers only an in-memory `$store` and reads config through
`$getUserPreference`. Do not treat `$store` as a persistence precedent.
mangayomi also supports several source dialects (`javascript`, `dart`, `mihon`,
`lnreader`, `aidoku`, `lib/eval/lib.dart:10-20`) with different host surfaces;
only the default JS path was surveyed. License: Apache-2.0
(`reference/mangayomi/LICENSE`).

## Q4. Mapping onto Autumn's `search`

Autumn's side is `crates/search/src/result.rs` (`Torrent`, `Indexer`),
`query.rs` (`SearchRequest`, `Query`, `plan`) and `search.rs` (`Searcher`,
`Transport`, `Config`).

### `SearchRequest` ← seanime search options

| seanime input                                 | Autumn equivalent                                                                                                                                                   | Assessment                                                                                                                   |
| --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `AnimeSearchOptions.query`                    | no field; only `search_keyword(keyword, refresh)` (`search.rs:214-233`)                                                                                             | Manual keyword search exists, but a provider-facing free-text query would need a new call shape or be derived from `titles`. |
| `Media.romajiTitle`/`englishTitle`/`synonyms` | `SearchRequest.titles` (ordered, cleaned, max 3; `query.rs:19-32`, `:198-213`, `:313-322`)                                                                          | Enough for search; no AniList/MAL id, status, format, episode count, start date or adult flag.                               |
| `Media.absoluteSeasonOffset`                  | `SearchRequest.season`, `absolute_episode` (`query.rs:22-27`)                                                                                                       | Different model; Autumn's season is explicit.                                                                                |
| `AnimeSmartSearchOptions.episodeNumber`       | `episode` (`query.rs:25`)                                                                                                                                           | Direct.                                                                                                                      |
| `AnidbAID`, `AnidbEID`                        | `anidb_aid`, `anidb_eid` (`query.rs:28-31`)                                                                                                                         | Direct.                                                                                                                      |
| `batch`, `resolution`, `bestReleases`         | no request field; `Query::anime_tosho_batch` is built internally (`query.rs:113-123`, `:272`) and resolution/best-release are auto-select concerns (`rank.rs:1-10`) | Cannot be expressed to an extension today without a new request field or manifest capability flags.                          |

### `Torrent` ← seanime `AnimeTorrent`

| seanime field                                                               | Autumn field                                                                             | Assessment                                                                                                                                             |
| --------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `provider`                                                                  | `indexer: Indexer` (`result.rs:3-22`)                                                    | **Blocker:** closed enum, only Nyaa/AnimeTosho. Must grow an extension identity (e.g. variant carrying an id) before provider results can be labelled. |
| `infoHash` (optional; `""` falls back to name during merge)                 | `infohash: String`, required and the dedupe key (`result.rs:37-38`, `search.rs:169-171`) | **Blocker for ported providers:** hash-less results are skipped (`nyaa.rs:120`, `animetosho.rs:33`); seanime intentionally tolerates them.             |
| `name`                                                                      | `name` (`result.rs:40`)                                                                  | Direct.                                                                                                                                                |
| `downloadUrl`                                                               | `torrent_url` (`result.rs:41-42`)                                                        | Direct; `nyaa.rs:67`, `animetosho.rs:49`.                                                                                                              |
| `magnetLink`                                                                | `magnet_url` (`result.rs:43-44`)                                                         | Direct, and Autumn derives one from the hash (`util::magnet_for`, `nyaa.rs:68`).                                                                       |
| `date` (RFC3339 string)                                                     | `published: Option<u64>` Unix seconds (`result.rs:67-68`)                                | Needs conversion; `nyaa.rs:83`, `animetosho.rs:59`.                                                                                                    |
| `size`, `seeders`, `leechers`, `downloadCount`                              | `size`, `seeders`, `leechers`, `downloads` (`result.rs:45-52`)                           | Direct.                                                                                                                                                |
| `isBatch`                                                                   | `is_batch` (`result.rs:57-58`)                                                           | Direct.                                                                                                                                                |
| `resolution`, `releaseGroup`, `episodeNumber`, `isBestRelease`, `confirmed` | no fields                                                                                | Autumn infers these from `name` via `release_parser` and `rank.rs`. No way for an extension to assert `confirmed`/`isBestRelease`.                     |
| `formattedSize`, `link` (page URL)                                          | no fields                                                                                | Presentational / page link; only a `.torrent` URL is stored.                                                                                           |
| — (seanime result has none)                                                 | `anidb_aid`, `anidb_eid` (`result.rs:61-64`)                                             | Autumn can carry ids that seanime only passes as _input_ to `smartSearch`.                                                                             |
| —                                                                           | `trusted`, `remake`, `is_seadex`, `num_files`, `files` (`result.rs:53-70`)               | Indexer-specific; no seanime analogue.                                                                                                                 |

### Missing on either side

- **Autumn:** the closed `Indexer` enum; required infohash; `Searcher::search`
  hard-codes both indexers and uses `nyaa?`/`tosho?`, so one failing indexer
  fails the whole search (`search.rs:161-172`) — unlike seanime, where only the
  main provider's error is fatal. There is no extension settings/user-config
  plumbing, no provider-facing smart-search shape, and no per-result
  `confirmed`/`isBestRelease` or page-link field.
- **seanime:** no persistent storage for providers (`$store` is in-memory and
  `$storage` is plugin-only); no exposed hash primitive; no declaration file
  for the host globals in the torrent API (only the class is declared).
- **Both:** debrid availability and dedupe assume infohash, so a hash-less
  extension result needs an explicit policy.

## Open questions surfaced (not resolved here)

1. Should Phase 4 expose a `smartSearch` call at all? Autumn's `SearchRequest`
   has no `batch`/`resolution`/`bestReleases`; capability flags could live in
   the manifest instead.
2. Does `getLatest()` belong in Phase 4? There is no Phase 3 "latest" seam for
   it to feed.
3. What happens to an extension result with no infohash: drop it as Autumn does
   today, or keep it with a name-based dedupe key as seanime does? This affects
   the exit criterion if a provider cannot supply a hash without scraping.
4. Should `confirmed`/`isBestRelease` become explicit result flags, or keep
   relying on release-name parsing and auto-select ranking?
5. Persistent per-extension storage is not demonstrated as needed by any
   surveyed provider, and seanime's `$store` is not persistent — the map's
   "Extension storage and logs" item has no precedent to copy here.
6. Does manual search need a torrent **page** link, or is `torrent_url` plus
   magnet enough? seanime keeps both.
