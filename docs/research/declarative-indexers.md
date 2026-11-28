# Declarative indexer definitions: what they express, and what a torrent provider needs

Research for [#263](https://github.com/buildxlabs/autumn/issues/263). Researched
2026-11-28 against primary sources only: the Jackett definition-format wiki and
schema, the Servarr/Prowlarr Cardigann wiki and the `Prowlarr/Indexers`
repository, real definition files from both projects, and Autumn's own
`crates/search` and `crates/release-parser` source.

- `https://github.com/Jackett/Jackett/wiki/…` and
  `https://wiki.servarr.com/en/prowlarr/…` are the format documentation; the
  Servarr page states it is synced from the Jackett wiki (last sync 2025-08-16)
  and adds Prowlarr-specific notes.
- Local paths (`crates/…`, `ARCHITECTURE.md`) are in this repo at the commit
  this research branch is based on.

## Verdict

Cardigann-style YAML is a **declarative scraper descriptor**: it says which URL
to request, how to build the request from a search query and user settings, what
shape the response has, which rows and fields to extract, and which string
filters to apply. Much of a definition is boilerplate for two things Autumn
already has: (a) adapting to the Newznab/Torznab category and ID model
of the \*arr apps, and (b) rewriting release titles into `SxxEyy` form for
Sonarr/Radarr compatibility.

For a Phase 4 **public anime torrent provider**, the part worth expressing is a
small core: identity and site links, a request template, a response kind
(HTML/JSON/XML), row and field selectors with a short filter set, a per-site
request delay, and error/no-results detection. Login flows (`post`/`get`/`form`/
`cookie`), CAPTCHA and CSRF inputs, Newznab category mappings, ratio economy
fields, poster/genre/cross-site IDs and all the title-normalization filters are
not needed. In particular, the long `title_phase1 … title_live_action` filter
chain in the real Nyaa definition duplicates work Autumn's `release-parser`
already does from the raw release name.

A declarative source should emit the **raw** title and let `release-parser` and
`search::matching` interpret it.

## Licences and porting

Autumn is GPL-3.0-only (`AGENTS.md`). Licences were checked against each
project's `LICENSE` file and GitHub's detected SPDX identifier.

| Project | Licence | Evidence | Consequence for Autumn |
| --- | --- | --- | --- |
| Cardigann (`cardigann/cardigann`) | MIT | [`LICENSE`](https://github.com/cardigann/cardigann/blob/master/LICENSE) | Format ancestor. MIT code could be ported with its notice kept, but nothing needs porting. |
| Jackett | GPL-2.0 (LICENSE text is GPL v2; GitHub SPDX `GPL-2.0`) | [`LICENSE`](https://github.com/Jackett/Jackett/blob/master/LICENSE) | GPL-2.0-only code is **not** automatically compatible with a GPL-3.0-only project. Definitions and docs are behaviour reference only; do not copy them in. |
| Prowlarr | GPL-3.0 | [`LICENSE`](https://github.com/Prowlarr/Prowlarr/blob/master/LICENSE) | Licence-compatible, but the definitions live in a separate repo and carry their own authorship. Reference only. |
| Prowlarr definitions (`Prowlarr/Indexers`) | no separate `LICENSE` at repo root; definitions are maintained by the Prowlarr team | [`README.md`](https://github.com/Prowlarr/Indexers/blob/master/README.md) | Do not vendor YAML files; design Autumn's own format for its own manifest and host API (D10). |

**Conclusion: behaviour-only.** No code or definition text should be ported.
Autumn's declarative source format is an original design targeting our own
manifest (`ARCHITECTURE.md` §6) and host API, not a Cardigann-compatible loader.

## What a Cardigann definition expresses

Documentation: [Jackett wiki, *Definition format*](https://github.com/Jackett/Jackett/wiki/Definition-format)
and [Servarr wiki, *Cardigann YML Definition*](https://wiki.servarr.com/en/prowlarr/cardigann-yml-definition).
Syntax is checked against [Jackett's `schema.json`](https://github.com/Jackett/Jackett/blob/master/src/Jackett.Common/Definitions/schema.json)
or Prowlarr's per-version `schema.json`. Two-space indentation is significant
and a mis-indented clause may be silently ignored.

### 1. Header and capabilities

Required: `id`, `name`, `description`, `language`, `type` (`public`,
`semi-private`, `private`), `encoding`, `links` (first is the default mirror),
`caps`, `search`. Optional: `replaces`, `legacylinks`, `followredirect`,
`testlinktorrent`, `requestDelay` (seconds between requests), `certificates`.

`caps` holds `categorymappings` (tracker category ID → Newznab category, with
`desc` and `default`) or a simple `categories` map, plus `modes`: `search: [q]`
is mandatory; `tv-search`, `movie-search`, `music-search`, `book-search` list
the parameters the site supports (`season`, `ep`, `imdbid`, `tvdbid`, `tmdbid`,
`tvmazeid`, `traktid`, `doubanid`, `year`, `genre`, `album`, `artist`, …).
`allowrawsearch` and `allowtvsearchimdb` are extra opt-ins.

### 2. Settings

A `settings:` list declares the config form: `name` (available as
`{{ .Config.name }}`), `type` (`text`, `password`, `checkbox`, `select`,
`multi-select`, `info`, and `info_*` help boxes), `label`, `default`, `options`,
`defaults`. A public tracker with no settings uses `settings: []`. This is the
closest analogue to Autumn's per-extension settings rows.

### 3. Login — private-trackers only

`login.method` is `post`, `get`, `form` (fetch the form first: handles CSRF and
simple CAPTCHAs), or `cookie`. It carries `path`, `inputs`, `captcha`,
`selectorinputs`/`getselectorinputs` (values read from the page with selectors),
`error` selectors, and a `test` path/selector to check the session. Jackett and
Prowlarr differ here: Prowlarr does not request the `test` path, checks each
search response for a redirect/error/failed selector and re-logins, and does
not auto-handle Google reCAPTCHA. None of this applies to Nyaa or AnimeTosho.

### 4. Search — the actual declarative core

`search` has:

- `paths` (or single `path`): URL templates, `method: get|post`, `headers`,
  per-path `inputs`, `categories` (select the path by category, `!` negates),
  `inheritinputs`, `followredirect`, and `response: {type: json|xml,
  noResultsMessage: …}`. HTML is the default when `response` is absent.
- `inputs`: query parameters; `$raw` is appended without escaping. Empty-valued
  inputs are dropped unless `allowEmptyInputs: true`.
- `keywordsfilters`: filters applied to the search keywords before templating;
  result is `.Keywords`.
- `error`: selectors that mark a search response as failed.
- `preprocessingfilters`: applied to the raw HTML/XML body before row parsing.
- `rows`: `selector`, `attribute`, `multiple`, `count`,
  `missingAttributeEqualsNoResults`, `dateheaders`, `after` (merge following
  rows), `remove`, and row `filters` (e.g. `andmatch`, `strdump`).
- `fields`: one `SelectorBlock` per output field, with `selector` (CSS for HTML;
  JSONPath-like with `$` root and `..` row prefix for JSON; the same CSS-like
  syntax for XML), `attribute`, `text` (fixed/templated value), `optional`,
  `default`, `case`, `remove`, and an ordered list of `filters`.

The schema requires `title`, `size`, `seeders`, one of `category`/`categorydesc`,
and one of `download`/`magnet`/`infohash`. For public and semi-private sites,
Cardigann can synthesize a magnet URI from an infohash and the result title
plus a set of public trackers; for private sites it will not, because those
trackers must be preserved.

Selectors support `:has()`, `:not()` and `:contains()`. Despite the question's
wording, there is **no first-class XPath** in the format: XML definitions use
the same CSS-style `selector` (e.g. `rss > channel > item` in AnimeTosho's
definition). Regex enters through filters (`regexp`, `re_replace`,
`querystring`, `dateparse`/`fuzzytime`, `split`, `trim`, `replace`,
`urlencode`/`urldecode`, `htmldecode`, `prepend`/`append`, `validate`,
`jsonjoinarray`, `diacritics`), not through the selector itself.

### 5. Template language

A small Go-template-like language, not a scripting language:

- `{{ .Variable }}`, `{{ re_replace .Keywords "re" "replacement" }}`
- `{{ if … }}…{{ else }}…{{ end }}`, `and`/`or`, `eq`/`ne` (string comparison),
  nesting; `.True`/`.False` sentinels
- `{{ join .Categories "," }}`, `{{ range .Categories }}…{{ end }}` and
  indexed `range $i, $e := .Categories …`
- namespaces: `.Config.*` (from settings), `.Query.*` and `.Categories` during
  search (`.Query.Season`, `.Query.Ep`, `.Query.IMDBID`, `.Query.Type`,
  `.Query.Keywords`, `.Query.Limit`, `.Query.Offset`, …), `.Result.*` for
  earlier fields in the same row, `.DownloadUri.*` in the download block.

Definitions routinely use `.Result.<working_field>` chains to build a final
value; names starting with `_` are working fields.

### 6. Paging

There is **no `paging` block** in either schema. Paging is expressed through
multiple `paths` with different inputs plus the `.Query.Limit` / `.Query.Offset`
template variables. The real [Nyaa definition](https://github.com/Jackett/Jackett/blob/master/src/Jackett.Common/Definitions/nyaasi.yml)
adds a second path that requests `p=2` for keyword-less searches; the
[AnimeTosho definition](https://github.com/Jackett/Jackett/blob/master/src/Jackett.Common/Definitions/animetosho-xyz.yml)
simply sets `limit: 100`. Autumn already has its own request budget
(`MAX_REQUESTS_PER_INDEXER = 8` in `crates/search/src/query.rs`), so a
definition should not need to own paging.

### 7. Download block

Needed only when the `.torrent` URL is not in the search result, the request
must be POST, or an intermediate page is needed. Supports `method`, `before`
(with `path`/`pathselector`, `inputs`), `selectors` (fallback chain),
`infohash` (hash + title scraped from the download page), and `headers`. For
Autumn, a `Torrent` can be built from `magnet` plus `infohash` alone; the
download block is out of scope unless a source exposes no direct link.

## What the anime sources actually need

| Capability | Nyaa-like HTML / RSS | AnimeTosho (JSON or Torznab XML) | Needed by a Phase 4 source? |
| --- | --- | --- | --- |
| Site identity, mirrors, per-site delay | Yes (`links`, `legacylinks`, `requestDelay: 2`) | Yes (`api-url` setting) | **Yes** — mirrors and a delay belong in a definition. |
| Request template + query variables | `q`, `c`, `f`, `s`, `o` inputs | `t`, `q`, `season`, `ep`, `limit` | **Yes** — the core of a declarative provider. |
| Keyword rewriting | `re_replace` for years, `S1`, ordinal seasons | same two filters | Optional. Autumn's `clean_title` / `search::plan` already build queries. |
| Response shape | HTML rows `tr.default,tr.danger,tr.success`; RSS is the site's own XML feed, not Cardigann | `response.type: xml` (Torznab), `rss > channel > item` | **Yes** — HTML and structured JSON/XML. |
| Row + field selectors | CSS `td:nth-child(n)`, attributes, `case` for row classes | `selector` on XML elements / `[name=…]` Torznab attrs | **Yes**. |
| String filters | `split`, `regexp`, `append`, `dateparse` | `dateparse` | **Yes**, but a short set: query-string, regex, date, trim, url-decode. |
| Magnet from infohash | Both `magnet` and `.torrent` present | `infohash` attr; `magnet` can be derived | **Yes**, and Autumn already derives a magnet in `search::util::magnet_for`. |
| Login / cookies / CAPTCHA / CSRF | No | No | **No** for public anime trackers. Belongs with private-tracker support. |
| Newznab category mapping, `modes` | Nyaa maps 30+ categories to Newznab `TV/Anime`, `Movies/Other`, … | single `Anime` category | **No** — Autumn has no Newznab category model; a simple filter/category selector is enough. |
| `downloadvolumefactor` / `uploadvolumefactor`, `minimumratio`, `minimumseedtime` | Hard-coded `0`/`1` | hard-coded `0`/`1` | **No** — private-tracker ratio economy. |
| imdb/tmdb/tvdb/trakt/douban IDs, poster, genre, description | optional fields | `tmdbid`, `tvdbid` attrs | **No** — `Torrent` has no slots for these; AniDB IDs come from AnimeTosho's own fields. |
| Release-title normalization | ~15 chained `re_replace` rules (`title_phase1` → `title_live_action`) to insert `S01`, convert `12x45` to `S12E45`, strip years for Sonarr/Radarr | same chain | **No** — `release-parser` does this from the raw name; rewriting the name destroys information. |

Note the format difference for AnimeTosho: Cardigann reads the **Torznab XML**
endpoint (`feed.animetosho.net/api/torznab`) with Torznab attribute names,
while Autumn already reads the **JSON feed** (`feed.animetosho.net/json`, ADR
0008) at `crates/search/src/animetosho.rs`. A definition for Autumn should
name the JSON shape, not copy the XML attribute selectors.

## Mapping onto `search::Torrent`

`crates/search/src/result.rs` is the target. `Indexer` is currently a closed
enum; for an extension-provided source the indexer label will have to come from
the extension's identity, not from the definition (the map already treats this
as an open question — no decision is made here).

| Cardigann / source field | `Torrent` field | Notes |
| --- | --- | --- |
| — | `indexer` | Extension identity; not a definition field. |
| `infohash` (`download`/`magnet` alternatives) | `infohash` | **Required** in Autumn: lowercase v1 hash, dedupe key. v2-only hashes are kept by AnimeTosho's parser. |
| `title` | `name` | Keep the raw site title. Do **not** apply Sonarr/Radarr normalization filters. |
| `download` | `torrent_url` | Optional; resolved against the configured mirror. |
| `magnet` / synthesized magnet | `magnet_url` | Auto-derived from `infohash` when absent (`search::util::magnet_for`). |
| `size` | `size` | Cardigann auto-parses units; Autumn's `util::parse_size` does this for Nyaa HTML. |
| `seeders` | `seeders` | Optional. |
| `leechers` | `leechers` | Optional. |
| `grabs` | `downloads` | Cardigann name is `grabs`. |
| `date` | `published` | Unix seconds; `dateparse`/`fuzzytime` and timezone suffixes. |
| `files` | `num_files` | Count only; the `files` vector needs the separate AnimeTosho detail request and is not a search field. |
| site row class (`success`/`danger`) | `trusted` / `remake` | Nyaa-specific; express as a `case` selector if a source has equivalents. |
| AnimeTosho detail flag | `is_batch`, `is_seadex` | `is_seadex` is present in the JSON feed; `is_batch` comes from details. |
| source AniDB fields | `anidb_aid`, `anidb_eid` | No Cardigann equivalent; AnimeTosho's JSON has both. |
| `category`, `categorydesc`, `poster`, `genre`, `description`, `year`, `downloadvolumefactor`, `minimumratio`, cross-site IDs | — | No home in `Torrent`. Drop them. |

Required by `Torrent::new`: `indexer`, `infohash`, `name`. Everything else is
`None` or empty when the source does not report it.

## What `release-parser` already covers

`crates/release-parser/src/lib.rs` parses an anime release name into
`Release` (pure, never fails): `title`, `group`, `season`/`last_season`,
`episode`/`last_episode`, `volume`, `batch`, `version`, `episode_title`,
`resolution`, `extension`, `checksum`, `year`, `kind` (Episode/Special/Movie/
NCOP/NCED/Extra), `video_codec`, `bit_depth`, `audio`, `source`, `streaming`,
`subtitles`. `search::matching` and `search::query::episode_season` already call
it on titles.

So a definition must **not** reimplement: `S01` insertion, `12x45` → `S12E45`,
year stripping, batch detection, episode ranges, NCOP/NCED/OVA/Movie markers,
resolution or codec tags. Those filters are the largest single block in the
real Nyaa definition and exist to satisfy external \*arr apps, not to identify
an episode for Autumn.

A definition still must supply everything `release-parser` cannot know from a
name: the infohash and URLs, size, swarm counts, download count, publish time,
file count, AniDB IDs, and the site-specific flags.

## Versioning and testing

**Prowlarr** keeps definitions in a separate repo,
[`Prowlarr/Indexers`](https://github.com/Prowlarr/Indexers), in version folders
`v1` … `v12` with a `schema.json` in each. [`VERSIONS`](https://github.com/Prowlarr/Indexers/blob/master/VERSIONS)
declares `CURRENT_VERSION=11`, `MAX_VERSION=12`, `MIN_VERSION=11`,
`NEXT_VERSION=13`. A new schema version needs changes to Prowlarr's Cardigann
C# code; Prowlarr falls back to an older version when no YML exists for the
current one. The README documents `scripts/validate.py` (whole set, one
directory, single file, best-version detection) and legacy `ajv-cli-servarr`
validation. Definitions are synced from Jackett daily by
`scripts/indexer-sync-v2.sh` with a configurable blocklist.

**Jackett** keeps a flat `src/Jackett.Common/Definitions/` directory with one
root `schema.json`, validated with `ajv test … -c ajv-formats --spec=draft2019`.
Its unit tests use recorded HTML fixtures:
[`CardigannIndexerHtmlTests.cs`](https://github.com/Jackett/Jackett/blob/master/src/Jackett.Test/Common/Indexers/CardigannIndexerHtmlTests.cs)
registers `html-response1.html` through `TestWebClient.RegisterRequestCallback`
and asserts the parsed release fields. The wiki's
[*Yearly updates for test code*](https://github.com/Jackett/Jackett/wiki/Development:-Yearly-updates-for-test-code)
page exists because relative date filters make a fixture's expected publish
year drift each year. Prowlarr has the analogous
`ApplyGoTemplateTextFixture.cs` for the template engine.

**Live testing** is manual in both projects: Prowlarr's
[*Indexers* wiki page](https://wiki.servarr.com/en/prowlarr/indexers) says to
put a custom YML in `<AppData>/Definitions/Custom` (restarting to clear the
cache) and use the indexer form's **Test** button — a green checkmark means the
live site, login and selectors worked. Neither project's CI runs definitions
against live sites. Autumn's `ARCHITECTURE.md` §7 already fixes the equivalent
rule: HTTP integrations test against recorded fixtures, never live services in
CI. A definition test harness should therefore feed captured HTML/JSON/XML
through the field extractor and assert a `Torrent`, with a live "Test source"
action in the Extensions screen as the manual equivalent.

## Source list

- [Jackett — Definition format](https://github.com/Jackett/Jackett/wiki/Definition-format)
- [Jackett — Cardigann schema](https://github.com/Jackett/Jackett/blob/master/src/Jackett.Common/Definitions/schema.json)
- [Jackett — Nyaa definition](https://github.com/Jackett/Jackett/blob/master/src/Jackett.Common/Definitions/nyaasi.yml)
- [Jackett — AnimeTosho definition](https://github.com/Jackett/Jackett/blob/master/src/Jackett.Common/Definitions/animetosho-xyz.yml)
- [Jackett — Cardigann HTML fixture test](https://github.com/Jackett/Jackett/blob/master/src/Jackett.Test/Common/Indexers/CardigannIndexerHtmlTests.cs)
- [Servarr wiki — Cardigann YML Definition](https://wiki.servarr.com/en/prowlarr/cardigann-yml-definition)
- [Servarr wiki — Indexers](https://wiki.servarr.com/en/prowlarr/indexers)
- [Prowlarr/Indexers — README](https://github.com/Prowlarr/Indexers/blob/master/README.md) and [`VERSIONS`](https://github.com/Prowlarr/Indexers/blob/master/VERSIONS)
- [Prowlarr/Indexers — v11 Nyaa definition](https://github.com/Prowlarr/Indexers/blob/master/definitions/v11/nyaasi.yml)
- [Prowlarr — licence](https://github.com/Prowlarr/Prowlarr/blob/master/LICENSE), [Jackett — licence](https://github.com/Jackett/Jackett/blob/master/LICENSE), [Cardigann — licence](https://github.com/cardigann/cardigann/blob/master/LICENSE)
- In-repo: `crates/search/src/result.rs`, `crates/search/src/nyaa.rs`, `crates/search/src/animetosho.rs`, `crates/search/src/query.rs`, `crates/release-parser/src/lib.rs`, `ARCHITECTURE.md` §6–§7, `ROADMAP.md` §3 Phase 4
