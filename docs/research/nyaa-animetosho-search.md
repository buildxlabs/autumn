# Nyaa and AnimeTosho search APIs: endpoints, fields and limits

Answers [issue #178](https://github.com/buildxlabs/autumn/issues/178), a child of the
[Phase 3 wayfinder map #176](https://github.com/buildxlabs/autumn/issues/176), for
ROADMAP Phase 3 and the future `search` crate (ARCHITECTURE §2: "Torrent indexers
(Nyaa, AnimeTosho, extension-provided), result analyzer, batch detection,
auto-select scoring profiles, availability checks").

Researched 2026-11-25. Sources, in trust order:

- Live, deliberately light GETs against `nyaa.si`, `animetosho.org`,
  `animetosho.net` and `feed.animetosho.net` on 2026-11-25. Marked **(observed)**.
- First-party documentation: Nyaa's help page <https://nyaa.si/help>; Anime Tosho's
  About/FAQs and shutdown notices; the Torznab `t=caps` endpoint; and the
  first-party source repos `animetosho/animetosho-website` and
  `manami-project/anime-offline-database`.
- This repo: `crates/metadata/src/{anime_offline_database,anizip}.rs`,
  `crates/db/src/media.rs`, ARCHITECTURE.md and ROADMAP.md.
- Read-only references in the main checkout (the `reference/` tree is
  git-excluded, so these paths are relative to the main checkout root):
  `reference/nyaa/src/**` and `reference/seanime/internal/**`.

Anything not backed by one of these is marked **unverified**. "Documented" means
the operator's own docs or source; "observed" means a live response on the date
above. Both services can change without notice — treat every "observed" fact as
a snapshot.

## 1. Nyaa (`nyaa.si`)

### Endpoints, query parameters and sorting

Two search surfaces exist.

1. **HTML listing.** `GET https://nyaa.si/?q=<query>&c=<major>_<minor>&f=<filter>&p=<page>&s=<sort>&o=<asc|desc>&u=<user>`.
   This is the URL shape the reference client builds
   (`reference/nyaa/src/source/nyaa_html.rs:306-309`), with `q` URL-encoded,
   `c` split into major/minor by integer division/modulo 10
   (`nyaa_html.rs:303`), and `s` mapped as Date→`id`, Downloads→`downloads`,
   Seeders→`seeders`, Leechers→`leechers`, Size→`size`
   (`nyaa_html.rs:161-186`). Nyaa's help page confirms results can be filtered
   by category, remake, trusted and users, and sorted by size, date, seeders,
   leechers, completed count and comment count (<https://nyaa.si/help>).
   Filter values used by the reference client: `0` no filter, `1` no remakes,
   `2` trusted only, `3` batches (`nyaa_html.rs:188-199`).
   **(Observed)** a live HTML query `?q=frieren&c=1_2&f=0&p=1&s=seeders&o=desc`
   returned a listing sorted by seeders, with torrent, magnet, size, date and
   seed/leech/download columns.

2. **RSS.** `GET https://nyaa.si/?page=rss&f=<filter>&c=<major>_<minor>&q=<query>&u=<user>&m`
   (`reference/nyaa/src/source/nyaa_rss.rs:61-66`). The trailing `m` flag is
   sent by the reference client; its meaning is **unverified**. Nyaa's own feed
   URLs also carry `limit=N` (e.g. `?c=0_0&f=0&limit=10&page=rss&q=...`), but
   the effect of `limit` on the RSS result count is **unverified**.
   **(Observed)** `GET /?page=rss&q=frieren&c=1_2` returned
   `Content-Type: application/xml`, 200 OK.

Nyaa's help page also documents search operators: `|` for OR (`foo|bar`), `-`
to exclude, `"quoted phrases"` for substrings, and parentheses for precedence
(<https://nyaa.si/help>).

### Fields per result

**RSS (observed on 2026-11-25).** Each `<item>` has `<title>`, `<link>` (a
`https://nyaa.si/download/<id>.torrent` URL), `<guid>` (a
`https://nyaa.si/view/<id>` URL), `<pubDate>` (RFC 2822), and a `nyaa:`
extension namespace (`xmlns:nyaa="https://nyaa.si/xmlns/nyaa"`) with:

| Element | Example |
| --- | --- |
| `nyaa:seeders`, `nyaa:leechers`, `nyaa:downloads` | `36`, `3`, `1728` |
| `nyaa:infoHash` | `ef3e7ad1b12bdd9fc341691d8866cd1fa8374a4b` (40-hex v1) |
| `nyaa:categoryId`, `nyaa:category` | `1_2`, `Anime - English-translated` |
| `nyaa:size` | `6.6 GiB` |
| `nyaa:comments` | `0` |
| `nyaa:trusted`, `nyaa:remake` | `No` / `No` (the strings `"Yes"`/`"No"`) |

Caveat: `reference/nyaa/src/source/nyaa_rss.rs:82-147` reads seeders,
leechers, downloads, categoryId, size, trusted and remake, but ignores
`nyaa:infoHash`, and stores `item.link()` in a field named `magnet_link` even
though the RSS `<link>` is the `.torrent` URL. An Autumn implementation should
read the hash from `nyaa:infoHash` and not repeat that mislabel.

**HTML (observed).** Each `table.torrent-list > tbody > tr` has, in column
order: category icon/link (`?c=1_2`), title (with `title` attribute), `.torrent`
link, magnet link (`magnet:?xt=urn:btih:...`), size, date (`YYYY-MM-DD HH:MM`),
seeders, leechers, downloads. Trusted/remake are conveyed by CSS row classes:
the reference client treats `success` (green) as trusted and `danger` (red) as
remake (`nyaa_html.rs:407-413`); Nyaa's help page also describes orange batches
and grey hidden torrents (<https://nyaa.si/help>). The infohash is available
from the magnet's `xt=urn:btih:` parameter.

### Paging

HTML paginates at **75 results per page**: the reference client derives
`last_page = (total_results + 74) / 75` from the `.pagination-page-info` text
(`nyaa_html.rs:335-348`), and `p=<page>` selects the page. RSS returns only the
first page; the reference client sets `last_page = 1` and sorts locally instead
of asking Nyaa (`nyaa_rss.rs:56, 31-43`). The reference client's own docs say
exactly this: "nyaa.si's implementation of RSS only will return the first page
of results ... you cannot move to the next page, and will have to refine your
search" and "it also does not allow for sorting" (`reference/nyaa/docs/sources/nyaa.md:40-45`).

### Rate limits and blocking behaviour

- No numeric request-rate limit is published on Nyaa's help page, and none is
  returned in response headers (**unverified**).
- **(Observed)** the first RSS GET returned `server: ddos-guard`, three
  `__ddg*` cookies, `cache-control: max-age=300` (and also a
  `no-cache, no-store, must-revalidate` header), and `x-robots-tag: noarchive`.
- **(Observed)** subsequent requests from the same host — including `/help`,
  `/faq`, `/robots.txt` and an HTML search — failed with `curl: (35) Recv
  failure: Connection reset by peer` (TCP reset), while a small number of
  further fetches through a different network path succeeded. This is
  consistent with DDoS-Guard IP-level throttling/blocking, not with a documented
  API limit. Practical consequence: keep concurrency low, cache responses,
  back off on connection resets, and design for a configurable mirror/proxy.

### Mirrors and how the reference client handles a mirror base URL

- The reference client's default is `base_url = "https://nyaa.si/"`
  (`nyaa_html.rs:145-159`); Sukebei is `https://sukebei.nyaa.si/`
  (`reference/nyaa/src/source/sukebei_nyaa.rs:78-93`).
- README advises: if `nyaa.si` is blocked, replace `base_url` with a working
  proxy; it recommends `nyaa.land` and links to
  <https://nyaatorrents.info/#proxy> (`reference/nyaa/README.md:110-125`).
- Third-party lists name `nyaa.iss.ink`, `nyaa.land`, `nyaa.ink`, `nyaa.mom`,
  `nyaa.media` (e.g. Jackett's `nyaasi.yml`). There is **no operator-published
  official mirror list** that this research could find; treat all mirrors as
  unverified and possibly malicious/stale.
- Mechanism (`reference/nyaa/src/source/nyaa_html.rs:300-309,398-405`,
  `nyaa_rss.rs:59-66,102-105`, `reference/nyaa/src/util/conv.rs:58-73`):
  `add_protocol(base_url, true)` parses a full `http`/`https`/`socks5` URL as
  given, or prefixes `https://` for a bare host; all further URLs are derived
  with `Url::join` relative to that base (`/download/<id>.torrent`, post links),
  so a mirrored response yields mirrored links. A separate global
  `request_proxy` sets a reqwest proxy (`source.rs:131-146`).
  `(base_url, timeout, rss)` are per-source config knobs
  (`nyaa_html.rs:106-118`).

## 2. AnimeTosho

### Availability: the original feed/API is gone; a successor is live

This is the most important finding for planning.

- The original Anime Tosho announced permanent shutdown: ingestion of new
  torrents halted around **2026-05-09**, updates ceased around 2026-05-10, and
  "this website (including feed/API) will continue to be operational for at
  least several months after May" (<https://animetosho.org/about/shutdown>,
  <https://animetosho.org/about/shutdown2>).
- **(Observed 2026-11-25)** `feed.animetosho.org` does not resolve (`Could not
  resolve host`), and `https://animetosho.org/json?...` and
  `https://animetosho.org/api?...` both return HTTP 404. The original JSON
  feed/API is therefore **not usable** from Autumn as of this date.
- A successor, **Anime Tosho NEW** (`https://animetosho.net/`), is live. Its
  About page (last updated 2026-10-17) says it "grew from the original project
  and its source code" but has been extensively rewritten — a different
  service, not the original operator (<https://animetosho.net/about>).
  `https://animetosho.xyz` 308-redirects to `.net`; feed paths on `.net`
  return 410 telling clients to use `https://feed.animetosho.net`
  **(observed)**.
- The live JSON feed is `https://feed.animetosho.net/json` **(observed, 200
  with `application/json`)**. This is the endpoint any new integration should
  target, while treating its long-term availability as a risk.

### Endpoints and parameters

First-party docs (<https://animetosho.net/about>, "Is there a feed available?"
and "Is there an API available?"):

- RSS2: `https://feed.animetosho.net/rss2` (with `?only_tor=1` torrents-only,
  `?magnet_only=1` magnets-only), ATOM: `/atom`, Newznab/Torznab: `/api`.
- The JSON API is the feed with `rss2`/`atom` replaced by `json`:
  `https://feed.animetosho.net/json`. A single torrent/file can be fetched
  with `?show=torrent&id=<id>` (or `btih`, `nyaa_id`, `sukebei_id`, `tosho_id`,
  `anidex_id`, `nekobt_id`; `show=file&id=`), documented in first-party source
  `pages/feed.php:196-323` of `github.com/animetosho/animetosho-website`.

**(Observed)** `GET https://feed.animetosho.net/api?t=caps` returns:

```
<limits max="200" default="75"/>
<search available="yes" supportedParams="q,aid,aids,eid,eids,gid,gids,tvdbid,imdbid,tmdbid,malid,myanimelistid,kitsuid" searchEngine="raw"/>
<tv-search ... supportedParams="...,season,ep" searchEngine="raw"/>
<categories><category id="5070" name="Anime"/><category id="2020" name="Movies"/></categories>
<url type="newznab" value="https://feed.animetosho.net/api/newznab"/>
<url type="torznab" value="https://feed.animetosho.net/api/torznab"/>
```

So `max=200`/`default=75` are **page sizes**, not a request rate.

Search parameters (first-party source `pages/includes/listing.php:5-19,29-38,153-168` of
`animetosho/animetosho-website`, plus the `caps` endpoint):

| Parameter | Meaning |
| --- | --- |
| `q` | free-text search |
| `aid` | one AniDB anime id (`anidb_aid`) |
| `eid` | one AniDB episode id (`anidb_eid`) |
| `aids`, `eids`, `gids` | comma-separated id lists; capped at 20 ids; `eids` takes precedence over `aids` |
| `offset`, `limit` | pagination (default limit 75, max 200 per caps) |
| `gid`, `tvdbid`, `imdbid`, `tmdbid`, `malid`/`myanimelistid`, `kitsuid` | other external ids (Torznab `supportedParams`) |
| `season`, `ep` | additional TV-search params |

Free-text operators (About/FAQs): partial-word matching, `*` at word start/end
(minimum 4 characters), `-word` exclusion, `"quoted phrase"`.

**(Observed)** `t=search&q=frieren&limit=2` returned 2 results;
`aid=17617` returned torrents with `anidb_aid: 17617` (Frieren);
`eid=271418` returned a torrent with `anidb_eid: 271418` (Frieren ep. 1);
`aids=17617,18886` returned both anime; `offset=2&limit=2` returned a later
slice. This confirms AniDB anime-id and episode-id search and free-text search
all work on the live successor feed.

### Fields per result

**(Observed)** JSON search items carry: `id`, `title`, `link`, `timestamp`
(Unix seconds), `status`, `tosho_id`, `nyaa_id`, `nyaa_subdom`, `anidex_id`,
`nekobt_id`, `info_hash` (40-hex v1), `info_hash_v2`, `magnet_uri`,
`torrent_url`, `torrent_name`, `seeders`, `leechers`,
`torrent_downloaded_count`, `tracker_updated`, `nzb_url`, `total_size` (bytes),
`num_files`, `anidb_aid`, `anidb_eid`, `anidb_fid`, `article_url`,
`article_title`, `website_url`, `is_seadex`. This matches the first-party field
list in `pages/feed.php:474-515`.

**File list:** search results give only `num_files`. The full file list comes
from `?show=torrent&id=<id>`: **(observed)** top-level keys include `files`,
`anidb_gid`, `is_batch`, `is_dupe`, `source`, `source_id`, `tvdbid`, `imdbid`,
`tmdbid`, `primary_file_id`; each `files[]` entry has `id`, `filename`, `name`,
`size`, `crc32`, `md5`, `sha1`, `sha256`, `ed2k`, `bt2`, `processed`,
`is_archive`, `info.mediainfo`, `screenshots`, `links`, `links_audio`,
`attachments` (subtitle/font attachments with URLs) and `vidframe_timestamps`.
For one 28-file torrent this response was ~260 KB — fetch it only for a
shortlisted result, not for every search hit.

### Rate limits and terms

- No published numeric **request** rate limit was found in the About/FAQs, in
  `t=caps`, or in response headers (**unverified**). Do not assume a quota; be
  conservative.
- The Newznab/Torznab endpoints are public; **no account or API key is
  required**, and any key value a client sends is ignored
  (<https://animetosho.net/about>). The original AT likewise asked clients to
  use key `0` (<https://animetosho.org/about>).
- **(Observed)** feed responses carried `access-control-allow-origin: *`,
  `access-control-allow-methods: GET, OPTIONS`, `cache-control: public,
  max-age=30`, `server: cloudflare`; no `x-ratelimit-*` or `Retry-After` headers
  on a normal 200.
- Both `animetosho.net/robots.txt` and `feed.animetosho.net/robots.txt`
  (observed) declare `Content-Signal: search=yes, ai-train=no, use=reference`
  and disallow several named AI-crawler user agents. Autumn is a normal client
  fetching a public API, not a crawler; still, keep request volume low and
  respect `Cache-Control`/`ETag`.
- Practical limits observed/documented: `limit` max 200, default 75; id lists
  capped at 20; 30-second cache lifetime; the site's own skipping policy drops
  most torrents over 25 GB and duplicates/reposts (About/FAQs), so coverage is
  not identical to Nyaa.

## 3. Where Autumn gets AniDB ids

- **anime-offline-database (anime-level ids: yes).** The upstream project
  aggregates cross-provider ids and documents `sources` as URLs such as
  `https://anidb.net/anime/4563`; its week-27-2026 README reports **14,517**
  AniDB entries out of 41,537, under ODbL-1.0 + DbCL-1.0
  (<https://github.com/manami-project/anime-offline-database/blob/master/README.md>).
  The dataset has **no episode ids** — only anime-level cross-references.
- **`metadata` already carries them.** `crates/metadata/src/anime_offline_database.rs`
  loads `anime-offline-database.jsonl` from the latest GitHub release, refreshes
  at most weekly, and exposes `OfflineAnime.ids: AnimeOfflineIds` with
  `anilist`, `mal`, `anidb`, `kitsu` (lines 42-79, 154-217). `AnimeOfflineIds::parse_sources`
  extracts `anidb` from `anidb.net/anime/<N>` source URLs (lines 262-286); the
  fixture test asserts `anidb: Some(10_143)` (line 610). So for a media Autumn
  has matched to the offline database, **an AniDB anime id is already
  available** through `entries()` / `OfflineAnime.ids.anidb`. As of this
  checkout no other crate consumes that field yet (grep across `crates/`).
- **AniZip (also anime-level, and episode-level in the raw API).**
  `crates/metadata/src/anizip.rs` fetches
  `https://api.ani.zip/v1/episodes?anilist_id=<N>` (lines 239-269) and exposes
  `Mappings.anidb_id` (lines 168-194). The recorded API fixtures contain
  `anidbEid` per episode (`crates/metadata/tests/fixtures/anizip_frieren.json:43,70,...`,
  `anizip_onepiece.json:31,39,...`), but the Rust `Episode` struct
  (`anizip.rs:91-125`) has **no field for it** — AniDB episode ids are dropped
  on deserialization today. Conclusion: Autumn has anime ids from two sources,
  but **AniDB episode ids are not currently available** to `search` without a
  small `metadata` change (out of scope for this research ticket).
- The db layer already models AniDB as an external-id provider
  (`crates/db/src/media.rs:13-14`), so no schema concept is missing.

## 4. How Seanime builds its Nyaa/AnimeTosho queries (behaviour)

All paths below are under `reference/seanime` in the main checkout.

- `internal/torrents/torrent/search.go:192-249`: before searching it fetches
  AniZip "Animap" metadata by AniList id, takes `Mappings.AnidbId` into
  `anidbAID`, and resolves the episode's `AnidbEid` into `anidbEID` via
  `FindEpisode`. Both are passed to each provider's `SmartSearch`.
- `internal/extension/hibike/torrent/types.go:97-120`: the provider contract
  exposes `AnimeSmartSearchOptions` with `media`, `query`, `batch`,
  `episodeNumber`, `resolution`, `anidbAID`, `anidbEID`, `bestReleases`.
  `AnimeTorrent.Confirmed` documents that a torrent found via AniDB anime or
  episode id is marked confirmed (lines 164-168) — i.e. an id hit is trusted
  more than a title hit.
- `internal/torrents/autoselect/search.go:303-337`: auto-select uses **smart
  search** when the provider declares `CanSmartSearch`; otherwise it falls back
  to **simple search** with `util.CleanMediaTitle(romajiTitle)` as the query.
  Same file at lines 168-189, 340-357: it tries profile resolutions in turn,
  searches up to 3 providers concurrently, falls back batch→single, and treats
  a batch search as good when max seeders ≥ 15 or more than 2 results were
  found.
- `internal/torrents/torrent/search.go:306-360`: results are deduplicated by
  infohash, best releases put first, then sorted by seeders; the same sort is
  applied to previews (lines 522-549). Misses are cached 1 minute, releasing
  media 5 minutes, otherwise GC time (lines 503-520).
- **Boundary / unverified from local sources:** the HTTP query strings for the
  Nyaa and AnimeTosho *providers* are not in this checkout — they live in
  Seanime's external JavaScript provider extensions (the local
  `internal/extension/hibike/torrent` only defines the interface and the
  `anime-torrent-provider.d.ts` shape). Only the orchestration above is
  verifiable here. What Seanime passes down is clear: AniDB anime id, AniDB
  episode id, cleaned romaji title, episode number, batch flag, resolution.

## 5. Implications for Phase 3 `search`

- Nyaa exposes **no AniDB ids**: match by parsed release title and AniList/AniDB
  synonyms; take the hash from RSS `nyaa:infoHash` or the HTML magnet. Prefer
  HTML for paging and sort; RSS is a one-page, locally-sorted fallback.
- AnimeTosho exposes **direct AniDB anime/episode search** (`aid`, `eid`), which
  is the more precise route *if* the successor feed is deemed dependable; fall
  back to `q` free text. Fetch `show=torrent&id=` only for shortlisted results.
- Autumn already has AniDB **anime** ids; it does not yet retain AniDB
  **episode** ids. That is a small `metadata` follow-up, not a blocker for
  title-based search.
- Neither service publishes a numeric request-rate limit; design for caching,
  low concurrency and graceful degradation (DDoS-Guard resets on Nyaa, sunset
  risk on AnimeTosho). Keep a configurable Nyaa base URL and proxy, and do not
  hard-code the retired `feed.animetosho.org` host.
