# How seanime's debrid providers check cache, add, resolve, stream and download

Answers [#294](https://github.com/buildxlabs/autumn/issues/294) for ROADMAP
Phase 5 ("Debrid, online streaming, relay"). Researched 2026-11-28.

## Verdict

Seanime puts four debrid services behind one Go `Provider` interface
(`GetSettings`, `Authenticate`, `AddTorrent`, `GetTorrentStreamUrl`,
`GetTorrentDownloadUrl`, `GetInstantAvailability`, `GetTorrent{,Info,s}`,
`DeleteTorrent`). Each provider owns its own HTTP client, API-key handling,
status normalisation and link resolution; a `client.Repository` holds exactly
one active provider and adds the app-level flow around it. The recurring shape is:

1. **Cache check** — batched by infohash, but it is *not* uniform: TorBox has a
   real per-file cache endpoint, Premiumize a per-hash aggregate, Real-Debrid's
   endpoint is gone (seanime disabled it) and AllDebrid is unused. Cache status
   is only used to *reorder* auto-select candidates, never to gate streaming.
2. **Add** — dedupe by infohash against the provider's own list, then submit the
   magnet/questionable `.torrent` URL. Only Real-Debrid supports selecting files;
   seanime selects **all** files there because partial selection makes RD
   re-download a cached torrent.
3. **Resolve / stream** — poll torrent status every 1.5 s (5 s for Premiumize)
   until `IsReady`, then convert the provider hoster link into a direct CDN URL
   (`unrestrict` / `requestdl` / `link/unlock` / `item details + folder/list`),
   validate that the URL is actually a video, and hand it to the player. Native
   playback gets a **loopback proxy URL with an HMAC token**; only the external
   player link gets the raw debrid URL (which for TorBox contains the API key in
   `?token=`).
4. **Download to library** — a queued DB row plus a one-minute poll loop, three
   attempts per file with backoff, staging in a hidden `.tmp-*` folder in the
   destination, optional zip/rar extraction, a move into the destination and
   progress events every 2 s. There is **no resume** (fresh `os.Create` on every
   attempt), no cross-restart continuation, and cancellation is an in-memory
   `context.CancelFunc`.
5. **Errors and limits** — only two typed errors exist (`ErrNotAuthenticated`,
   `ErrFailedToAuthenticate`); everything else becomes a formatted string.
   Documented 429s (RD 250/min, AllDebrid 12/s + 600/min, Premiumize
   `rate_limit_reached`, TorBox `createtorrent` 60/h uncached) are not mapped or
   retried with backoff; seanime uses a few ad-hoc `time.Sleep`s instead.

For Autumn the behavior is a useful template for the *flow*, but nearly every
security-relevant choice should be inverted: no API keys in SQLite (ADR 0002,
0012), no raw provider URL to the player (ADR 0005), and typed errors +
`net` rate limiting rather than string errors and sleeps.

### Sources

- `seanime` (Go, GPL-3.0 — `LICENSE` at the clone root), shallow-cloned from
  <https://github.com/5rahim/seanime>, commit
  `d4d58b22fa690093da572083d163f60cc8725104` (2026-11-21, "bump version").
  All `path:line` citations below are in that commit, paths relative to the
  repository root. It is GPL-3.0, so its code may be ported with the original
  notice kept.
- Official API docs: Real-Debrid <https://api.real-debrid.com/>, AllDebrid
  <https://docs.alldebrid.com/>, TorBox <https://api-docs.torbox.app/>,
  Premiumize <https://www.premiumize.me/api>. Read 2026-11-28.
- Autumn's own decisions: `ROADMAP.md` §3 Phase 5, `ARCHITECTURE.md` §2–3 and §6,
  `GLOSSARY.md`, and ADRs 0002, 0005, 0006, 0007, 0010, 0011–0016.

## 1. The provider interface and its types

`internal/debrid/debrid/debrid.go:30-43` defines the interface. The methods
(and who calls them) are:

| Method | Called from | Purpose |
| --- | --- | --- |
| `GetSettings() Settings` | `client/repository.go:265` | stable id (`realdebrid`, `torbox`, `alldebrid`, `premiumize`, `dummy`) for DB rows and queue filtering |
| `Authenticate(apiKey)` | `client/repository.go:179` | store/verify the key |
| `AddTorrent(AddTorrentOptions)` | `client/stream.go:226`, `client/repository.go:254` | magnet → provider torrent id |
| `GetTorrentStreamUrl(ctx, StreamTorrentOptions, itemCh)` | `client/stream.go:299` | **blocking**; pushes status items on `itemCh`, returns a direct URL |
| `GetTorrentDownloadUrl(DownloadTorrentOptions)` | `client/download.go:144`, and inside each `GetTorrentStreamUrl` | direct URL(s) for local download |
| `GetInstantAvailability([]hash)` | `finder.go:59`, `finder.go:115` | cheap cached-hash lookup |
| `GetTorrentInfo(GetTorrentInfoOptions)` | `finder.go:133`, `autoselect/file_selection.go:224`, `previews.go:45` | file list for a magnet/hash *before* streaming |
| `GetTorrent`, `GetTorrents` | dashboard and download loop | status list |
| `DeleteTorrent(id)` | `client/stream.go:605`, `GetTorrentInfo` cleanup | remove from provider |

`AddTorrentOptions` carries `MagnetLink`, `InfoHash` and `SelectFileId`
(`:45-49`, marked "Real-Debrid only"); `StreamTorrentOptions` and
`DownloadTorrentOptions` carry `ID` + `FileId` (`:51-57`). `TorrentItem`
(`:60-80`) is the normalised dashboard/polling row: id, name, hash, size,
completion %, ETA, status, added-at, speed, seeders, `IsReady`, and two
seanime-specific flags `IsQueuedForLocalDownload` / `IsDownloadingLocally`.
`TorrentItemFile` is `{ID, Index, Name, Path, Size}` (`:82-88`); note `ID` is
provider-defined and is *not* always the index (TorBox uses the original file
name as the id, `torbox.go:658`; AllDebrid uses the flattened index,
`alldebrid.go:601`).

Status is a string enum (`:109-117`): `downloading`, `completed`, `seeding`,
`error`, `stalled`, `paused`, `other`. Only `completed` makes the stream path
continue (`realdebrid.go:756`, `alldebrid.go:776`); TorBox uses
`DownloadPresent` (`torbox.go:638`) and Premiumize uses
`finished|seeding` (`premiumize.go:770-772`). `FilterVideoFiles`
(`:119-127`) drops non-video files by extension after `GetTorrentInfo`.

Two package errors exist (`:15-19`): `ErrNotAuthenticated` and
`ErrFailedToAuthenticate`. There is no error-kind for cache miss, rate limit,
expired link, missing file or API failure.

## 2. Provider-by-provider details

### Real-Debrid (`internal/debrid/realdebrid/realdebrid.go`)

- Base `https://api.real-debrid.com/rest/1.0`, Bearer header, 10 s client
  timeout (`:89-98`, `:118-131`). Non-2xx bodies are decoded as
  `{error, error_details, error_code}` and wrapped into
  `fmt.Errorf("failed to query API: %s, %s", status, details)` (`:137-152`).
  A 429 is just another non-2xx; there is no `Retry-After` handling.
  `Authenticate` only stores the key (`:162-165`); no `/user` check.
- **Cache:** `GetInstantAvailability` is deliberately a no-op —
  `// Devnote: Endpoint doesn't exist anymore` (`:183-196`). The old batched
  implementation (100 hashes per call, retry with limit ÷ 10, only hoster
  `rd`) is still present as `getInstantAvailabilityT` (`:198-279`) but unreachable.
  The official docs list `/torrents`, `/torrents/info/{id}`,
  `/torrents/activeCount`, `/torrents/availableHosts` — no
  `/torrents/instantAvailability` route — and state a 250 req/min cap with HTTP
  429 for excess requests.
- **Add:** dedupe by infohash over `GET /torrents?limit=500` then a 1 s sleep,
  otherwise `POST /torrents/addMagnet` (`:281-319`, `:629-645`).
  `selectCachedFiles` always calls `_selectFiles(id, "all")`:
  *"Real Debrid will re-download cached torrent if we select only a few files"*
  (`:568-622`).
- **Ready + resolve:** `GetTorrentStreamUrl` (`:322-434`) polls
  `GET /torrents/info/{id}` immediately then every 1.5 s, gives up after 5
  consecutive status errors, emits each status to `itemCh`, and when
  `status == "downloaded"` matches the selected file id against the *index* of
  `ti.Links` and calls `POST /unrestrict/link` to get `download`.
  `GetTorrentDownloadUrl` (`:451-505`) returns one unrestricted URL for a
  `FileId`, or a comma-separated list for all selected files.
- `GetTorrentInfo` **adds the magnet then deletes it** to read the file list
  (`:518-546`); `toDebridTorrentStatus` maps `queued`/`downloading` →
  downloading, `waiting_files_selection`/`magnet_conversion` → stalled,
  `downloaded`/`dead` → completed, `uploading` → seeding (`:788-803`).
  `DeleteTorrent` is `DELETE /torrents/delete/{id}` (`:805-814`).

### TorBox (`internal/debrid/torbox/torbox.go`)

- Base `https://api.torbox.app/v1/api`, Bearer + `User-Agent: Seanime/…`,
  30 s client with pooled idle connections (`:100-114`, `:127-173`). Every
  response is `{success, detail, data}`; a non-success body is an error string.
- **Cache:** `GET /torrents/checkcached?hash=<csv>&format=list&list_files=true`
  in batches of 100, files keyed by their **index** (`:182-232`). Official docs
  confirm ~100 hashes per GET, `format=object|list`, `list_files`, an internal
  one-hour server cache, and O(log n) lookup.
- **Add:** dedupe by infohash over `/torrents/mylist?bypass_cache=true`, a
  500 ms "small delay to avoid rate limiting", then multipart
  `POST /torrents/createtorrent` (`:234-293`). No file selection: the official
  TorBox↔RD mapping table explicitly says `selectFiles` has no equivalent and
  "Torrents will download all files. This will not be changed."
- **Ready + resolve:** poll `GET /torrents/mylist?…&id=` every 1.5 s, 5 error
  retries, `IsReady = DownloadPresent` (`:296-377`, `:620-642`, `:676-698`).
  `GET /torrents/requestdl?token=<apikey>&torrent_id=…&file_id=…&append_name=true`
  returns a CDN string (`:379-428`); without a `FileId` it requests
  `zip_link=true`. `encodeFilename`/`normalizeDownloadUrl` (`:430-485`) escape
  the `filename=` query value so the player sees a usable name.
  Official docs: the `requestdl` link is open **3 hours to *start* a download**
  (once started it may run as long as needed); CDN links are not permanent, and
  `redirect=true` gives a permalink that mints a fresh CDN link. The docs also
  give `createtorrent` limits of 60/hour for uncached items and 300/min when
  cached.
- `GetTorrentInfo` tries `checkcached?format=object` first and falls back to
  `torrentinfo?hash=…&timeout=15`, including a workaround for a wrongly nested
  `data` object (`:515-577`). `DeleteTorrent` is `POST /torrents/controltorrent`
  with `operation=delete` (`:700-720`).

### AllDebrid (`internal/debrid/alldebrid/alldebrid.go`)

- Base `https://api.alldebrid.com/v4.1`, Bearer + `User-Agent`, 30 s (`:129-138`,
  `:193-231`). Uses `agent=Seanime` in the query for most calls (`:174-176`)
  although the official docs' 2025-01-15 changelog says agent/version are no
  longer required. `Authenticate` calls `GET /user` to validate (`:147-155`).
  Errors: HTTP status, then `status != "success"` → `error.message`.
- **Cache:** `GetInstantAvailability` returns an empty map (`:531-533`). There
  is no AllDebrid cache-check call in seanime.
- **Add:** dedupe by infohash over `/magnet/status`, then either
  `POST /magnet/upload` (multipart `magnets[]`) or, for an `http` src,
  `addTorrentFile` — `http.Get` the `.torrent`, optionally scrape a magnet out
  of an HTML page, then `POST /magnet/upload/file` (`:234-356`).
- **Ready + resolve:** poll `GetTorrent` every 1.5 s with **no retry cap** (unlike
  RD/TB), then `GetTorrentDownloadUrl` (`:358-427`). That method calls
  `/magnet/files`, flattens the nested `{n,s,l,e}` file tree (`flattenFileTree`,
  `:429-472`), resolves `FileId` by index or by substring of name, and calls
  `POST /link/unlock` for each file (`:474-529`). With no `FileId` it returns a
  comma-separated list.
- `GetTorrentInfo` adds the torrent and then waits **up to 5 minutes**, polling
  every 4 s for completion, before reading files and deleting it (`:543-614`);
  this is the slowest path in all four providers. `DeleteTorrent` is a `GET`
  with `apikey=` **in the query string** (`:641-657`). Status mapping uses
  `statusCode`: 4 → completed, 0–3 → downloading, 7 → error, plus a
  `strings.Contains(..., "error")` fallback (`:782-796`).
- Official docs: 12 req/s and 600 req/min, excess returns 429 or 503;
  `link/unlock` can return a `delayed` id requiring a follow-up flow, which
  seanime does not implement.

### Premiumize (`internal/debrid/premiumize/premiumize.go`)

- Base `https://www.premiumize.me/api`, Bearer + `User-Agent`, 30 s
  (`:151-161`, `:223-276`). Every response has a top-level `status`; non-success
  surfaces `message`. `Authenticate` verifies with `GET /account/info`
  (`:205-213`). Official docs confirm Bearer is recommended and that business
  errors still return HTTP 200.
- **HashStore (unique constraint):** the API never returns a transfer's
  infohash. `HashStore` (`:23-37`) persists `transferId → hash`, seeded into an
  in-memory cache at construction (`:151-175`, `:179-196`); seanime's
  `premiumizeHashStore` writes it to SQLite (`client/repository.go:205-231`).
- **Cache:** `POST /cache/check` with `items[]=magnet:?xt=urn:btih:<hash>` in
  batches of 100 (`:431-491`). The API returns parallel `response/filename/filesize`
  arrays and only an **aggregate** name/size per hash, so seanime emits one
  synthetic cached file with id `"0"`. `flexibleInt` (`:110-137`) exists because
  `filesize` sometimes arrives as a quoted string — the official docs' example
  indeed shows `"12345678901"`.
- **Add:** dedupe **only** via the local `hashCache`, then `POST /transfer/create`
  with `src=magnet` (`:280-319`).
- **Ready + resolve:** `GetTorrentStreamUrl` uses a 5 s ticker (not 1.5 s), an
  extra 1 s sleep once ready, and no 5-error cap (`:342-390`).
  `GetTorrentDownloadUrl` (`:394-429`) resolves a finished transfer's files via
  `resolveFiles` (`:706-730`): a single `FileID` → `GET /item/details`; a
  `FolderID` → recursive `GET /folder/list` (`listFolderFlat`, `:732-768`).
  `FileId` is parsed as an index. `isReady` is `finished|seeding` (`:770-772`).
- **`GetTorrentInfo` deliberately has no side effects** (`:524-596`): it checks
  an existing transfer via the hash cache, otherwise tries
  `POST /transfer/directdl` (which the official docs describe as resolving links
  *without storing* the transfer), and otherwise fails with "not cached and
  cannot be previewed without downloading it first". The comment explicitly
  says auto-select calls this for multiple candidates and must not start real
  downloads. This is the opposite of RD/AllDebrid and is the behavior Autumn
  should copy.
- Official docs group error codes by retry semantics (`transient`,
  `semi-permanent` incl. `rate_limit_reached`, `permanent`); seanime ignores the
  code and shows only `message`.

### Dummy (`internal/debrid/dummy/dummy.go`)

A feature-flagged test provider that can simulate cached/ready delays, progress,
bandwidth and local fallback files (`client/repository.go:160-166`,
`models.go:577-588`). Useful precedent for Autumn's offline UI tests, but it is
not part of the production contract.

## 3. Cache check → selection

`client/finder.go:27-106` (`findBestTorrent`) loads the persisted auto-select
profile or defaults to `1080p`, then supplies a `postSearchSort` hook that calls
`GetInstantAvailability` for all search-result hashes and annotates each torrent
with `IsCached`. `autoselect/comparison.go:384-457` (`smartCachedPrioritization`)
then reorders as **high-quality cached → uncached → low-quality cached**, where
"high-quality" means a profile score ≥ 70% of the top score. So a cached copy
wins, but a bad cached copy does not beat a well-scoring uncached one. The top
three candidates are then verified at file level
(`autoselect/file_selection.go:20`, `:40-98`), mirroring ADR 0007's "verify the
top three, never silently relax" rule; the debrid branch
(`selectFileFromDebrid`, `:199-270`) gets the file list from
`GetTorrentInfo`, parses every path with `habari`, matches by AniDB episode and
returns `DebridFileID`.

Manual selection (`finder.go:109-197`) warns but continues if the hash is not
cached, resolves the magnet from the search result, asks `GetTorrentInfo` for
files, uses the file directly if there is only one, and otherwise runs the same
`torrentanalyzer` + `GetFileByAniDBEpisode`. The `/torrents/file-previews`
endpoint (`client/previews.go:38-…`) returns parsed file previews with relative
episode numbers and `IsLikely` so the user can pick a file by hand. Batch info
is recorded after a successful start via the shared `torrentstream` history
table (`client/stream.go:555-563`, `db_bridge/torrentstream_history.go:30-62`).

## 4. Ready → resolve → player

`client/stream.go:88-580` is the orchestrator:

1. Cancel any previous download/playback contexts; resolve media + metadata.
2. Auto-select or manual file pick (above).
3. `AddTorrent` with the magnet, infohash and `SelectFileId` (`:226-230`).
4. Start a goroutine with a `itemCh` used only to push polling progress to the
   UI (`:279-295`), then call the blocking `GetTorrentStreamUrl` (`:299-302`).
5. Optionally verify the URL with `CanStream` — archives rejected, known video
   extension accepted, otherwise HEAD then a `Range: bytes=0-511` GET +
   `http.DetectContentType` (`stream_helpers.go:53-141`). The hook defaults to
   **4 retries × 8 s** (`:331-382`).
6. Emit `DebridStreamState` events (`started`, `downloading` with %, `ready`,
   `failed`) and send the URL to the player (`:384-427`).

`PlaybackType` decides the handoff (`:79-85`, `:429-553`):

- **`default`** — subscribe to playback status and call
  `StartStreamingUsingMediaPlayer(url, userAgent, clientId, …)`. The stream URL
  is cleared on `StreamStoppedEvent` (`:455-505`).
- **`externalPlayerLink`** — send the **raw** debrid URL over
  `ExternalPlayerOpenURL` (`:507-527`). For TorBox this URL embeds the API key,
  so it is a credential leak into the player's history/log.
- **`nativeplayer`** — register an open-step callback and call
  `directStreamManager.PlayDebridStream` (`:528-553`).
- **`none` / `noneAndAwait`** — just report "ready", optionally block until the
  worker closes `readyCh`.

`cancelStream` (`:582-610`) closes the native-player open, cancels the worker
context, clears the cached URL, and optionally calls `DeleteTorrent`. There is
no re-resolve on link expiry: the worker resolves once, then hands the URL off.

### The loopback proxy

`directstream/debridstream.go:21-42` makes a `DebridStream` an `httpBaseStream`
with `PlaybackTypeDebrid`; `PlayDebridStream` (`:56-98`) builds it from the
resolved URL and selected file path and calls `loadStream`.

`httpstream.go:127-201` does **not** give the player the provider URL: the
`PlaybackInfo.PlaybackURI` is
`{{SERVER_URL}}/api/v1/directstream/stream?id=<uuid>&<HMAC token query>`
(`:148`). MKV metadata/subtitle extraction is done server-side only when the
core needs it; mpv just gets the byte proxy (`:164-195`). The handler at
`:205-305` answers HEAD with a synthetic `Content-Length` and `Accept-Ranges`,
forwards the player's `Range` header, strips hop-by-hop headers
(`proxyHopHeaders`, `:41-56`), applies stream-specific headers, proxies through
a dedicated `videoProxyClient` (`:31-39`), rejects non-2xx CDN responses, and
writes bytes into a shared file-backed `httputil.FileStream` cache
(`:307-357`). `Close`/`Terminate` drop that cache (`:96-125`). The route is
served by `ServeEchoStream` (`serve.go:14-19`).

**Link lifetime in practice.** Seanime resolves a fresh provider URL at stream
start and holds it in `httpBaseStream.streamUrl`. Official lifelimits: TorBox
CDN links are open for 3 h to *start* (with `redirect=true` permalinks for
minting fresh ones); the other three return an on-demand link with no stated
window in seanime's code. On expiry mid-playback the proxy forwards a fresh
`Range` request and receives a non-2xx, which becomes `CDN error: <status>` —
there is **no refresh-and-retry** path (`httpstream.go:284-289`). This is the
clearest behavioral gap for Autumn's "refresh transparently before fallback"
exit criterion.

## 5. Download to library

- **Queueing.** `AddAndQueueTorrent` (`client/repository.go:235-279`) requires an
  absolute destination, calls `AddTorrent`, and stores
  `{TorrentItemID, Destination, Provider, MediaId}` in `debrid_torrent_items`
  (SQLite). Hooks can override the add call entirely.
- **Loop.** `launchDownloadLoop` (`download.go:33-53`) wakes every **minute**;
  `processQueuedDownloads` (`:55-100`) filters by active provider id, skips
  entries already in the in-memory `ctxMap`, waits for `IsReady`, then downloads
  and on success marks the auto-downloader item downloaded and removes the DB
  row. Stopping the provider cancels the loop (`repository.go:110-122`).
- **Fetching.** `downloadTorrentItemThen` (`:117-226`) registers a cancellable
  context in `ctxMap` (duplicate → `ErrDownloadAlreadyActive`), gets a
  comma-separated download URL string via `GetTorrentDownloadUrl`, and starts
  one goroutine per URL. Each URL gets `DownloadAttempts = 3` with
  `2n` second delays (`:26-30`, `:228-246`). The destination is created; a hidden
  `.tmp-*` staging dir is created *inside the destination* on desktop (app temp
  dir on mobile) and removed on exit (`:272-279`, `:506-511`).
- **Progress / cancellation.** A 32 KiB read loop writes to a plain file, emits
  `DebridDownloadProgress` at most every 2 s with aggregated bytes, total size
  and KB/s (`:363-406`), and verifies `totalBytes == Content-Length`
  (`:426-432`). `CancelDownload` (`repository.go:342-363`) calls the stored
  `CancelFunc` and emits `status: cancelled`; the read loop detects
  `context.Canceled` (`:412-417`). **No resume**: every attempt starts a fresh
  `os.Create`, there is no `.part` file and no `Range` continuation, and the
  queue is not re-driven after an app restart beyond re-reading DB rows (whose
  in-flight downloads were simply cancelled).
- **Final placement.** Filename comes from `Content-Disposition`, then the URL
  path, then a `Content-Type`-derived extension, defaulting to
  `downloaded_torrent` (`:303-345`). `.zip`/`.rar` are extracted into another
  temp dir (`client/utils.go:21-137`, with symlink/path-traversal rejection via
  `ResolveArchiveEntryPath`), then `moveContentsTo` flattens the single relevant
  folder/file into the destination (`utils.go:180-282`). A plain file is moved
  directly. Completion emits `DebridDownloadProgress{status: "completed"}` plus
  a hook and an OS notification (`download.go:218-222`, `:554-568`).

### Dashboard data

`GET /api/v1/debrid/torrents` (`handlers/debrid.go:295-339`) calls
`provider.GetTorrents()` and overlays two flags read from local state: whether
the id has a queued DB row (`IsQueuedForLocalDownload`) and whether it is in
`ctxMap` (`IsDownloadingLocally`). The other dashboard endpoints are
`GET/PATCH /debrid/settings`, `POST /debrid/torrents{,/download,/cancel,/info,
/file-previews}`, `DELETE /debrid/torrent`, and `POST /debrid/stream/{start,cancel}`
(`internal/handlers/routes.go:556-568`).

## 6. Settings, active provider, errors, rate limits

- `models.DebridSettings` (`internal/database/models/models.go:566-575`) holds
  `Enabled`, `Provider` (one string), **`ApiKey` in a `gorm:"column:api_key"`
  SQLite column**, `IncludeDebridStreamInLibrary`, `StreamAutoSelect` and
  `StreamPreferredResolution`. The key is returned by
  `GET /debrid/settings` (`handlers/debrid.go:24-31`) and saved by `PATCH`
  (`:40-59`). There is no keychain. A separate `DummyDebridSettings` row exists
  for the test provider.
- `Repository.provider` is a single `mo.Option[debrid.Provider]`
  (`repository.go:35-57`). `InitializeProvider` (`:137-194`) is called on every
  settings change; it closes the previous provider, constructs exactly one from
  `settings.Provider`, authenticates it (clearing the provider on failure) and
  restarts the download loop. **Only one provider can be active at once**; there
  is no fallback between providers and no per-provider key store.
- Error mapping is thin: unauthenticated and failed-authentication are typed;
  every API problem is `fmt.Errorf("…: %w", err)` with the provider's text.
  `FindBestTorrent` special-cases the literal string `"no torrents found"`
  (`finder.go:85-87`), which is brittle. Rate-limit handling is a pair of
  `time.Sleep`s (`realdebrid.go:297`, `torbox.go:248`), the 1.5 s poll cadence,
  and 3-attempt download retries — none keyed on a 429 or `Retry-After`.
  Documented limits are RD 250 req/min, AllDebrid 12 req/s + 600 req/min,
  TorBox `createtorrent` 60/h uncached / 300/min cached, Premiumize
  `rate_limit_reached` (semi-permanent).

## 7. What maps onto Autumn

| Seanime behavior | Autumn decision it informs |
| --- | --- |
| One `mo.Option[Provider]`, rebuilt on settings save | One active provider; debrid is in the live-reload list (`ARCHITECTURE.md` §6). Reuse `settings` + keychain-only secrets (ADR 0002, ADR 0012), never a DB `api_key` column or a settings API that returns it. |
| Provider-specific cache endpoints, RD endpoint gone, AD unused | Treat cached-ness as a hint, not a gate; keep the no-relaxing rule of ADR 0007. `debrid → search` is an allowed edge (`ARCHITECTURE.md` §3) so the provider consumes the shared result type. |
| `GetTorrentInfo` adds-then-deletes (RD, AD) vs. PM's side-effect-free preview | Prefer PM's contract: listing files must never start a download. A read that mutates the account is unacceptable for auto-select. |
| Poll to ready, then resolve to a CDN URL | `DebridProvider::resolve` is async and returns a direct URL; the engine owns the poll/cancel task (`bridge::run` + stored `Task`), not the UI. |
| Native player gets an HMAC loopback proxy; external link gets the raw URL | Extend ADR 0005: **all** debrid playback goes through the token-gated loopback relay, including external players; never put a provider URL (or `token=` API key) in the player's argv/history. |
| Single resolve, no expiry refresh; proxy returns `CDN error` | Autumn needs a re-resolve policy on 401/403/410 and a decision about retaining the same relay token across it (ties to the map's continuity/remote-stream question). |
| Queue in SQLite, 1-min loop, 3 attempts, no resume, `.tmp-*` staging, zip/rar extraction | Download-to-library is a separate explicit action from streaming (ADR 0006 keeps streams disposable). Decide whether Phase 5 needs resume/`.part` and archive extraction, or defers them to the Backlog multi-segment downloader. |
| String errors, ad-hoc sleeps | Use a per-crate `thiserror` enum and route all provider HTTP through `net`, with per-provider rate limits and typed `RateLimited { retry_after }`; ADR 0015's guard is for extensions, but the same client can throttle debrid traffic. |
| `HashStore` for Premiumize transfers | Autumn's shared `InfoHash` (`domain`) needs a decision: either persist a provider-id ↔ infohash cross-reference, or key debrid rows by provider id alone and accept that some providers cannot report a hash. |

## Open questions surfaced

1. **Is "cached" a gate or a hint?** RD has no cache endpoint, AD is unused by
   seanime, and PM reports only one aggregate file per hash. Should the Entry
   Debrid tab show "unknown" for those, or should Autumn probe by adding a
   torrent (which mutates the account) and then deleting it? What happens to
   ADR 0007's no-relaxing rule when cache data is incomplete?
2. **What exactly is in `DebridProvider`?** Does the trait include
   `list_torrents` for the dashboard and a status/progress stream, or does the
   engine poll `get_torrent`? Where does the shared `search::Torrent`/result type
   enter, and does `GetTorrentInfo`-style file listing return provider file ids
   or `domain::EpisodeNumber` + index?
3. **Link expiry mid-playback.** On a relay/proxy 401/403/410, should the engine
   re-resolve transparently and keep the same session token, or fail the stream
   and fall back? How does that interact with resume position, watched
   threshold and auto-next (the map's "Continuity and the player for remote
   streams" item)?
4. **External players.** ADR 0005 currently speaks of torrent streams. Does it
   cover debrid URLs too — i.e. is the raw provider URL ever handed to mpv/IINA,
   or is the relay mandatory for every source? (Seanime's TorBox link embeds the
   API key, so copying it is a credential leak.)
5. **Download resume and archives.** Is resume/`.part` in Phase 5 or left to the
   Backlog multi-segment downloader? Are `.zip`/`.rar` extraction and the
   "flatten to destination" heuristics in scope, or does Autumn download only
   selected video files?
6. **Rate-limit and error taxonomy.** Which provider limits does `net` enforce
   (per provider or per account), what is the backoff on 429, and which errors
   are user-visible vs. retryable vs. auto-disable? Should a repeated
   auth-failure disable the provider until reconfigured?
7. **Provider state model.** GLOSSARY reserves `preparing`/`downloading`/
   `streaming`/`paused`/`seeding` for `Torrent state`. Does debrid reuse that
   enum, or get its own (seanime has `waiting_files_selection`,
   `magnet_conversion`, `dead`, `cached`, `stalled (no seeds)` and maps them
   lossily)?
8. **Premiumize hash persistence.** The API cannot report a transfer's
   infohash, so Autumn must either persist a transfer-id ↔ infohash
   cross-reference (new DB row, no new crate edge) or give up hash-based
   dedupe/auto-downloader matching for that provider. Which one?
