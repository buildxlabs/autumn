# Autumn

A native desktop app for anime: a local library, streaming, online sources from extensions and list tracking. Manga is not planned for now.

## Language

### Media and identity

**Media**:
One anime or manga as a catalog lists it, at the granularity of a single season or cour. Two seasons of the same story are two media joined by a relation.
_Avoid_: show, title, series, entry, franchise

**Media identity**:
Autumn's own identifier for a media. It exists once the user has recorded something about that media, and it never changes while the media lives.
_Avoid_: AniList id (as a synonym)

**External id**:
The identifier a provider (AniList, MyAnimeList, AniDB, Kitsu, or a custom source) gives a media. All providers count equally, and a media can have one external id per provider.
_Avoid_: source id, remote id

**Cross-reference**:
A link saying that an external id names a particular media. An external id belongs to at most one media.
_Avoid_: mapping, alias

**AniDB eid**:
An AniDB episode id, used as an episode-level **Cross-reference** to an **Episode**. It is not a **Media external id**: `ExternalId::AniDb` is AniDB's series aid.
_Avoid_: AniDB id, episode number

**Custom source**:
A provider of media that are not in the AniList catalog, supplied by an extension.

### Library

**Local file**:
A video on disk that Autumn has found in a library folder and can play directly.
_Avoid_: media file, video file, entry

**Episode**:
A numbered installment of a media's main run, distinct from its specials and creditless clips.
_Avoid_: file, local file

**Special / NC**:
Video attached to a media but outside its episode numbering: an OVA, special, movie, NCOP or NCED.
_Avoid_: extra, episode

**Locked file**:
A local file whose match a re-scan never changes.
_Avoid_: pinned file, frozen file

**Ignored file**:
A local file a scan deliberately leaves out of the library.
_Avoid_: hidden file, excluded file

**Shelved file**:
A local file on a disconnected drive, kept with its match until the drive returns.
_Avoid_: missing file, offline file

**Missing episode**:
An aired regular **Episode** with no **Local file** in the library; counted on the Entry Library tab's missing banner. A **Shelved file** on a disconnected drive counts as owned, not missing.
_Avoid_: missing file, gap

**Scan summary**:
The outcome of one scan: files found, files matched and files left unmatched.
_Avoid_: report, scan log

### Torrents

**Torrent**:
A torrent identified by its infohash, whether found on an indexer or brought in by the user and tracked in Autumn's own session this run. An indexer result carries a **Release** name, links, size, seeders and an origin indexer; an in-session torrent adds a **Torrent state**, an origin and selected files.
_Avoid_: magnet, link, download, hit

**Indexer**:
A source of torrent search results: Nyaa, AnimeTosho, or an enabled **Torrent provider**. Results name the indexer they came from.
_Avoid_: provider (a provider supplies external ids), tracker, site

**Release**:
A fansub or scene name for one **Episode** or a **Batch**, carrying optional group, resolution, codec, audio, source, season, episode number and version; parsed by `release-parser` into a `Release`.
_Avoid_: torrent name, filename, rip

**Batch**:
A **Torrent**/**Release** containing multiple **Episodes**: an explicit range, or flagged complete; matching and file pick treat it as covering an episode it contains.
_Avoid_: pack, complete-series torrent, season pack

**Incoming torrent**:
A torrent the user brings in by paste, `/magnet`, `/droptorrent` or drop, rather than one found by search.
_Avoid_: manual torrent, pasted torrent

**Magnet link**:
A torrent identifier carrying a v1 infohash and an optional display name, but no file list until metadata arrives.
_Avoid_: magnet, link

**Match review**:
The step that attaches an incoming torrent to a Media and an Episode before it can stream.
_Avoid_: auto-match, mapping

**Torrent cache**:
A disposable staging area for streamed torrent files, separate from the library.
_Avoid_: download dir, temp folder

**Disk read switch**:
The point at which a completed stream is served from its file on disk instead of through the torrent stream.
_Avoid_: disk handoff, completion switch

**Stream drop**:
The event that ends a streamed torrent and releases its partial files.
_Avoid_: stop, cancel

**Torrent stream**:
Playing one file of a torrent while it is still downloading, delivered to the player through the relay.

**Readiness gate**:
The condition that must hold before the player is launched on a torrent stream: its head window and tail window are downloaded.

**Head window / Tail window**:
The first and last byte ranges of a streamed file that must be downloaded before playback starts.

**Stream token**:
The random secret in a relay URL; valid for one playback session and revoked when it ends.

**Buffering**:
A torrent stream waiting on pieces, either before launch (_Preparing_) or during playback.

**Torrent List**:
The screen listing every **Torrent** currently in Autumn's own embedded torrent session this run, with live state and stats.
_Avoid_: downloads page, transfer list, queue, active torrents page

**Torrent state**:
The per-torrent lifecycle label in the Torrent List: `preparing`, `downloading`, `streaming`, `paused`, `seeding`; `paused` overrides, `seeding` means complete, no `complete`/`error` state.
_Avoid_: status, phase

**Preload**:
Fetching the next Episode's torrent stream while the current Episode plays, so auto-next can start it without waiting.
_Avoid_: prefetch, pre-download, prepared stream

**Yield**:
A preload pausing while the current torrent stream is buffering.
_Avoid_: throttle, deprioritize

**Auto-select**:
Choosing the best **Torrent** and the correct in-torrent file for an **Episode** per an **Auto-select profile**, verifying the episode exists before playback; never silently relaxing.
_Avoid_: smart search (for the matching stage), auto-pick

**Auto-select profile**:
A named, user-editable set of filters + ordered ranking criteria used by **auto-select**; one shipped `Default`; global default + optional per-**Media** override; per-episode override in the manual search sheet.
_Avoid_: scoring profile, selection rules, autoselect settings

**Release list**:
The per-episode table of search results where the user picks a torrent and a file by hand.
_Avoid_: manual search sheet, search results

**Batch suggestion**:
A batch covering every episode of a finished media, offered on the Torrent tab.
_Avoid_: season pack

**Availability badge**:
A pill on an episode saying whether a matching release exists on the built-in indexers right now.
_Avoid_: torrent status

**Buffer overlay**:
The preparing state of a torrent stream, shown in the playback bar until the player launches.
_Avoid_: loading screen

### Network

**Bound interface**:
A network interface that all torrent traffic is pinned to. If it is unavailable, the torrent engine does not start rather than using another route.
_Avoid_: VPN tunnel, bind device, kill switch

**Indexer mirror**:
A user-supplied base URL that serves one indexer's releases under the same query shape as its default host.
_Avoid_: base URL, host, domain

**Outbound proxy**:
A relay Autumn sends traffic through when configured. Torrent peer traffic and other app requests are configured separately.
_Avoid_: VPN, relay host

### Extensions

**Extension**:
An installable content-provider package, either a Luau script or a **Declarative source**, run under its **Extension manifest** and the **Host API**.
_Avoid_: plugin (reserved for future UI-hook extensions), addon, scraper

**Extension manifest**:
The JSON descriptor defining an **Extension**'s identity (`id`), SemVer `version`, `api_version`, type, runtime, declared permissions and settings schema.
_Avoid_: package.json, config

**Torrent provider**:
An **Extension** of type `torrent-provider`; once enabled and configured it searches as one more **Indexer**.
_Avoid_: torrent extension, scraper, indexer extension

**Provider contract**:
What a **Torrent provider** implements: a `search(request)` function returning release rows (a name and a v1 infohash, plus optional size, seeders, date and file details). Luau scripts and **Declarative sources** both satisfy it, and it is versioned by the manifest's API version.
_Avoid_: provider interface, provider API, SDK

**Declarative source**:
A code-free **Extension** defined in JSON inside its **Extension manifest** (one request template, HTML/XML/JSON row and field selectors, and a fixed filter set), executed natively by Autumn under the same **Provider contract** and network limits as Luau extensions.
_Avoid_: config extension, scraper definition

**Host API**:
What Autumn lets a Luau extension call, under the `autumn` global: HTTP, HTML and JSON parsing, encoding, hashing, time, logging and the extension's own settings. It is versioned by the manifest's API version.
_Avoid_: SDK, bindings, runtime API

**Extension repository**:
A user-added HTTPS JSON index listing installable **Extensions** and their metadata.
_Avoid_: plugin store, source list, repo feed

**Origin URL**:
The **Extension repository** index URL, or direct manifest URL, an **Extension** was installed from; automatic update checks query only this origin.
_Avoid_: source repo, upstream

**Marketplace**:
The Extensions tab that lists installable extensions from the shown extension repositories.
_Avoid_: store, catalog (Catalog is the AniList media catalog)

**Update review**:
The sheet that shows an extension update's permission, settings and code changes before the user accepts or skips it.
_Avoid_: update dialog, changelog

**Extension health**:
The runtime state of an installed extension: `healthy`, `failing` (recording the last non-cancelled error), or `auto-disabled` after repeated CPU or memory kills.
_Avoid_: extension status, crash state

**VM kill**:
Aborting a running extension call through Luau's one-shot interrupt, a wall-clock timeout or an out-of-memory error, cancelling any in-flight host future and rebuilding the VM before the next call.
_Avoid_: thread kill, panic, force stop

**Outbound guard**:
The network policy `net` enforces on extension HTTP traffic: it blocks non-globally-reachable addresses, `localhost`/`.local` names, DNS rebinding and undeclared hosts, on the first request and every redirect hop.
_Avoid_: firewall, sandbox filter

**Declared host**:
A domain or subdomain pattern listed in an extension's manifest that the extension may contact; adding or widening one on update is a permission change.
_Avoid_: allowed domain, whitelist

**LAN host exception**:
A user-approved private or `.local` host permitted for one extension in its settings, overriding the **Outbound guard** only for that extension and exact host.
_Avoid_: SSRF bypass, local whitelist

**Extension playground**:
An in-app tab on the Extensions screen for editing and running a scratch copy of an **Extension** against the real **Host API**, limits and **Outbound guard** without affecting installed extensions.
_Avoid_: extension IDE, sandbox tester

### Tracking

**List entry**:
A user's tracking record for one media on one platform: status, progress, score and the like.
_Avoid_: entry (on its own), list item

**Platform**:
Where a user's list entries live: the account-free local platform, or a tracking service account such as AniList or MyAnimeList.
_Avoid_: tracker, provider (a provider supplies external ids), service, backend

**Active platform**:
The one platform whose list entries are shown, edited and updated by playback right now. The other platforms' entries are dormant: kept, but untouched.
_Avoid_: current account, primary platform

**Local platform**:
The account-free platform. The user's list entries live only in Autumn.
_Avoid_: offline mode, guest account

**List transfer**:
A one-shot, user-reviewed copy of list entries from one platform to another. It creates or updates entries and never deletes.
_Avoid_: sync, merge, mirror, import, upload

**Custom list**:
A user-named list on a platform that a list entry can belong to besides its status. An entry can be in several.
_Avoid_: collection, folder, category

### Accounts

**AniList token**:
The credential that lets Autumn act on a user's AniList account.
_Avoid_: API key, session, password
