# Player remote-stream capabilities (research for #283)

**Verdict.** All four players can open progressive HTTP; HLS and DASH come from
their bundled demuxers rather than from a player-specific API: mpv and IINA
inherit FFmpeg's `hls`/`dash` demuxers [S1][S2], VLC has an explicit "Unified
adaptive streaming for DASH/HLS" demux [S10], and MPC-HC/MPC-BE delegate adaptive
formats to their source filters [S12][S13]. Only mpv/IINA let Autumn inject
arbitrary headers, User-Agent, Referer and a Netscape cookie file directly, both
on launch and over their shared mpv JSON IPC [S1][S2][S4][S6][S7]. VLC covers UA,
Referer and cookie forwarding but exposes no arbitrary-header or cookie-file
option in its own source [S8][S10]. MPC-HC and MPC-BE have no supported way for
Autumn to set request headers or cookies: MPC-HC's UA/Referer plumbing is
internal to youtube-dl integration, and its web interface opens a bare URL
[S12]. A relay is therefore unavoidable for header/cookie injection on VLC and
MPC-HC/BE, for rewriting HLS/DASH manifests whose segment URLs embed expiring
tokens, and — for _every_ player — for refreshing an expired URL without the
player reopening the file and losing its position.

Researched 2026-11-28 from player manuals/wikis and source clones listed under
Sources. Anything not confirmed there is marked **unverified**.

## Autumn's current player seam

`crates/player/src/mpv.rs` launches mpv with `--force-window=immediate`,
`--keep-open=yes`, `--input-ipc-server=<endpoint>`, one `--sub-file=<path>` per
subtitle and an optional `--network-timeout=<secs>`, then the media URL;
`iina.rs` passes the same ideas through iina-cli as `--mpv-input-ipc-server`,
`--mpv-keep-open`, `--mpv-network-timeout`. `Session::load_file` sends
`["loadfile", url, "replace"]`; the IPC pump in `ipc.rs` observes only
`time-pos`, `duration`, `pause`, `eof-reached`, `path`, `chapter` and
`chapter-list`. `PlayerEvent` in `event.rs` has MediaReady/Position/Duration/
Paused/Eof/Exited/Chapters — **no stream-error event**, and it deliberately
ignores `end-file` reasons other than `eof`. `LaunchOptions` has
`subtitles: Vec<PathBuf>` (local paths only) and `network_timeout`, but no
header/cookie fields. VLC is launched with its HTTP interface and driven by
polling `/requests/status.json`; MPC-HC attaches to `/variables.html`,
`/command.html` and `/browser.html`; both translate mpv-shaped commands into web
requests and infer failures from status polls or a lost interface, never from
an error event (`http.rs`). ADR 0005 (repo) already records why torrent streams
go through a token-gated loopback relay: `loadfile` reopens/seeks mid-episode
and VLC/MPC-HC can only relaunch.

## Per-player table

| Player | HTTP progressive              | HLS                                                     | DASH                                    | Range / seek over HTTP                                                                  | Headers on CLI                                                                                                             | Headers over IPC                                                                    | Cookies                                                                 | External subs with headers                                                                             | In-process error signal                                                         | Relay needed?                                   |
| ------ | ----------------------------- | ------------------------------------------------------- | --------------------------------------- | --------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------- | ----------------------------------------------- |
| mpv    | yes [S1]                      | yes, FFmpeg demuxer + `--hls-bitrate` [S1][S2]          | yes, FFmpeg demuxer [S2]                | Range requests; non-seekable if server refuses Range, `--force-seekable` overrides [S1] | `--user-agent`, `--referrer`, `--http-header-fields`, `--cookies-file` (+`--cookies=yes`) [S1][S2]                         | `set_property` before load; `loadfile url replace -1 {…}` per-file options [S1][S3] | Netscape-format file [S1][S2]                                           | yes: `--sub-file=URL` / `sub-add`; global headers apply [S2]                                           | `end-file reason=error` + `file_error`; `eof` possible for broken networks [S1] | only for manifest rewrite / transparent refresh |
| IINA   | yes (mpv 0.41 core) [S4][S5]  | yes (mpv core) [S4][S5]                                 | yes (mpv core) [S4][S5]                 | mpv semantics [S1]                                                                      | `--mpv-user-agent`, `--mpv-referrer`, `--mpv-http-header-fields`, `--mpv-cookies-file` [S6][S7]                            | same mpv JSON IPC as mpv [S3][S6]                                                   | inherited from mpv [S7]                                                 | inherited from mpv [S2][S4]                                                                            | mpv events over its IPC [S3][S4]                                                | same as mpv                                     |
| VLC    | yes [S8]                      | yes, adaptive demux [S10][S11]                          | yes, adaptive demux [S10][S11]          | `Range: bytes=N-`, `Accept-Ranges` detection [S10]                                      | `:http-referrer`, `:http-user-agent`, `--http-forward-cookies`, `--http-reconnect`; **no arbitrary headers** [S8][S9][S10] | only web interface: `in_play` with `option` params [S10]                            | redirect forwarding via internal jar; no Netscape-file option [S8][S10] | `--sub-file` is path-only; `--input-slave`/`add_subtitle_mrl` accept MRLs but no per-sub headers [S10] | none; `/requests/status.json` has only `state` [S10]                            | yes for headers; yes for refresh                |
| MPC-HC | yes via LAV HTTP source [S12] | LAV `hls` format enabled [S12]                          | LAV `dash` format enabled [S12]         | delegated to LAV source; **unverified** in MPC-HC's own source [S12]                    | **none exposed**; UA/Referer only from youtube-dl metadata [S12]                                                           | web interface `?path=` opens a bare URL [S12]                                       | none exposed [S12]                                                      | no documented remote-URL + headers command [S12]                                                       | none; `/variables.html` has state/position/duration/file [S12]                  | yes for headers, manifest rewrite and refresh   |
| MPC-BE | yes [S13]                     | yes (changelog: HLS M3U8 passed to source filter) [S13] | yes (changelog: MP4-DASH support) [S13] | unverified                                                                              | none found in own source [S13]                                                                                             | no equivalent control interface found in source [S13]                               | none found [S13]                                                        | unverified                                                                                             | none documented [S13]                                                           | yes                                             |

## mpv

- **Formats and seeking.** http/https URLs are documented protocol prefixes
  [S1]. FFmpeg's HLS/DASH demuxers are first-class: mpv's format-hack table marks
  `hls` and `dash` as `nested_network`, and `--hls-bitrate` picks a variant
  [S1][S2]. A non-seekable HTTP server disables seeking unless
  `--force-seekable=yes`; in the libcurl backend a seekable transfer can be
  split into bounded Range requests with `--curl-max-request-size` [S1].
- **Headers/cookies.** `--user-agent`, `--referrer` and
  `--http-header-fields='Field1: value1','Field2: value2'` are documented; the
  last is a comma-separated _string-list_ of `Field: Value` pairs [S1]. In
  source they live in the `mp_network_opts` config group, are applied by both
  the libcurl backend (`build_header_list`) and the FFmpeg backend (`headers`
  AVDict), and `--cookies-file` (Netscape format) is only read when
  `--cookies=yes` is set [S2]. Default UA in source is `libmpv` [S2].
  `--network-timeout` is seconds (default 60; 0 keeps FFmpeg defaults) and
  affects at least HTTP [S1][S2].
- **Do headers reach segments and subtitles?** Yes, when set globally: HLS/DASH
  demuxers get the same AVOptions because of the `nested_network` flag
  [S2 `demux/demux_lavf.c:1513`], and external subtitle URLs go through
  `mp_add_external_file` → `demux_open_url` → `stream_create`, which reads the
  same network config group [S2 `player/loadfile.c:1006`]. Note that mpv reads
  these options from the _global_ config group at stream-open time, so set them
  on the command line or with IPC `set_property` **before** the fetch; treating
  them as file-local `loadfile` options is **unverified** [S2].
- **IPC.** JSON IPC supports `set_property <name> <value>` [S3] and
  `loadfile <url> [<flags> [<index> [<options>]]]`, where the fourth argument is
  a map of per-file option values; since mpv 0.38 the third argument must be
  `-1` to use it [S1]. `sub-add <url> [<flags> [<title> [<lang>]]]` loads a
  remote subtitle and routes through the same external-file path [S1][S2].

## IINA

- IINA's own wiki: "IINA is based on mpv. Hence, you can use most of mpv's
  options as well as mpv's config file" [S4]. Version 1.5.0 bundles mpv 0.41.0
  and FFmpeg 9.0.1, so its HLS/DASH and HTTP behaviour is mpv's [S5]. The wiki
  lists reserved window/OSD/terminal options; network options are not among
  them [S4].
- `iina-cli --help` (source) documents `--mpv-*` forwarding of all mpv options
  (negated as `--mpv-foo=no`), plus raw mpv options after `--` [S6]. IINA's
  source explicitly names `user-agent`, `referrer`, `http-header-fields`,
  `cookies`, `network-timeout` and `cookies-file` as mpv options it knows about
  [S7]. Autumn can therefore add `--mpv-http-header-fields=…`,
  `--mpv-referrer=…`, `--mpv-user-agent=…`, `--mpv-cookies-file=…` and
  `--mpv-sub-file=…` to its existing launch arguments.
- `--mpv-input-ipc-server` gives the same JSON IPC endpoint as mpv, so
  `set_property`, `loadfile` and `sub-add` behave as described above; mpv's own
  IPC docs define their events [S3][S6]. IINA maps mpv's `keep-open` to a
  preference, which Autumn already sets [S4].

## VLC

- HTTP is a documented access module [S8]; HLS/DASH are handled by the adaptive
  demux whose own description is "Unified adaptive streaming for DASH/HLS"
  [S10], and libVLC advertises HLS/DASH support [S11]. Generic HTTP reads issue
  `Range: bytes=N-` and detect seekability from `Accept-Ranges`/`206`
  [S10 `modules/access/http/file.c`].
- Options from VLC's own module descriptor: `http-referrer`, `http-user-agent`,
  `http-forward-cookies` (default true), `http-continuous`; the legacy HTTP
  access module adds `http-reconnect` [S8][S10]. There is **no arbitrary header
  option and no cookie-file option** in VLC's HTTP access source [S10]. Caveat:
  in current master `http-user-agent` is declared `change_safe()` +
  `change_private()`, so reliable command-line/per-item behaviour is
  version-dependent — **unverified** beyond the wiki description [S8][S10].
- Item-specific `:option=value` syntax is documented, including `:sub-file=`
  [S9]. For remote subtitles, source shows `--sub-file` is converted to a local
  file URI, while `--input-slave` accepts strings containing `://` as MRLs
  [S10 `src/input/input.c`]; the HTTP interface's `in_play` accepts `option`
  params and has an `addsubtitle` command, but that calls the _path_ variant
  (`add_subtitle`), while `add_subtitle_mrl` is a separate Lua binding
  [S10]. No per-subtitle request headers are documented.
- No error event: `status.json` publishes `vlc.playlist.status()` as `state`
  and has no error field [S10]. The `http-continuous` live path retries one read
  after an error, and the legacy `http-reconnect` option is described as
  reconnecting after a sudden disconnect; neither is an HTTP 403/410 refresh
  mechanism [S8][S10].

## MPC-HC / MPC-BE

- MPC-HC registers `http`, `https` and `icyx` with the LAV Splitter Source and
  explicitly enables `dash` and `hls` formats; `.m3u8` is registered too, so
  adaptive support is LAV's, not MPC-HC's own [S12]. MPC-BE's changelog records
  HLS M3U8 being passed to the source filter and MP4-DASH support [S13].
- Honest answer on headers: MPC-HC has `useragent`/`referrer` fields on
  `OpenFileData`, populated from youtube-dl metadata and handed to LAV's
  `LoadURL(url, ua, ref)` — there is no user-facing command-line or web-option
  that Autumn can use for an arbitrary URL [S12]. Its web interface's
  `/browser.html?path=<url>` sends a plain path/URL through `WM_COPYDATA` with
  no header fields [S12], and `/variables.html` exposes only playback state,
  position, duration and file [S12]. No cookie option or remote-subtitle-with-
  headers command exists in MPC-HC's own source, and none was found for MPC-BE
  [S12][S13]. Range/seek and 403/410 handling are delegated to LAV and are
  **unverified** from these repos.
- MPC-BE has no web-interface equivalent to MPC-HC's pages in the cloned source
  [S13], so Autumn's current MPC-HC seam does not transfer to it.

## 403/410 mid-playback and relay refresh

- **mpv/IINA:** HTTP status errors terminate the stream rather than fetching a
  new URL. mpv's libcurl backend retries only _transport_ failures on seekable
  transfers (timeout, connection drop, HTTP/2 reset, …) — 403/410 are not in
  that list [S2 `stream/stream_curl.c:700`]. The failure surfaces as
  `end-file` with `reason=error` and a `file_error` string; mpv's manual notes
  that `reason=eof` can also cover broken network connections [S1]. Autumn's
  pump ignores every `end-file` reason except `eof` (`crates/player/src/ipc.rs`),
  so today it sees neither an error nor an Eof — only a stalled position and,
  if the connection dies, `Exited`. `playback-restart` means "start of playback
  after seek or after file was loaded", not an error signal [S1].
- **Refresh options for mpv:** a fresh URL can be supplied with
  `["loadfile", url, "replace", -1, {"start": "<secs>"}]`, but that reopens and
  seeks mid-episode (ADR 0005 [S14]); global `set_property` of headers must precede
  the reload because streams read the global config at open time [S1][S2].
- **VLC:** there is no error payload in `status.json` [S10]; Autumn can only
  observe `state` changing (typically to stopped) and time stopping. The only retry
  paths are `http-continuous` (one read retry) and legacy `http-reconnect` on sudden
  disconnects [S8][S10]. Refresh means a new `in_play` with the new URL (and
  new item options), i.e. a restart, not a transparent continuation [S10].
- **MPC-HC/MPC-BE:** no error event in the web interface; a failed URL is
  visible only as stopped/unchanging state in `/variables.html` [S12], and the
  only reinvoke is `?path=<new url>`/`OpenFile`, which
  rebuilds the graph [S12]. Effectively a relaunch (ADR 0005).
- **Relay shape.** A stable loopback URL (`/r/<token>/<id>` in ARCHITECTURE §2)
  means the player never sees the upstream URL change: the relay can refresh an
  expiring signed URL, replay the same headers/cookies, serve Range requests and
  keep the byte position continuous. Crucially, for HLS/DASH it must rewrite the
  _manifest and segment URLs_, not just the top-level URL, because mpv marks
  those formats `nested_network` and VLC's adaptive demux opens nested URLs
  itself [S2][S10]. This is the only refresh strategy that preserves resume for
  all four players; header injection alone via mpv/IINA options does not solve
  manifest-embedded expiry.

## Open questions surfaced

- Standardise on relay-for-all remote sources, or allow direct URLs for
  header-free, non-expiring ones? (Direct mpv/IINA is cheap; VLC/MPC-HC are not.)
- Add a stream-error variant to `PlayerEvent` by observing `end-file reason` and
  `file_error`, and define how it interacts with the existing `Eof` path and
  `MAX_POLL_FAILURES` logic?
- How should `StreamSource` represent remote subtitle URLs _and their headers_,
  given mpv applies global headers to `sub-add` but VLC's `addsubtitle` is
  path-only and MPC-HC has no equivalent?
- What resume/continuity semantics do we want across a mid-episode refresh:
  relay-side byte continuity, or `loadfile … {"start": …}` with an accepted
  rebuffer?
- Do we need to support VLC's per-item `in_play&option=…` for UA/Referer, or is
  relay-for-VLC simpler and sufficient?
- What do we do about VLC's version-dependent `http-user-agent` (`change_private`
  in master) and MPC-BE's lack of a known control interface?
- Should mpv/IINA headers be set with `set_property` before every `loadfile`, or
  once at launch, given the global config-group read at stream open?

## Sources

1. [S1] mpv manual (stable): <https://mpv.io/manual/stable/> — Network options,
   `loadfile`, `sub-add`, `end-file`/`playback-restart`, Range/seek notes.
2. [S2] mpv source at commit `33b4c0511245924df90071f9302c4eb74b0282f8`
   (`0.42.0-UNKNOWN`): <https://github.com/mpv-player/mpv/tree/33b4c0511245924df90071f9302c4eb74b0282f8>
   — `DOCS/man/options.rst`, `stream/network.c`, `stream/network.h`,
   `stream/stream_lavf.c`, `stream/stream_curl.c`, `demux/demux_lavf.c`,
   `player/loadfile.c`, `player/command.c`, `RELEASE_NOTES`.
3. [S3] mpv JSON IPC docs:
   <https://github.com/mpv-player/mpv/blob/33b4c0511245924df90071f9302c4eb74b0282f8/DOCS/man/ipc.rst>.
4. [S4] IINA wiki, "MPV Options and Properties":
   <https://github.com/iina/iina/wiki/MPV-Options-and-Properties>.
5. [S5] IINA 1.5.0 release note (mpv 0.41.0, FFmpeg 9.0.1):
   <https://iina.io/release-note/1.5.0.html>.
6. [S6] IINA `iina-cli/main.swift` at commit
   `d71bb5c0feca2dbd9871a97f9091ad44d1079c1d`:
   <https://github.com/iina/iina/blob/d71bb5c0feca2dbd9871a97f9091ad44d1079c1d/iina-cli/main.swift>.
7. [S7] IINA `AppData.swift` (safe mpv option list) and `MPVOption.swift` at the
   same commit `d71bb5c0feca2dbd9871a97f9091ad44d1079c1d`:
   <https://github.com/iina/iina/tree/d71bb5c0feca2dbd9871a97f9091ad44d1079c1d>.
8. [S8] VideoLAN wiki, "Documentation: Modules/http":
   <https://wiki.videolan.org/Documentation:Modules/http/>.
9. [S9] VideoLAN wiki, "Documentation: Command line" and "VLC command-line
   help": <https://wiki.videolan.org/Documentation:Command_line/>,
   <https://wiki.videolan.org/VLC_command-line_help/>.
10. [S10] VLC source at commit `a75e766d82c461f3f5e8be7e91a8a2aaabc200c1`:
    <https://github.com/videolan/vlc/tree/a75e766d82c461f3f5e8be7e91a8a2aaabc200c1>
    — `modules/demux/adaptive/adaptive.cpp`, `modules/access/http/access.c`,
    `modules/access/http/file.c`, `modules/access/http/live.c`,
    `src/input/input.c`, `share/lua/intf/modules/httprequests.lua`,
    `modules/lua/libs/input.c`.
11. [S11] VideoLAN wiki, "LibVLC" (HLS/DASH in feature list):
    <https://wiki.videolan.org/LibVLC>.
12. [S12] MPC-HC source at commit
    `d86c1877e7fd169e6cd0e97cbfb169c56d82e3f3`:
    <https://github.com/clsid2/mpc-hc/tree/d86c1877e7fd169e6cd0e97cbfb169c56d82e3f3>
    — `src/mpc-hc/FGManager.cpp`, `src/mpc-hc/MainFrm.cpp`,
    `src/mpc-hc/WebClientSocket.cpp`, `src/mpc-hc/WebServer.cpp`.
13. [S13] MPC-BE source at commit
    `e14c0301eeb5d2ecd3b5cd3c38ca360311fa56d9`:
    <https://github.com/Aleksoid1978/MPC-BE/tree/e14c0301eeb5d2ecd3b5cd3c38ca360311fa56d9>
    — `docs/Changelog.txt`.
14. [S14] Autumn ADR 0005, `docs/adr/0005-torrent-streams-reach-player-via-token-gated-relay.md`
    (repo file, project context): ADR 0005.
