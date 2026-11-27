# AniDB episode ids (eid): cheapest reliable source

Research for decision ticket [#206](https://github.com/buildxlabs/autumn/issues/206) on the [Phase 3 wayfinder map](https://github.com/buildxlabs/autumn/issues/176). Checked 2026-11-27 against primary sources. Research only: no product code changed, and no AniDB UDP call was made.

## Answer

**Use `api.ani.zip` as the mapping source, with AnimeTosho results as an optional cross-check.** AniZip already supplies an `anidbEid` on every episode entry in the payloads Autumn fetches today; the field is simply discarded by our `Episode` model. Reading it is an additive, key-free, no-new-service change to `metadata`.

AnimeTosho also publishes `anidb_eid` on releases, but it is release-centric, null on batches and specials, and at least one observed release carried a season/episode-mismatched eid. Per the [#181 decision](https://github.com/buildxlabs/autumn/issues/181), `eid` stays **used when known, never required**, with the `q=` keyword fallback always available — so an incomplete source is acceptable as long as it never blocks search.

AniDB direct is the authoritative source but is terms-expensive for a desktop app: registered client, per-user account, strict throttling, and CC BY-NC-SA content. Torznab accepts `eid` as a _query filter_ but does not return an AniDB attribute, so it cannot supply ids.

## 1. AniZip — recommended primary source

`GET https://api.ani.zip/v1/episodes?anilist_id=<n>` returns a single JSON object with `titles`, `episodes`, `episodeCount`, `specialCount`, `images` and `mappings`. Entries are keyed by the API's key (`"1"`, `"S1"`, `"O1"`); each carries `episode`, **`anidbEid`**, `airDate`, `length`, `rating`, `summary` and TVDB fields where present. `mappings` carries the series-level `anidb_id`, `mal_id`, `kitsu_id`, etc. ([One Piece](https://api.ani.zip/v1/episodes?anilist_id=21), [Frieren](https://api.ani.zip/v1/episodes?anilist_id=154587))

Observed coverage (every entry in each fetched payload had an eid):

| Show (AniList id)                                                                   | State          |                        Entries |  With `anidbEid` |
| ----------------------------------------------------------------------------------- | -------------- | -----------------------------: | ---------------: |
| [One Piece (21)](https://api.ani.zip/v1/episodes?anilist_id=21)                     | airing         | 1,265 (numbered + specials/O1) |    1,265 / 1,265 |
| [Frieren (154587)](https://api.ani.zip/v1/episodes?anilist_id=154587)               | finished       |          59 (28 + 31 specials) |          59 / 59 |
| [Attack on Titan (16498)](https://api.ani.zip/v1/episodes?anilist_id=16498)         | specials/OVAs  |                             40 |          40 / 40 |
| [Steins;Gate (9253)](https://api.ani.zip/v1/episodes?anilist_id=9253)               | OVAs           |                             27 |          27 / 27 |
| [Apothecary Diaries S3 (195516)](https://api.ani.zip/v1/episodes?anilist_id=195516) | current season |               20 (12 numbered) | 12 / 12 numbered |

Ids include specials and OVAs (`S1` → 273428 for Frieren, 151521 for Attack on Titan; `O1` → 248666 for One Piece). Current-season episodes had eids before airing (Apothecary Diaries eps 10–12 had `airDate: null` but eids 318846–318848), so the dataset is refreshed. Responses carried `cache-control: public, max-age=900, s-maxage=900`; a request without a `User-Agent` was refused with 403, one with a UA succeeded.

**Local state:** Autumn already stores these payloads. [`crates/metadata/src/anizip.rs:70-119`](/Users/gaurav/.t3/worktrees/autumn/research-anidb-eid-5058fbdd7c4e03a5/crates/metadata/src/anizip.rs:70) defines `Media.episodes: BTreeMap<String, Episode>`; `Episode` models numbers, titles and dates but has no `anidbEid` field, so serde silently drops it. `Mappings.anidb_id` at [`:170-178`](/Users/gaurav/.t3/worktrees/autumn/research-anidb-eid-5058fbdd7c4e03a5/crates/metadata/src/anizip.rs:170) is the _series_ aid. The checked-in fixtures already contain the field — [anizip_frieren.json:43](/Users/gaurav/.t3/worktrees/autumn/research-anidb-eid-5058fbdd7c4e03a5/crates/metadata/tests/fixtures/anizip_frieren.json:43), [anizip_onepiece.json:31](/Users/gaurav/.t3/worktrees/autumn/research-anidb-eid-5058fbdd7c4e03a5/crates/metadata/tests/fixtures/anizip_onepiece.json:31) — so there is no new HTTP call, crate edge or service dependency in this option. The existing cache TTLs (24 h airing / 7 d finished, [`:30-50`](/Users/gaurav/.t3/worktrees/autumn/research-anidb-eid-5058fbdd7c4e03a5/crates/metadata/src/anizip.rs:30)) govern freshness.

**Caveat:** no published terms of service, rate limit, cadence statement or API docs were found (`/docs` and `/` 404; [robots.txt](https://api.ani.zip/robots.txt) has no usable values). "Cheap and reliable" here is an observed property of the payload, not a contractual guarantee. Keep the existing single-request-per-show cache behaviour and a UA; do not bulk-fetch.

## 2. AnimeTosho — secondary / cross-check only

The live successor to the retired original feed is Anime Tosho NEW at `feed.animetosho.net` ([shutdown notice](https://animetosho.org/about/shutdown), [domain move](https://animetosho.net/news/26), [v1.3.1](https://animetosho.net/news/1035), [v1.4](https://animetosho.net/news/1036)). Its JSON search results include `anidb_aid`, `anidb_eid`, `anidb_fid`, `info_hash`, `magnet_uri`, seeders/leechers and file counts. The machine-readable [`/json/v1/caps`](https://feed.animetosho.net/json/v1/caps) lists `/json/v1/episodes/{eid}`, `/json/v1/episodes`, `/json/v1/series/anidb/{aid}` and release endpoints; no full parameter reference is published beyond the [About page](https://animetosho.net/about) (replace `rss2`/`atom` with `json`).

Measured on the newest 100 release rows on 2026-11-27:

| Query                                                                         | `anidb_eid` non-null | Distinct eids |
| ----------------------------------------------------------------------------- | -------------------: | ------------: |
| [`aid=69` (One Piece, airing)](https://feed.animetosho.net/json?aid=69)       |             96 / 100 |            15 |
| [`aid=17617` (Frieren, finished)](https://feed.animetosho.net/json?aid=17617) |             38 / 100 |            20 |

Nulls are batches/season packs and specials; querying [`eid=271418`](https://feed.animetosho.net/json?eid=271418) returned 5/5 results with that eid. This is **release coverage, not an episode catalogue**. One observed mismatch: an `aid=69` release labelled `S23E06` carried `anidb_eid: 439`, which [AnimeTosho's own episode endpoint](https://feed.animetosho.net/json/v1/episodes/439) and AniZip both identify as the 1999 episode 6, not the current ~1182. Treat release→eid as a heuristic hint, never an authority.

Limits: no API key, `cache-control: public, max-age=30`, JSON default 50 / max 100 ([observed](https://feed.animetosho.net/json?aid=17617&limit=2), `offset` verified), Torznab default 75 / max 200, search queries capped at 200 chars and feed history at offset 7,500 ([v1.6 news](https://animetosho.net/news/1038)). [robots.txt](https://feed.animetosho.net/robots.txt) sets `search=yes, ai-train=no, use=reference`. No numeric request-rate limit is published.

## 3. AniDB direct — not recommended for Phase 3

AniDB returned HTTP 403 (Cloudflare) to direct fetches from this environment, so the points below are **docs-reported from indexed excerpts of AniDB's own pages**, not re-read live; the UDP API was not called.

- Both APIs require a registered client identifier; UDP also needs a user account and API key ([HTTP API](https://wiki.anidb.net/HTTP_API_Definition), [UDP API](https://wiki.anidb.net/UDP_API_Definition)).
- Flood rules: UDP must not exceed 0.5 packets/s (enforced after 5 packets) or one packet per 30 s over an extended period; HTTP asks for no more than one page every two seconds, and warns that repeated same-day fetches or too many requests get you banned ([UDP](https://wiki.anidb.net/Udp), [HTTP API](https://wiki.anidb.net/HTTP_API_Definition)). Failed auth requires backoff from 30 s up to 30 min; sending while banned resets the timer; reusing one UDP port matters.
- Content is CC BY-NC-SA 4.0 and AniDB states **"No scraping"** and offers no database dumps or bulk export ([policy](https://anidb.net/policy)). The wiki [API page](https://wiki.anidb.net/API) documents only a daily anime-titles dump (title→aid, no episode ids); the full TCP API is request-only. The docs explicitly say the UDP API is not appropriate for downloading a local copy of the database.
- Identifiers do exist (`aid`, `eid`, `fid`, `gid`, `lid`; HTTP `<episode id="111977">`, UDP `EPISODES`/`EPISODE`) ([IDs](https://wiki.anidb.net/ID)), so a rare user-initiated lookup is possible later — but per-user library hydration is exactly the access pattern the terms discourage, and it adds account, throttling, backoff and licensing obligations to a key-free path that already works.

## 4. Torznab — query filter, not a source

The [Torznab 1.3 spec](https://torznab.github.io/spec-1.3-draft/torznab/Specification-v1.3.html) defines `season`, `episode`, `tvdbid`, `imdb*`, `tmdbid` and friends; there is **no AniDB id attribute**. Anime Tosho NEW's live [`/api/torznab?t=caps`](https://feed.animetosho.net/api/torznab?t=caps) reports `supportedParams="q,aid,aids,eid,eids,gid,gids,tvdbid,imdbid,tmdbid,malid,myanimelistid,kitsuid"` — you can _find_ releases by eid (`t=tvsearch&aid=17617&ep=1`, `t=search&eid=271418`) — but with `extended=1` the returned `<torznab:attr>` names are `category, size, files, year, tvdbid, season, episode, tvairdate, tmdbid, coverurl, bannerurl, infohash, magneturl, nzburl, seeders, leechers, peers, downloadvolumefactor`. No eid is returned. [Prowlarr's definition](https://raw.githubusercontent.com/Prowlarr/Indexers/master/definitions/v11/animetosho-xyz.yml) uses only `q,season,ep`. So Torznab adds `season`/`episode`/`tvairdate` over JSON but cannot populate an eid mapping.

## Recommendation: an additive `metadata` mapping

1. **Model the field we already receive.** Add `anidb_eid: Option<u32>` to `anizip::Episode` with `#[serde(default, rename = "anidbEid")]`, matching the existing `absolute_number`/`season_number` pattern. Add a fixture assertion so a payload change is caught. This is additive: no `ExternalId` variant, no new crate edge (ARCHITECTURE §3 unchanged), no new service.
2. **Expose a lookup, not a provider.** `metadata` offers an episode-level lookup keyed by AniList media id + the AniZip episode key (`"1"`, `"S1"`, `"O1"`), returning `Option<u32>`. `search` consumes it as an optional hint for `eid=` alongside the existing `aid=` path; absence falls through to `q=` exactly as [#181 Q3](https://github.com/buildxlabs/autumn/issues/181) requires. Nothing is required, nothing is retried for the sake of the eid.
3. **Keep AnimeTosho as cross-check, not a second mapper.** Its `anidb_eid` is already on the results Phase 3 receives; cache it in the existing result/availability cache and use it for matching (`anidb_aid` + `anidb_eid`) or to backfill a missing value. Never overwrite a conflicting AniZip value with it silently; the One Piece mismatch shows why.
4. **No DB migration in Phase 3.** Today `media_external_id` is series-level and `local_file` has no episode-level id ([migrations 0001](https://github.com/buildxlabs/autumn/blob/main/crates/db/migrations/0001_media_ids.sql), [0004](https://github.com/buildxlabs/autumn/blob/main/crates/db/migrations/0004_local_file.sql)); the cached AniZip payload is sufficient for "used when known". Persist a dedicated episode cross-reference only if a later phase needs offline/startup lookup, as a separate additive change.
5. **Do not add an AniDB client now.** Revisit only as a rare, user-initiated, opt-in lookup with a registered client and a persistent cache, after the terms are re-read from AniDB directly.

### Glossary framing

An AniDB **eid** is a **Cross-reference** at _episode_ level — a link saying that an external id names a particular **Episode**. It is **not** a **Media external id**: `ExternalId::AniDb(u32)` already names the series aid, and the glossary's "external id belongs to at most one media" is a series-level rule. The two ids must stay distinct in names and types. Proposed glossary entry (for the coordinator's docs PR, not committed here):

```md
**AniDB eid**:
An AniDB episode id, used as an episode-level **Cross-reference** to an **Episode**.
It is not a **Media external id**: `ExternalId::AniDb` is AniDB's series aid.
_Avoid_: AniDB id, episode number
```

## Confidence and unverified points

- AniZip coverage figures are from five live payloads on 2026-11-27, not a census; no published ToS, rate limit or update cadence exists at a reachable URL.
- AnimeTosho limits/offset were verified empirically; the feed has no full published parameter reference. The mismatched eid is one observed case, not a measured error rate.
- AniDB statements come from indexed excerpts of its docs (direct fetches were 403); confirm them by reading AniDB from a permitted network before any direct integration. No UDP call was made.
- "Currently airing" for One Piece was confirmed with one AniList GraphQL query (`nextAiringEpisode.episode: 1181`).

## Sources

- **AniZip:** [One Piece](https://api.ani.zip/v1/episodes?anilist_id=21), [Frieren](https://api.ani.zip/v1/episodes?anilist_id=154587), [Attack on Titan](https://api.ani.zip/v1/episodes?anilist_id=16498), [Steins;Gate](https://api.ani.zip/v1/episodes?anilist_id=9253), [Apothecary Diaries S3](https://api.ani.zip/v1/episodes?anilist_id=195516), [robots.txt](https://api.ani.zip/robots.txt)
- **AnimeTosho:** [About](https://animetosho.net/about), [migration](https://animetosho.net/news/26), [v1.3.1](https://animetosho.net/news/1035), [v1.4](https://animetosho.net/news/1036), [v1.6](https://animetosho.net/news/1038), [JSON caps](https://feed.animetosho.net/json/v1/caps), [JSON by aid](https://feed.animetosho.net/json?aid=17617), [JSON by eid](https://feed.animetosho.net/json?eid=271418), [episode endpoint](https://feed.animetosho.net/json/v1/episodes/271418), [Torznab caps](https://feed.animetosho.net/api/torznab?t=caps), [robots.txt](https://feed.animetosho.net/robots.txt), [original shutdown](https://animetosho.org/about/shutdown)
- **AniDB:** [IDs](https://wiki.anidb.net/ID), [UDP API](https://wiki.anidb.net/UDP_API_Definition), [HTTP API](https://wiki.anidb.net/HTTP_API_Definition), [flood rules](https://wiki.anidb.net/Udp), [API](https://wiki.anidb.net/API), [policy](https://anidb.net/policy)
- **Torznab:** [spec 1.3](https://torznab.github.io/spec-1.3-draft/torznab/Specification-v1.3.html), [anime-search proposal](https://github.com/Sonarr/Sonarr/issues/2909), [Prowlarr definition](https://raw.githubusercontent.com/Prowlarr/Indexers/master/definitions/v11/animetosho-xyz.yml)
- **Repo:** [anizip.rs:70](/Users/gaurav/.t3/worktrees/autumn/research-anidb-eid-5058fbdd7c4e03a5/crates/metadata/src/anizip.rs:70), [fixtures](/Users/gaurav/.t3/worktrees/autumn/research-anidb-eid-5058fbdd7c4e03a5/crates/metadata/tests/fixtures), [ADR 0001](/Users/gaurav/.t3/worktrees/autumn/research-anidb-eid-5058fbdd7c4e03a5/docs/adr/0001-own-media-id-minted-on-first-user-fact.md), [GLOSSARY.md](/Users/gaurav/.t3/worktrees/autumn/research-anidb-eid-5058fbdd7c4e03a5/GLOSSARY.md)
