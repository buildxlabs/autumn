# AniList API constraints for Phase 1

Answers [issue #10](https://github.com/buildxlabs/autumn/issues/10) for
ROADMAP Phase 1 ("Catalog, tracking, and discovery").

Researched 2026-11-21. Sources:

- AniList docs, read from their source repo
  [`AniList/docs`](https://github.com/AniList/docs) at commit `03281c0`
  (2026-08-18), rendered at <https://docs.anilist.co>. Paths below are
  `docs/guide/...` in that repo.
- Live requests to `https://graphql.anilist.co` on 2026-11-21 (headers,
  introspection, small probes). Marked **(observed)**.
- cynic book in the cynic repo (moved to
  [codeberg.org/obmarg/cynic](https://codeberg.org/obmarg/cynic); the GitHub
  mirror [obmarg/cynic](https://github.com/obmarg/cynic) is archived) and
  crates.io metadata.
- `reference/seanime` (Go, read-only) for real-world behavior. Paths are
  relative to the main checkout root.

Anything not backed by one of these is marked **unverified**.

## 1. Rate limit

**Documented limit.** "The AniList API has a rate limit of 90 requests per
minute", but a banner on the same page says "The API is currently in a
degraded state and is limited to **30 requests per minute**. This is a
temporary measure until the API is fully restored."
(`docs/guide/rate-limiting.md`; the banner is also live on
<https://docs.anilist.co/guide/rate-limiting>.)

**Observed.** Every response on 2026-11-21 carried
`x-ratelimit-limit: 30` **(observed)**. The degraded limit is the current
reality, not a past incident.

**Headers.** Every response has `X-RateLimit-Limit` and
`X-RateLimit-Remaining`. Once over the limit you get "a 1 minute timeout";
responses during the timeout also carry `Retry-After` (seconds to
wait) and `X-RateLimit-Reset` (Unix timestamp) (`docs/guide/rate-limiting.md`).
The CORS header `access-control-expose-headers` lists
`X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset` **(observed)**.

**429 behavior.** HTTP `429 Too Many Requests` with a GraphQL body
`{"data": null, "errors": [{"message": "Too Many Requests.", "status": 429}]}`
(`docs/guide/rate-limiting.md`).

**Burst limiter.** "On top of the above rate limiting, we also have a burst
limiter" to stop "hammering the API with too many requests in a very short
period of time". No numbers are given (`docs/guide/rate-limiting.md`). The
burst threshold is **unverified**.

**Raises.** "We are not currently accepting requests for increased rate
limits" (`docs/guide/rate-limiting.md`).

**Other failure modes** (`docs/guide/considerations.md`):

- In severe outages AniList may lower limits further or suspend the API;
  announcements go to the AniList Discord only.
- A suspended API answers `403` with a GraphQL error message ("The AniList
  API has been temporarily disabled ...").
- AniList may manually block an IP for excessive requests.

**Whether the limit is per IP or per token is not documented.** Unverified.
Because Autumn is a desktop app, each user has their own IP, so the budget
is per user in practice.

**Seanime's handling** (`reference/seanime/internal/api/anilist/client.go`,
`doAniListRequestWithRetries` and `getRetryWindow`): retry when status is 429
**or** `X-Ratelimit-Remaining` is `"0"`; wait until `Retry-After` (+1 s) or
else `X-RateLimit-Reset`, measured from the response `Date` header; one
process-wide blocker so every request waits out the same window; at most 3
retries; a UI event (`AnilistRateLimit`) and a throttled warning toast tell
the user how many seconds remain. This maps directly onto the Phase 1
"rate-limit indicator".

## 2. Authentication

From `docs/guide/auth/index.md` unless noted:

- **OAuth2**, both Authorization Code and Implicit grants.
- **No scopes.** "Scopes are not supported. Access tokens provide (almost)
  full access to a user's data."
- **Lifetime.** "AniList access tokens are long-lived. They will remain valid
  for 1 year from the time they are issued."
- **No refresh.** "Refresh tokens are not currently supported. Once a token
  expires, you will need to re-authenticate your users."
- **Client registration.** At <https://anilist.co/settings/developer>:
  an application name and a redirect URL, which "can be any valid URI,
  including custom URI schemes". You get a client ID and secret.
  Applications cannot be deleted.
- **Which grant.** Implicit for clients that cannot keep a secret ("typically
  websites or mobile apps"); Authorization Code for server apps. A desktop
  binary cannot keep a secret, so Implicit is the documented fit.
- **PIN.** "If you are absolutely unable to use an HTTP or custom URI scheme
  redirect", set the client's redirect URL to
  `https://anilist.co/api/v2/oauth/pin`; after approval the user sees the
  token on a page and pastes it into the app. Works with either grant.
- **One redirect URL per client.** The docs describe a single "Redirect URL"
  field. Choosing PIN therefore rules out a custom-scheme redirect on the
  same client ID (inferred from the single field; **unverified** whether
  multiple URLs are accepted).

**Implicit flow** (`docs/guide/auth/implicit.md`): open
`https://anilist.co/api/v2/oauth/authorize?client_id={client_id}&response_type=token`;
after approval the JWT `access_token` is in the URL **fragment** of the
redirect.

**Using the token** (`docs/guide/auth/authenticated-requests.md`):
`Authorization: Bearer <token>`. Tokens are JWTs; decoding gives "the user's
ID, expiration date, and other information". So Autumn can read `exp` to warn
before the one-year expiry, and `sub` for the user ID, without a request.
(The exact claim names are **unverified**; the docs only say ID and expiry
are present.)

**Seanime** builds exactly the PIN/implicit URL:
`https://anilist.co/api/v2/oauth/authorize?client_id=15168&response_type=token`
(`reference/seanime/seanime-web/src/lib/server/config.ts`) and sends
`Authorization: Bearer` only when a token is set
(`reference/seanime/internal/api/anilist/request_provider.go`,
`PrepareRequest`).

## 3. What works without auth

From `docs/guide/auth/index.md`:

- **No auth:** anime and manga data, character search, data of public and
  unlisted users.
- **Auth required:** modifying user lists; private users' data (only your
  own); user-specific fields on other objects, e.g. `Media.mediaListEntry`.

Confirmed live **(observed)**:

- `Page.airingSchedules(notYetAired: true, sort: TIME)` and
  `Media(sort: TRENDING_DESC)` return data with no token.
- `Viewer` without a token returns HTTP 200 with
  `{"errors":[{"message":"Unauthorized.","status":401}],"data":{"Viewer":null}}`.
  So "not authorized" arrives as a GraphQL error, not an HTTP 401.

`Media.mediaListEntry` is "The authenticated user's media list entry"
(schema description) and "Requesting this field while not authenticated will
return `null`" (`docs/guide/graphql/queries/media-list.md`).

`MediaListCollection` itself only needs a `userId`/`userName` and works for
public users without a token; private lists need the owner's token
(`docs/guide/auth/index.md`). Note: "Even when making authenticated
requests, the user is not inferred. You will need to specify the user"
(`docs/guide/graphql/queries/media-list.md`), so fetch `Viewer { id }`
first.

## 4. Queries and mutations Phase 1 needs

All from a live introspection of `https://graphql.anilist.co` on 2026-11-21
**(observed)**: 197 types, 28 query root fields, 30 mutations, no
subscriptions. Requests must be `POST` to `https://graphql.anilist.co`
(`docs/guide/graphql/index.md`).

**Catalog / search with filters.** `Page(page, perPage) { media(...) }`.
`Page.media` takes ~75 filter args, including `search`, `type`, `format`,
`format_in`, `status`, `season`, `seasonYear`, `genre_in`, `genre_not_in`,
`tag_in`, `tag_not_in`, `isAdult`, `onList`, `averageScore_greater`,
`countryOfOrigin`, `source_in`, `startDate_greater`, `id_in`, `idMal_in` and
`sort: [MediaSort]`. `MediaSort` includes `TRENDING_DESC`, `POPULARITY_DESC`,
`SCORE_DESC`, `START_DATE_DESC`, `UPDATED_AT_DESC`, `SEARCH_MATCH`,
`FAVOURITES_DESC`. `GenreCollection` and `MediaTagCollection` exist for the
filter pickers.

Pagination limits:

- `perPage` is capped at **50**: asking for 100 returned 50 items and
  `pageInfo.perPage: 50` **(observed)**. The docs do not state the cap.
- "The `total` and `lastPage` fields are not currently accurate. You should
  only rely on `hasNextPage`" (`docs/guide/graphql/pagination.md`). Schema
  description of `PageInfo.total` agrees.
- Only one data field per `Page` (plus `pageInfo`)
  (`docs/guide/graphql/pagination.md`). Several lists in one request need
  aliased `Page` fields.

**Trending.** `Media.trending` ("related activity in the past hour") and sort
`TRENDING_DESC`. A `MediaTrend` root query exists too.

**Airing schedule.** `Page.airingSchedules(...)` with `notYetAired`,
`airingAt_greater`, `airingAt_lesser`, `mediaId_in`, `episode_greater`,
`sort: [AiringSort]`. Per-media: `Media.nextAiringEpisode` and
`Media.airingSchedule(notYetAired, page, perPage)`. Seanime's
`ListRecentAnime` and `AnimeAiringSchedule` queries use these
(`reference/seanime/internal/api/anilist/queries/anime.graphql`).

**`MediaListCollection`.** Args include `userId`, `userName`, `type`,
`status_in`, `forceSingleCompletedList`, `chunk`, `perChunk`, `sort`. Schema
description: "User ID and Media Type arguments required" (`userName` also
works per the docs). Docs (`docs/guide/graphql/queries/media-list.md`):

- Returns lists pre-grouped by status and custom lists.
- "Do not skip over the user's custom lists": entries hidden from status lists
  appear only under custom lists.
- Capped at "the 11,000 most recently updated unique entries".

**`SaveMediaListEntry`** ("Create or update a media list entry"): args `id`,
`mediaId`, `status`, `score`, `scoreRaw`, `progress`, `progressVolumes`,
`repeat`, `priority`, `private`, `notes`, `hiddenFromStatusLists`,
`customLists`, `advancedScores`, `startedAt`, `completedAt`
(`FuzzyDateInput`). Returns `MediaList`. Passing `mediaId` alone creates or
updates (upsert), as seanime does
(`reference/seanime/internal/api/anilist/queries/entry.graphql`).

**`DeleteMediaListEntry(id: Int)`** returns `Deleted { deleted }`. It takes
the **list entry** id, not the media id, so Autumn must keep the
`MediaList.id` returned by the collection or by `SaveMediaListEntry`.

**Viewer avatar.** `Viewer { avatar { large medium } }`; `UserAvatar` has
only `large` and `medium` (both `String`). **There is no animated/GIF
field.** Seanime's "animated GIF upgrade" is a client-side probe
(`reference/seanime/seanime-web/src/app/(main)/_features/navigation/main-sidebar.tsx`,
`getAnilistGifAvatarCandidate`): if the URL is on `*.anilist.co`, under
`/file/anilistcdn/user/avatar/`, and not already `.gif`, swap `/medium/` for
`/large/` and the extension for `.gif`, try to load it, and fall back to the
static image on error. That CDN path convention is undocumented by AniList
(**unverified**, works by observation in seanime).

## 5. cynic 3.14 and the AniList schema

**Version.** `cynic` 3.14.0 was released 2026-07-12, MSRV 1.85; `cynic-cli`
and `cynic-introspection` are also at 3.14.0 (crates.io API). `cynic`
features: `http-reqwest`, `http-reqwest-blocking`, `http-surf`, `rkyv`,
`directives`. `cynic-codegen` features: `rkyv`, `rustfmt`.

**Fetching the schema.** `cynic introspect <URL> -o <file>` from `cynic-cli`
writes SDL; `-H` adds headers. The book recommends checking the schema into
the repo and refreshing it with a periodic CI job
(`cynic-book/src/schemas/introspection.md`). AniList allows anonymous
introspection: the standard introspection query returned the full schema
with no token **(observed)**.

Tried on 2026-11-21 with `cynic-cli` 3.14.0 **(observed)**:
`cynic introspect https://graphql.anilist.co -o anilist.graphql` printed
"Server supports the June 2018 specification" and wrote 166 KB of SDL
(8,885 lines, 184 type definitions).

A throwaway crate (`cynic = "=3.14.0"`, `cynic-codegen` with `rkyv`)
registered that SDL in `build.rs` and compiled QueryFragments for
`Viewer { id name avatar { large medium } }`,
`SaveMediaListEntry(mediaId, progress, status) { id }` and
`DeleteMediaListEntry(id) { deleted }`, emitting the expected GraphQL
**(observed)**. One gotcha: AniList's root fields are PascalCase (`Viewer`,
`Page`, `SaveMediaListEntry`), and cynic's derive maps a snake_case Rust field
to camelCase, so it rejected `viewer` with "no field `viewer` on the GraphQL
type `Query`. Did you mean `Viewer`?". Each root field needs
`#[cynic(rename = "Viewer")]` (or `cynic querygen` output, which adds it).

**Codegen.** Register the SDL in `build.rs` with
`cynic_codegen::register_schema("anilist").from_sdl_file(...)?.as_default()`,
then declare `#[cynic::schema("anilist")] mod schema {}`
(`cynic-book/src/schemas.md`). Query structs come from the derives, or are
generated from `.graphql` documents by `cynic querygen`.

**Size.** AniList is small for cynic: 197 types (including built-ins), a
445 KB introspection JSON and 166 KB of SDL **(observed)**. cynic's own test fixture `schemas/github.graphql` is
1.1 MB of SDL with ~1,500 type definitions
([github.com/obmarg/cynic/schemas/github.graphql](https://github.com/obmarg/cynic/blob/main/schemas/github.graphql)).
For large schemas the book advises the `rkyv` feature on `cynic_codegen` and
a separate crate for the schema module, registering the schema in every
crate that uses the derives (`cynic-book/src/large-apis.md`). At AniList's
size these are optional. In the throwaway crate, rebuilding only the crate
(dependencies cached, `rkyv` on) took about 1 s **(observed)**; the cost with
the dozens of fragments Phase 1 needs, and without `rkyv`, is **unverified**.
A schema module inside `metadata` is likely fine.

**Errors.** AniList reports rate limits, auth failures and outages as GraphQL
`errors` entries with a `status` field (sections 1 and 3). The client must
read `errors[].status` even on HTTP 200, and must check HTTP status before
decoding (429 and 403 bodies have `data: null`).

## 6. Terms of use and local caching

From `docs/guide/terms-of-use.md`:

1. Free for non-commercial use; commercial use is free under $150/month
   revenue.
2. "Using the AniList API as a backup or data storage service is strictly
   prohibited."
3. "**Hoarding** or mass collection of data from the AniList API is strictly
   prohibited."
4. Naming: an app with "AniList" in its name must say "UNOFFICIAL" or "for
   AniList". Autumn's name is unaffected.
5. "Use of the AniList API within competing, non-complementary services of
   the same nature is prohibited. This includes, but is not limited to, anime
   and manga list or tracker services. The restriction applies to all data
   provided through the API, including both user data and media data."

The terms say nothing about cache lifetimes. What they imply for Autumn:

- **Caching what the user browses is fine** (it is what any client does).
  **Pre-fetching or crawling the catalog to build an offline mirror is
  "hoarding"** (rule 3). Cache on demand, with a TTL, not by crawling.
- **The account-free "Local" tracker is the open question under rule 5.**
  Autumn's tracker is a local list, not a service, and AniList is one of its
  backends, so it arguably complements AniList. But rule 5 names "list or
  tracker services" and covers media data, and a Local list backed by AniList
  metadata could be read as competing. Whether a local, single-user,
  non-hosted list counts as a "service" is **unverified**; asking
  contact@anilist.co would settle it.
- **Adult content** (`docs/guide/considerations.md`): filter with
  `isAdult`; "Ecchi" is not adult on AniList. Matters only if Autumn ships to
  a store.

## Implications for Phase 1

- **Budget 30 req/min, not 90.** The `net` limiter should read
  `X-RateLimit-Limit` and `X-RateLimit-Remaining` from each response rather
  than hard-code a number, and keep a margin for the undocumented burst
  limiter.
- **One shared AniList gate.** Like seanime: block every AniList call until
  `Retry-After`/`X-RateLimit-Reset` after a 429 or `Remaining: 0`, retry a few
  times, and feed the wait time to the rate-limit indicator.
- **Design screens for few requests.** Home's widgets should share one
  request through aliased `Page` fields (one data field per `Page`), and the
  poster grid pages 50 at a time on `hasNextPage`. A 30-call minute disappears
  fast on Discover plus Advanced Search.
- **Login = PIN + token paste, one client ID.** Register an Autumn client
  with redirect `https://anilist.co/api/v2/oauth/pin`; open
  `.../oauth/authorize?client_id=...&response_type=token`; the user pastes
  the token. No secret, no refresh. Store the token in the keychain, decode
  `exp` from the JWT, and prompt to re-login before the year runs out. A
  custom-scheme redirect would need a second client.
- **Anonymous mode covers the no-account exit criterion.** Browse, search,
  trending and schedules all work without a token.
- **Track list-entry ids.** `DeleteMediaListEntry` needs `MediaList.id`;
  include custom lists when reading `MediaListCollection`; query
  `Viewer { id }` first.
- **Animated avatar is a client probe**, not an API field: a HEAD/GET of the
  rewritten `.gif` URL through the poster cache.
- **cynic 3.14 fits.** Check in the SDL from `cynic introspect`, register it
  in `build.rs`, rename PascalCase root fields in derives; add a split crate
  only if compile time hurts.
- **Cache on demand, never crawl,** and get an answer on rule 5 for the Local
  tracker before Phase 1 ships.
