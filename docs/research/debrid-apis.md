# Debrid provider APIs for Phase 5

Answers [issue #280](https://github.com/buildxlabs/autumn/issues/280) for
ROADMAP Phase 5 ("Debrid, online streaming, relay"). Read 2026-11-28.

Scope: public API documentation and public terms only. No accounts were
created, no credentials were used and no provider service was called with a
user token. Anything not stated in a primary source is marked **unverified**
or moved to [Open questions surfaced](#10-open-questions-surfaced).

Primary sources, all read 2026-11-28:

- Real-Debrid API documentation: <https://api.real-debrid.com/> (REST 1.0 and
  OAuth 2.0 reference); terms: <https://real-debrid.com/terms>.
- TorBox API documentation: <https://api-docs.torbox.app/docs> and the linked
  operation pages; API and Developer Terms (last updated 2026-07-31),
  retrieved from the policy page's own JS bundle at
  `https://torbox.app/assets/API-fce1409b.js`.
- AllDebrid API v4 documentation: <https://docs.alldebrid.com>; terms:
  <https://alldebrid.com/tos>.
- Premiumize API documentation: <https://www.premiumize.me/api>; terms
  (last updated 2026-11-19): <https://www.premiumize.me/legal>; pairing help:
  <https://premiumize.reamaze.com/kb/technical/pair-app>.
- Autumn context: `ROADMAP.md` §3 Phase 5, `ARCHITECTURE.md` §2–3 and §6,
  `GLOSSARY.md`, and ADRs 0002, 0005, 0006, 0007, 0010 and 0011–0016.

## 1. Verdict

- **Only two providers offer a real pre-add cache probe.** TorBox
  (`checkcached`) and Premiumize (`cache/check`) can answer "is this already
  in your cache?" before anything is added. Premiumize labels its answer
  best-effort; TorBox is the strongest signal and can also add with
  `add_only_if_cached`, making the check atomic.
- **AllDebrid has no standalone probe.** Its `magnet/upload` response carries
  a `ready` boolean, so the truth arrives _after_ the add. There is no
  `/magnet/check` in the current v4 docs.
- **Real-Debrid has no cache probe at all.** `torrents/instantAvailability`
  was disabled (error code 37, "Disabled endpoint") and no replacement
  endpoint exists in the current REST 1.0 reference. The only truthful RD
  signal is to add the magnet and read the resulting torrent's `status`:
  `downloaded` means instant, `magnet_conversion`/`queued`/`downloading`
  means it is being fetched. That mutates the account and consumes an active
  slot.
- Consequently the Phase 5 exit criterion ("a cached debrid torrent starts
  playing within a few seconds") is straightforward on TorBox and
  Premiumize, achievable with an add-then-observe flow on AllDebrid, and a
  policy decision on Real-Debrid: show an unknown badge, or optimistically
  add and clean up. This is the single biggest open decision.
- All four are ordinary HTTPS REST APIs with Bearer/API-key auth and simple
  JSON; all four support some device/PIN flow, so a desktop app never needs
  to ask for the account password. Every credential must go to the OS
  keychain only (ADR 0002).
- Real-Debrid is the only one with an explicit pre-start **file selection**
  step. TorBox downloads all files; AllDebrid and Premiumize expose the
  files after the transfer is ready.
- Delivery-link lifetime is documented only by TorBox ("opens the link for 3
  hours ... once a download is started ... nearly unlimited time"). The
  other three publish no TTL. Byte-range support on the four download hosts
  is **not documented** by any provider, and every provider's terms restrict
  account/link sharing and multi-IP use, so an HTTP proxy that changes the
  egress IP is a real correctness _and_ terms risk (ADR 0010). Both need
  account-based testing before Phase 5 is called done.

## 2. Real-Debrid

### Auth, base URL, limits

| Item        | Value                                                                                                          |
| ----------- | -------------------------------------------------------------------------------------------------------------- |
| REST base   | `https://api.real-debrid.com/rest/1.0/`                                                                        |
| OAuth base  | `https://api.real-debrid.com/oauth/v2/`                                                                        |
| Auth        | `Authorization: Bearer <token>`, or `?auth_token=` when a header is impossible                                 |
| Token kinds | private API token from <https://real-debrid.com/apitoken>, or an OAuth2 token                                  |
| Rate limit  | **250 requests/minute**; refused calls return 429 and still count; brute forcing gets an undefined block       |
| Response    | JSON on success, HTTP 4xx/5xx with `error` and optional numeric `error_code`; ETag / `If-None-Match` supported |

For a desktop app the documented path is to create an app in the control
panel and use **OAuth2 for devices**: `GET /oauth/v2/device/code?client_id=…`,
show `user_code` and `verification_url`, then poll `/oauth/v2/token` with
`grant_type=http://oauth.net/grant_type/device/1.0`. Open-source apps use
`new_credentials=yes` and `/device/credentials` to obtain a user-bound
client id/secret, because no secret can be shipped in the binary. The docs
publish a shared open-source client id `X245A4XAIBGVM` (scopes: unrestrict,
torrents, downloads, user) but warn it can have stricter limits than the
service limit. The private API token must never be used for a distributed
application.

### Operations

| Need                  | Endpoint                                                             | Notes                                                                                                                                                                                              |
| --------------------- | -------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Cache check           | **none**                                                             | `GET /torrents/instantAvailability/{hash}` is gone; current docs have no replacement. Error code 37 is "Disabled endpoint".                                                                        |
| Add magnet            | `POST /torrents/addMagnet`                                           | `magnet`, optional `host`; 201; returns `{id, uri}`                                                                                                                                                |
| Add torrent file      | `PUT /torrents/addTorrent`                                           | `host`; 201                                                                                                                                                                                        |
| Select files          | `POST /torrents/selectFiles/{id}`                                    | `files` = comma-separated file IDs or `all`; IDs from `/torrents/info/{id}`; 204                                                                                                                   |
| Status / poll         | `GET /torrents/info/{id}`, `GET /torrents`                           | statuses: `magnet_error`, `magnet_conversion`, `waiting_files_selection`, `queued`, `downloading`, `downloaded`, `error`, `virus`, `compressing`, `uploading`, `dead`; `links[]` appear when ready |
| Slot count            | `GET /torrents/activeCount`                                          | `{nb, limit}`                                                                                                                                                                                      |
| Unrestrict            | `POST /unrestrict/link`                                              | `link` from the torrent's `links[]`; optional `remote=1` lifts dedicated-server and account-sharing protection (paid Remote traffic); returns `download` URL                                       |
| Pre-check hoster link | `POST /unrestrict/check`                                             | unauthenticated, 503 if unavailable                                                                                                                                                                |
| Streaming variants    | `GET /streaming/transcode/{id}`, `GET /streaming/mediaInfos/{id}`    | HLS/MPD/live-MP4/H264-WebM links and media info                                                                                                                                                    |
| List downloads        | `GET /downloads`                                                     | `offset` or `page`, `limit` ≤ 5000                                                                                                                                                                 |
| Delete                | `DELETE /torrents/delete/{id}`, `DELETE /downloads/delete/{id}`      | 204                                                                                                                                                                                                |
| Account / quota       | `GET /user`, `GET /traffic`, `GET /traffic/details`, `GET /settings` | `premium` seconds, `expiration`, `type`; traffic `left`/`limit` per host and reset period                                                                                                          |

**What "cached" means here:** only _after_ adding. A torrent that lands in
`downloaded` with links already present was cached; `magnet_conversion` or a
download means it was not. Reliability before adding: **none**. The
third-party DMM plan records the endpoint as disabled since 2024-11-22 with
nothing replacing it, and also reports undocumented practical spacing of
about 5 s between `/unrestrict/link` calls and 20 s between addMagnet calls;
that spacing is not in the official docs and is **unverified**.

**Link/IP/proxy/Range:** the docs publish no TTL for `download` URLs and no
Range statement. `remote=1` is the only documented knob for server traffic.
Error 22 is "IP Address not allowed". The terms forbid VPS / dedicated
server / cloud use except via Remote traffic, forbid account sharing and
generated-link sharing, and say the account is for personal use only. That
argues against sending RD API or delivery traffic through a shared HTTP
proxy unless the user explicitly opts in.

## 3. TorBox

### Auth, base URL, limits

| Item        | Value                                                                                                                                                                |
| ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Base        | `https://api.torbox.app`, all paths under `/v1/api/`; OpenAPI 3.1 at `/openapi.json`                                                                                 |
| Auth        | `Authorization: Bearer <api key>` for most calls; the four `requestdl` endpoints take `token=` as a query parameter                                                  |
| Device flow | `GET /v1/api/user/auth/device/start`, then `POST /v1/api/user/auth/device/token` (30/min **per IP**)                                                                 |
| OAuth       | RFC 7591 registration at `POST /register`, PKCE at `/authorize`, token at `/token`; the returned access token **is** the user's API key, no expiry, no refresh token |
| Limits      | 300/min per token for everything else; `createtorrent`/`asynccreatetorrent` add 60 uncached/hour; `X-RateLimit-*` and `Retry-After` headers; no IP whitelisting      |
| Response    | `{success, error, detail, data}` envelope                                                                                                                            |

The API key gives full account access. The docs explicitly say not to share
a `token=` URL: it contains the key. Any resolved delivery URL must be
redacted before logging or issue reporting (ARCHITECTURE §7).

### Operations

| Need                | Endpoint                                      | Notes                                                                                                                                                                                                      |
| ------------------- | --------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Cache check         | `GET /v1/api/torrents/checkcached`            | `hash` (comma-separated, ~100 per request; repeat `hash` for more), `format=object\|list`, `list_files=true`; ~1 s/100, cached by TorBox for 1 hour                                                        |
| Cache check (batch) | `POST /v1/api/torrents/checkcached`           | JSON `{hashes: [...]}`; nearly unlimited hashes; same `format`/`list_files`                                                                                                                                |
| Add                 | `POST /v1/api/torrents/createtorrent`         | multipart `magnet` **or** `file`, plus `seed`, `allow_zip`, `name`, `as_queued`, `add_only_if_cached`; 404 `DOWNLOAD_NOT_CACHED` when that flag is set and the item is not cached; async variant available |
| Select files        | **none**                                      | TorBox downloads all files; `PUT /v1/api/torrents/edittorrent` only edits name, tags, alternative hashes and `airlocked`                                                                                   |
| Status / poll       | `GET /v1/api/torrents/mylist`                 | `offset`, `limit` (default 1000), `bypass_cache`; the list itself is cached for 600 s; pass `id` for one object; states include `completed`, `cached`, `stalled (no seeds)`, `metaDL`                      |
| Torrent info        | `GET /v1/api/torrents/torrentinfo?hash=…`     | public BitTorrent lookup, 10 s default timeout; docs say **do not** use it to check TorBox's cache                                                                                                         |
| Control / delete    | `POST /v1/api/torrents/controltorrent`        | JSON `torrent_id` (or `all: true`) + `operation` including `delete`, `resume`, `reannounce`                                                                                                                |
| Unrestrict          | `GET /v1/api/torrents/requestdl`              | `token=`, `torrent_id`, `file_id` or `zip_link`, `user_ip`, `append_name`, `redirect`; CDN link                                                                                                            |
| Batch links         | `GET`/`POST /v1/api/torrents/requestdl/batch` | for many files at once                                                                                                                                                                                     |
| Account             | `GET /v1/api/user/me`                         | plan 0 free, 1 essential, 2 pro, 3 standard; `?settings=true&download_stats=true`                                                                                                                          |

**What "cached" means here:** `checkcached` answers per hash whether the
torrent is in TorBox's cache, optionally with the cached file list.
Reliability before adding: **high**, and `add_only_if_cached` can make
"check then add" atomic so an uncached item never consumes an active slot.
The 1-hour server-side cache on `checkcached` means a just-expired cache
entry can lag reality.

**Link/IP/proxy/Range:** the only provider that states a lifetime: the
`requestdl` link "opens the link for 3 hours for downloads" and once a
download is started the time limit no longer applies. CDN links are not
permanent; the recommended pattern is a permalink of the form
`…/requestdl?token=…&torrent_id=…&file_id=…&redirect=true`, invalidated by
resetting the key or deleting the item. `user_ip` exists to choose the
nearest CDN when requesting on behalf of a user. Range support is not
documented. Rate limits are per token, not per IP, so an HTTP proxy does not
change the API limit, but delivery links carry the key and the API Terms
forbid exposing credentials in client-side code.

**Terms:** the API and Developer Terms (2026-07-31) allow private tools and
integrations with the user's own account, require secure credential storage
and accurate descriptions, and forbid scraping/indexing, sharing API access,
public or commercial applications without written authorization, and
creating a competing discovery or catalog service. Autumn is open source and
non-commercial, but whether "available to other users" makes it a _public
application_ under §9 is an open question worth grilling.

## 4. AllDebrid

### Auth, base URL, limits

| Item     | Value                                                                                                        |
| -------- | ------------------------------------------------------------------------------------------------------------ |
| Base     | `https://api.alldebrid.com/v4/`, with some endpoints at `/v4.1/`                                             |
| Auth     | `Authorization: Bearer <apikey>`; legacy `?apikey=` / POST `apikey=` still accepted                          |
| PIN flow | `GET /v4.1/pin/get` → show `pin` and `user_url` → poll `POST /v4/pin/check` every ~5 s for 10 min → `apikey` |
| Limits   | **12 requests/second and 600/minute**; over-limit returns 429 or 503                                         |
| Response | `{status: "success", data: …}` or `{status: "error", error: {code, message}}`; HTTP 2xx/4xx/5xx              |

Agent/version parameters were removed in January 2025; POST is now the
default way to send data. Demo keys (`staticDemoApikeyPrem`, etc.) exist for
read-only doc examples and return `demo: true`.

### Operations

| Need             | Endpoint                               | Notes                                                                                                                                            |
| ---------------- | -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| Cache check      | **none**                               | No `/magnet/check` in the v4 docs. `POST /v4/link/infos` reports filename/size for _hoster links_ only, not a torrent cache probe                |
| Add magnet       | `POST /v4/magnet/upload`               | `magnets[]` accepts magnet URIs or bare hashes; batch; returns `id`, `hash`, `size`, `ready`                                                     |
| Add torrent file | `POST /v4/magnet/upload/file`          | multipart `files[]`; same response shape                                                                                                         |
| Select files     | **none**                               | all files are processed; `POST /v4/magnet/files` returns the folder tree with per-file `l` links once ready                                      |
| Status / poll    | `POST /v4.1/magnet/status`             | optional `id`, `status` = `active\|ready\|expired\|error`; `statusCode` 0–15; Live Mode with `session` + `counter` returns deltas and `fullsync` |
| Restart          | `POST /v4/magnet/restart`              | `id` or `ids[]`                                                                                                                                  |
| Unrestrict       | `POST /v4/link/unlock`                 | `link`, optional `password`; returns `link`, `id`, `filesize`, `streams[]`, optional `delayed`                                                   |
| Quality variant  | `POST /v4/link/streaming`              | `id` + `stream` from `/link/unlock`                                                                                                              |
| Delayed link     | `POST /v4/link/delayed`                | poll at least every 5 s until `status` 2                                                                                                         |
| Delete           | `POST /v4/magnet/delete`               | `id`                                                                                                                                             |
| Account / quota  | `GET /v4/user`, `GET /v4.1/user/hosts` | `isPremium`, `premiumUntil`, `limitedHostersQuotas`; per-host `quota`, `quotaMax`, `quotaType`, `limitSimuDl`, reset daily for premium           |

**What "cached" means here:** `ready: true` on the magnet upload response
means the magnet was already available on AllDebrid. Reliability before
adding: **none**; availability is learned from the add itself, and a false
`ready` is not documented as impossible. Active magnets are capped at 30
(`MAGNET_TOO_MANY_ACTIVE`).

**Link/IP/proxy/Range:** no TTL and no Range statement in the docs; delayed
links exist for slow hosters. A new device, app or network can raise
`AUTH_BLOCKED` and require an email confirmation token via
`/v4/user/verif`. The terms allow personal use only and explicitly forbid
dedicated servers, proxies, VPS, cloud services and company networks. Using
Autumn's global HTTP proxy for AD traffic is therefore not a neutral
technical choice.

## 5. Premiumize

### Auth, base URL, limits

| Item     | Value                                                                                                                                                                      |
| -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Base     | `https://www.premiumize.me/api/`                                                                                                                                           |
| Auth     | `Authorization: Bearer <api key or OAuth access token>` (recommended); legacy `?apikey=`, `?pin=`, POST `apikey=`/`pin=` and the website session cookie                    |
| OAuth    | register a client at `/registerclient`; PKCE at `/authorize` + `/token` (recommended for native desktop), device code at `/token`, plus legacy flows; only scope is `full` |
| Limits   | no numeric request limit in the docs; `rate_limit_reached` is a documented error code; fair-use points are the real budget                                                 |
| Response | JSON `{status: "success", …}` or `{status: "error", message, code}`; business errors are HTTP 200, catastrophic failures HTTP 500                                          |

The API docs explicitly warn that undocumented response fields are either
legacy or experimental and must not be relied on. Several formerly
documented fields (`directlink`, `stream_link`, `transcode_status`,
`space_used`) are deprecated in favour of plain `link` and fair-use values.

### Operations

| Need                 | Endpoint                                                                  | Notes                                                                                                                               |
| -------------------- | ------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| Cache check          | `POST /api/cache/check`                                                   | `items[]` of links → parallel `response`, `filename`, `filesize` arrays; docs: "A hit is not guaranteed; the cache is best-effort"  |
| Service capabilities | `GET /api/services/list`                                                  | which services appear in `cache`, `directdl`, `queue`, with `fairusefactor`, aliases and regex patterns                             |
| Add (store)          | `POST /api/transfer/create`                                               | `src` (URI string or multipart upload), optional `folder_id`, `password`; returns `id`, `name`                                      |
| Add (no store)       | `POST /api/transfer/directdl`                                             | `src` → `content[]` of `path`, `size`, `link` for every file in a magnet/torrent                                                    |
| Select files         | **none**                                                                  | `directdl` lists all files; no pre-start selection endpoint                                                                         |
| Status / list        | `GET /api/transfer/list`                                                  | all transfers, newest first; `queued`, `running`, `finished`, `seeding`, `error`; `progress` 0–1; **not cached**, meant for polling |
| Retry / delete       | `POST /api/transfer/retry`, `/transfer/delete`, `/transfer/clearfinished` | by `id`                                                                                                                             |
| Resolve link         | `GET /api/folder/list`, `GET /api/item/details`, `GET /api/item/listall`  | file entries carry the CDN `link`; transcode/directlink aliases are deprecated                                                      |
| ZIP                  | `POST /api/zip/generate`                                                  | sync; returns `location`; resuming and multi-part downloads work, but the client should fetch with one connection                   |
| Account / quota      | `GET /api/account/info`                                                   | `customer_id`, `premium_until`, `limit_used` (fair-use fraction 0–1), `booster_points`                                              |

**What "cached" means here:** `/api/cache/check` checks a _link_, not an
infohash, against Premiumize's cache. `cache` in `/services/list` tells which
services can be looked up. Reliability before adding: **probabilistic by
documentation** ("a hit is not guaranteed"). A miss does not prove the
content is absent, and a hit does not guarantee the exact file layout.

**Link/IP/proxy/Range:** the API docs set no TTL. The ZIP endpoint is the
only place that documents resumability. Premiumize help says links are
device-specific and that generated download links must not be shared, and
the ToS prohibit using the service from several IPs in a short period,
sharing an API key, and "pairing" apps not used exclusively by the user.
The ToS (2026-11-19) set a fair-use budget of 1,000 points, replenished by
100 points each day at midnight UTC, with premium membership; third-party
apps are used at the user's own risk and are not supported. API-key paired
apps cannot be revoked individually; OAuth/device-paired apps can be revoked
at `/connected`, which favours the device flow for a desktop client.

## 6. Cross-provider "cached" semantics

| Provider    | Pre-add probe                                            | What a positive looks like                      | Reliability before add         | After-add truth                                                                 |
| ----------- | -------------------------------------------------------- | ----------------------------------------------- | ------------------------------ | ------------------------------------------------------------------------------- |
| Real-Debrid | none (endpoint disabled, no replacement)                 | n/a                                             | none                           | `status == downloaded` with `links[]` already present                           |
| TorBox      | `GET`/`POST /torrents/checkcached`; `add_only_if_cached` | per-hash cached flag, optional cached file list | high; 1 h server cache can lag | `mylist` state `cached`/`completed` plus `download_finished`/`download_present` |
| AllDebrid   | none in v4                                               | n/a                                             | none                           | `ready: true` on upload, then `statusCode 4` (`Ready`)                          |
| Premiumize  | `POST /cache/check`                                      | parallel boolean per `items[]`                  | best-effort by docs            | `transfer/list` `finished`/`seeding` plus a `link` from folder/item             |

Practical consequence for `DebridProvider`: the cache-check method cannot be
uniformly "does a hash exist in the cache?" RD has no such operation and PM
checks links. A shared result type needs at least `Cached`, `NotCached`,
`Unknown` and, ideally, the cached file list when the provider supplies it
(TorBox `list_files`, Premiumize `cache/check` filenames). Non-probing
providers must either return `Unknown` or be driven through an add-then-poll
flow that owns cleanup.

## 7. Implications for the Phase 5 design

These are observations for the upcoming decisions, not decisions themselves.

- **Trait shape.** A `DebridProvider` trait has to absorb asymmetry:
  RD needs a file-selection call; TB/AD/PM do not. TB/PM can pre-check,
  AD/RD cannot. PM's cache check takes links, not hashes. Forcing one
  "cache check by infohash" signature would either lie for RD/PM or hide
  useful TB data.
- **File pick on RD.** seanime selects _all_ files on Real-Debrid because a
  subset can make RD re-download an already cached torrent
  (`realdebrid.go:568-575`, §8). Autumn's RD file-selection UI needs a
  deliberate position on that tradeoff rather than a generic file picker.
- **Secrets.** All four accept a long-lived credential that is equivalent
  to the account. Keep it in the OS keychain only, as with AniList (ADR 0002) and extension secrets (ADR 0012). Prefer device/PIN/pairing flows
  over a pasted private token: RD explicitly warns against shipping its
  private API token, and PM's API-key pairing cannot be revoked per app.
- **No scraping.** The provider APIs are the only source. This respects
  ADR 0015's outbound-guard spirit and the TorBox terms' ban on scraping
  and index building.
- **Start-to-play path.** Cache probe → add → (RD only) select files → poll
  → resolve a delivery link → hand the player a URL. The delivery URL is
  short-lived or credential-bearing on all four, so it must be resolved
  close to play time and re-resolved on failure, not persisted. This is the
  same "stale URL refresh then fallback" shape `onlinestream` needs, and it
  argues for one shared re-resolution policy.
- **Relay.** ADR 0005 requires a token-gated loopback relay for torrent
  streams; debrid streams are plain HTTPS file URLs and do not need a
  torrent range reader, but they can still go through the relay if header
  injection, retry/re-resolve or proxy policy requires it. No provider
  documents Range on its delivery host, so ranged sub-requests and a
  graceful fallback must be verified against real accounts before the relay
  rewrite is finalised.
- **Cleanup ownership.** Adding can return an _existing_ torrent on TorBox,
  and AllDebrid/Premiumize transfers are account-wide. Autumn must snapshot
  what it added and never delete a transfer or torrent it did not prove it
  created; this matters most for RD's optimistic-cache option.
- **Proxies (ADR 0010).** `net` has one global HTTP proxy and a separate
  torrent SOCKS5 proxy. A global proxy changes the egress IP for provider
  API and delivery traffic. RD and AD terms restrict server/proxy use, PM
  restricts multiple IPs in a short time, and all four tie links to an
  account/IP in practice. A fail-closed warning and an explicit user choice
  are safer than silently routing debrid traffic through the global proxy.

## 8. Reference implementations (behavior)

Read from shallow clones in a scratch directory outside this repo on
2026-11-28. Paths are relative to each clone root; all line numbers refer to
the commit named below. Licences were confirmed from each `LICENSE` file.
Code below informs **behavior** only: no code was copied into Autumn, and
nothing here changes the GPL-3.0-only / MIT / Apache-2.0 porting rule in
`AGENTS.md`.

| Reference          | Clone URL                                     | Commit read                                             | Licence confirmed                                                                                               |
| ------------------ | --------------------------------------------- | ------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| seanime            | <https://github.com/5rahim/seanime>           | `d4d58b22fa690093da572083d163f60cc8725104` (2026-11-21) | `LICENSE`: GPL-3.0                                                                                              |
| MovieBox-Tui       | <https://github.com/mesamirh/MovieBox-Tui>    | `b93f4fc25f1beff202f46aa26c5f7900a4cac756` (2026-11-27) | `LICENSE-APACHE` and `LICENSE-MIT`; `Cargo.toml:22` says `MIT OR Apache-2.0`, so Apache-2.0 terms are available |
| VortX              | <https://github.com/VortXTV/VortX>            | `47a19146ace1e821231d577d7b7e95af6e1f1921` (2026-11-22) | `LICENSE`: GPL-3.0                                                                                              |
| Zangetsu           | <https://github.com/Spyou/Zangetsu>           | `4e096170349592e1df02c0ff8874313090ef7a87` (2026-11-28) | `LICENSE`: GPL-3.0 (the Apache/MPL files at root are third-party notices)                                       |
| Zangetsu providers | <https://github.com/Spyou/zangetsu-providers> | `1e4e39d887991455832a9f72856af8e44ada6d3f` (2026-11-26) | **No LICENSE file**; `README.md:116-118` claims MIT. Behavior-only until confirmed.                             |

### seanime

The provider contract is `internal/debrid/debrid/debrid.go:17-31`:
`GetSettings`, `Authenticate`, `AddTorrent`, `GetTorrentStreamUrl`,
`GetTorrentDownloadUrl`, `GetInstantAvailability`, `GetTorrent`,
`GetTorrentInfo`, `GetTorrents`, `DeleteTorrent`. `AddTorrentOptions`
carries `MagnetLink`, `InfoHash` and `SelectFileId`, the last marked
"Real-Debrid only" (`:33-37`). The four implementations are wired at
`internal/debrid/client/repository.go:151-169`.

**Real-Debrid's removed cache check, in code.**
`internal/debrid/realdebrid/realdebrid.go:183-196` now returns an empty map:

```go
// Devnote: Endpoint doesn't exist anymore
//return t.getInstantAvailabilityT(hashes, 3, 100)
return availability
```

The dead old call to `GET /torrents/instantAvailability/{hash...}` is still
at `:198-279` (request at `:219`). GitHub file history dates the disabling
to commit `30a7d691a1d8f986326988238718ca599921a758`,
2025-09-13T11:32:52Z; earlier `e945b9551c39` still called it live. This is
independent corroboration of the current docs' absence and error code 37.

What seanime actually does instead:

- Metadata/file-list preview is **add magnet → `GET /torrents/info/{id}` →
  delete the transfer** (`realdebrid.go:518-546`), used by autoselect
  (`internal/torrents/autoselect/file_selection.go:224-227`) and manual
  selection (`internal/debrid/client/finder.go:133-136`).
- Readiness comes from the info status: `toDebridTorrentStatus` maps
  `downloaded`/`dead` to `Completed`, `waiting_files_selection`/
  `magnet_conversion` to `Stalled` (`realdebrid.go:788-803`) and
  `IsReady` is `status == Completed` (`:756`).
- Selection always calls `_selectFiles(id, "all")` (`:568-575`): the old
  cached-files-only logic is commented out at `:577-609` because "Real
  Debrid will re-download cached torrent if we select only a few files".
  This matters for Autumn's file-pick UX: selecting a subset can cost
  instant availability.
- `finder.go:59-67` and `:115-119` still call `GetInstantAvailability` to
  sort candidates, but for RD it is now an empty map, so RD candidates are
  implicitly filtered by whether add+info reaches `Completed`.

Per-provider behavior at the same commit:

| Provider    | Cache check                                                                                                                                                                  | Add                                                                                                                    | Files                                                                              | Poll / ready                                                                                               | Resolve                                                                                          |
| ----------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| Real-Debrid | empty map (`:183-196`)                                                                                                                                                       | dedupe by hash then `POST /torrents/addMagnet` (`:281-307`)                                                            | `selectFiles` with `"all"` (`:571-575`)                                            | `/torrents/info/{id}` every 1500 ms, 5-error abort (`:336-344`, `:409-428`); ready when status `Completed` | match selected file id to `ti.Links[idx]`, then `POST /unrestrict/link` (`:380-392`, `:647-662`) |
| TorBox      | batches of 100 via `GET /torrents/checkcached?...&format=list&list_files=true` (`:182-232`); `/torrentinfo` fallback only for metadata (`:515-577`)                          | dedupe via `getTorrents()` + 500 ms sleep, then multipart `POST /torrents/createtorrent` (`:234-292`)                  | none; file id derived from `ShortName` at resolve (`:389-406`, `:657-663`)         | `/torrents/mylist?bypass_cache=true&id=…` (`:498-513`), ready = `DownloadPresent` (`:638`)                 | `GET /torrents/requestdl?token=…&file_id=…` or `zip_link=true` (`:379-428`)                      |
| AllDebrid   | empty map (`:531-533`)                                                                                                                                                       | `POST /magnet/upload` or `/magnet/upload/file`; the `Ready` field (`:41-54`) is ignored (`:237-285`)                   | none; `/magnet/files` tree flattened recursively (`:429-472`)                      | `POST /magnet/status` (`:661-703`), ready = `statusCode == 4` (`:776`, `:782-795`)                         | `POST /link/unlock`, file chosen by flattened index with name fallback (`:474-529`, `:728-747`)  |
| Premiumize  | `POST /cache/check` with `items[]=magnet:?xt=urn:btih:<hash>` in batches of 100 (`:431-491`); one aggregate hit per hash becomes a synthetic `CachedFiles["0"]` (`:482-486`) | `POST /transfer/create`; dedupe needs a custom hash store because the API never returns the transfer hash (`:280-319`) | none; `/item/details` or recursive `/folder/list` walked after finish (`:703-768`) | `GET /transfer/list` every 5 s + 1 s settle, ready = `finished`/`seeding` (`:341-390`, `:770-772`)         | the `link` field from item/folder listing; no unrestrict call (`:689-768`)                       |

No reference reads quota or account info beyond an auth validity check: AD
calls `/user` and PM calls `/account/info` but discards the data
(`alldebrid.go:147-155`, `premiumize.go:205-213`); RD and TB only store the
key.

**Relay / stream proxy.** The built-in player never gets a raw CDN URL:
`httpBaseStream.loadPlaybackInfo` builds
`{{SERVER_URL}}/api/v1/directstream/stream?id=<uuid>&token=<HMAC>`
(`internal/directstream/httpstream.go:137-156`). The token is endpoint-bound
with a 24 h TTL (`internal/util/hmac_auth.go:13-17, :46-71`), validated
against the request path with prefix matching (`:74-117`), and the server
binds `127.0.0.1:43211` by default (`internal/core/config.go:100-101`). The
handler answers `HEAD` locally with `Content-Length` and `Accept-Ranges`
(`httpstream.go:213-223`), strips hop headers, applies stream-specific
headers (`:41-60`, `:266-274`), forwards the client's `Range` verbatim
(`:225`, `:262-263`), rejects upstream responses ≥300 to avoid corrupting
its file-backed cache (`:284-289`) and serves from that cache with
`WriteAndFlush(body, w, ra.Start)` (`:291-303`). Debrid streams inject no
request headers (`internal/directstream/debridstream.go:73-91`).

There is a separate HLS proxy at `internal/handlers/proxy.go`: it accepts
`?url=`, JSON `?headers=` and `?token=` (`:206-230`), injects a Chrome UA
(`:459-464`), forwards `Range` (`:465-467`), and rewrites m3u8 playlist
URIs, keys, maps and master variants back through itself (`:288-370`,
`:409-421`, `:520-537`). It has **no DASH/MPD rewrite** and explicitly uses
`ProxyFromEnvironment` (`:77-100`). None of these proxies parses a link
expiry or IP binding; the only freshness mitigation is a 4×8 s `CanStream`
retry (`internal/debrid/client/stream.go:331-381`,
`client/stream_helpers.go:53-141`). External-player playback sends the raw
URL to the player (`client/stream.go:507-519`), so IP-bound links would be
a client-side problem there.

### MovieBox-Tui (Apache-2.0 available)

`src/proxy.rs` is a loopback sidecar (`--proxy-for-vlc`), not an in-process
middleware: `spawn_sidecar` launches the same binary with the target URL,
JSON headers, subtitle URL and max height, reads `PORT <n>`, and returns
`http://127.0.0.1:<port>/<scheme>/<original-url>` (`:105-175`); the child
binds `127.0.0.1:0` (`:177-200`) and exits after 600 idle seconds (`:14-15`,
`:204-221`).

Useful patterns for the Phase 5 relay:

- **Host allowlist:** a request path whose host differs from the target (or
  subtitle host) is `403`; auth headers are forwarded only to the target
  host (`:362-376`, `:286-288`, `:483-490`).
- **DASH rewrite:** MPD responses up to 10 MiB are rewritten so segment URLs
  point back at the loopback proxy, representations above `max_height` can
  be filtered, and a subtitle `AdaptationSet` is injected (`:513-567`,
  `:980-1108`). There is no HLS rewrite in this file.
- **Segment prefetch:** three `.m4s` segments ahead are fetched in the
  background with in-flight dedup (`:406-423`, `:677-735`, `:764-812`);
  cache caps at 24 segments × 16 MiB (`:17-18`, `:52-62`).
- **Ranged sub-requests:** `.m4s` chunks are fetched in 95 KiB pieces,
  parallelised 16 at a time, reassembled, then served from cache with
  `206` + `Content-Range` + `Accept-Ranges` (`:820-895`, `:437-459`).
  Ordinary streams pass upstream headers through (`:575-595`, `:899-943`).

### VortX

Correction to the map's premise: at the commit read, the native-player gate
is **not a token gate**. `is_loopback_torrent_url`
(`desktop/src-tauri/src/player.rs:305-340`) requires an exact bound
authority starting with `127.0.0.1:`, `http` scheme, no query string, and a
path of exactly `/<40-hex-infohash>/<u32-index>` (`:311-339`); `localhost`
is rejected because hosts-file resolution is mutable (`:324-325`). It is
enforced in `play_native` (`:404-420`) and Windows native playback is
disabled (`:385-394`). The authority comes from the in-process server
configured to `127.0.0.1:0` (`desktop/src-tauri/src/server.rs:52-86`). This
shows a loopback gate can be stronger than a bearer token; ADR 0005's
per-stream token remains Autumn's chosen shape.

### Zangetsu

The app ships no built-in providers (`pubspec.yaml:283`); the only debrid
source in the provider repo is TorBox (`index.json:86-92`). No
Real-Debrid, AllDebrid or Premiumize provider was found in either repo.

`providers/torbox.js` at `1e4e39d8…`:

- `cachedOnly` defaults true and `preferCachedFirst` defaults true (`:42`,
  `:118-121`, `:90-94`, `:151-154`); web HLS defaults off (`:37-41`).
- `_checkCachedFiles` POSTs `{hashes}` to
  `/torrents/checkcached?format=object&list_files=true`, falls back to GET,
  and normalises to a lowercase-hash map (`:717-744`). It is capped by mode
  (`:927-943`): all hashes only in full mode, `maxOut+3` in fast mode.
- `_createTorrent` sends `magnet=…&allow_zip=false` and adds
  `add_only_if_cached=true` whenever `cachedOnly` is set (`:746-751`);
  `DUPLICATE_ITEM` with a recoverable id counts as success (`:867-874`).
- `_pickFile` matches by filename, not TorBox id, because Torrentio's
  `fileIdx` is not a TorBox file id (`:302-353`, header note at `:4-7`).
- `_requestDl` calls `requestdl` with `redirect=false` (`:774-790`); HLS
  uses `/stream/createstream` + `/stream/getstreamdata` (`:792-820`).
- Polling is deliberately shallow: cached hits skip `/mylist`, uncached
  hits get one lookup, then a second after 800 ms (`:850-909`) because a
  full `mylist` can be multi-megabyte and blow the ~8 s playback budget
  (`:754-755`). This is a concrete data point for the start-to-play exit
  criterion.

### What the references do not settle

None of them parses delivery-link expiry, IP binding or per-link
concurrency limits; TorBox's `ExpiresAt` field is dead data
(`torbox.go:69`). Proxy policy is inconsistent across them: seanime's HLS
proxy honours `ProxyFromEnvironment` (`handlers/proxy.go:86`), while the
TorBox client and directstream CDN client use custom transports with no
`Proxy` field (`torbox.go:104-111`, `httpstream.go:31-39`). Treat link
lifetime, IP binding, Range on the four CDN hosts and the HTTP-proxy question
as open, not answered by the references.

## 9. Sources not used / limits of this research

- No provider account was available, so lifetime, Range, IP binding and
  concurrent-stream claims are reported only where a provider states them.
  Community reports are labelled **unverified** and confined to open
  questions.
- The TorBox API and Developer Terms are served by a JavaScript application;
  the text was read from that page's own `API-fce1409b.js` bundle rather
  than rendered HTML.
- The AllDebrid documentation is a single long page; section references
  above use its own anchors.
- AGPL references (`reiverr`, `bookorbit`) were excluded: behavior only, no
  code read.

## 10. Open questions surfaced

For the grilling tickets on the Phase 5 map:

1. **Real-Debrid cache policy.** Accept an optimistic add and clean up when
   the item was not cached (consumes an active slot, may trigger "too many
   active torrents"), or show `Unknown` and only learn cache status on play,
   or omit cached badges for RD entirely? Which behaviour matches the exit
   criterion without surprising the user?
2. **Uniform cache result type.** Do we model cache status as a three-state
   enum plus optional file list, or keep provider-specific branches behind
   `DebridProvider`? What does the Entry Debrid tab show for `Unknown`?
3. **Credential flow per provider.** Device/pairing flow everywhere (more
   UI, revocable on PM, allowed by RD/AD), or paste-an-API-key as the simple
   first version? Does RD's shared open-source client id `X245A4XAIBGVM`,
   with its stricter limits, become a shipped default?
4. **Delivery-link refresh.** When do we re-resolve a link (before every
   play, on 403/404, on a timer), and can the same stale-URL-then-fallback
   policy serve both debrid links and online-stream URLs?
5. **Range and proxy behaviour.** Do the four download hosts honour
   `Range`, and do links survive Autumn's global HTTP proxy? This needs
   account-based experiments; until then, does the relay fail closed or
   fall back to a direct player URL?
6. **Concurrent-stream and device limits.** No provider publishes a clear
   number. Do we cap debrid streams in Autumn, warn, or leave it to the
   provider's abuse system?
7. **Rate-limit budgets.** RD 250/min plus unverified tighter per-endpoint
   spacing, TB 300/min + 60 uncached/hour, AD 12/s + 600/min, PM no numeric
   limit: does `debrid` use `net`'s shared limiter or its own per-provider
   budget, and how are cache checks counted?
8. **Cleanup and ownership.** How do we record "we added this" so a failed
   or cancelled start never deletes a pre-existing user transfer?
9. **Terms risk.** TorBox API Terms §9 restricts "public applications"
   without written authorisation; RD/AD restrict proxy/server use; PM
   restricts multiple IPs and third-party apps. What does Autumn state in
   its UI, and which proxy combination is the default?
10. **Cache checks and availability badges.** ADR 0011 keeps unattended
    traffic off indexers and out of availability badges. Debrid cache checks
    are per-user, credentialed requests: are they on-demand only, or can
    they back a persisted badge on the Entry Debrid tab?
