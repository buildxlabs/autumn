--!strict
-- Autumn host API, api_version 1 (#267).
--
-- Ships with the app for the extension playground and editors. Every name
-- below is part of api_version 1; within a version, changes are additive
-- only (new functions, new optional fields, new error kinds).
--
-- A torrent provider's chunk must return a table with a `search` function:
--
--     return { search = function(request) ... return rows end }
--
-- Host calls look synchronous: `autumn.json.decode(s)` returns its value
-- directly, even for calls that wait on the network.
--
-- Errors. A failing host call raises a string error of the form
--
--     "<function>: <kind>: <detail>"
--
-- for example "autumn.json.decode: parse: EOF while parsing an object". Every
-- such error can be caught with `pcall`, and scripts may branch on the kind
-- with `string.find(err, ": parse: ", 1, true)`. The kinds are:
--
--   invalid_argument  an argument has the wrong type or an invalid value
--   blocked           the outbound guard refused a request (http.fetch)
--   network           the connection or transfer failed (http.fetch)
--   timeout           the request took too long (http.fetch)
--   too_large         a request or response body is over its cap (http.fetch)
--   rate_limited      the extension is over its request rate (http.fetch)
--   parse             the input could not be parsed
--   keychain          a secret setting could not be read (settings.get)
--
-- Limit kills are not errors and cannot be caught: once a call goes over
-- 1 s of CPU time, 32 MiB of memory, 30 s of wall time or 8 HTTP requests,
-- or the user cancels it, the host aborts the whole call. `pcall`, `xpcall`
-- and `coroutine.resume` re-raise the kill, and the call's result is the
-- kill whatever the script returns. Running out of memory counts as a kill
-- even inside `pcall`.
--
-- Not available in the sandbox: `require`, `loadstring`, `getfenv`,
-- `setfenv`, `io`, `os` other than `os.clock/date/difftime/time`, file or
-- process access. `print` writes to the log like `autumn.log.info`.

-- What `search` receives. A fresh table each call; the script may change it.
export type ProviderRequest = {
	-- Always set: the user's text for a keyword search, or `titles[1]` for a
	-- structured search.
	keyword: string,
	-- Title variants, most specific first; `titles[1] == keyword`. Set only
	-- for a structured search, together with `season` and `episode`; `nil`
	-- for a keyword search.
	titles: { string }?,
	-- The season, 1 or more. Set with `titles` and `episode`.
	season: number?,
	-- The episode within the season; 0 means specials. Set with `titles` and
	-- `season`, so `if request.episode then` tells the two searches apart.
	episode: number?,
	-- The episode number counted from the first season, when known.
	absolute_episode: number?,
	-- The AniDB anime id, when known.
	anidb_aid: number?,
	-- The AniDB episode id, when known.
	anidb_eid: number?,
}

-- One row `search` returns.
--
-- `search` must return an array: a table with keys exactly 1..n (an empty
-- table means no results). Anything else (nil, a string, a table with other
-- keys or holes) fails the call with
-- "invalid result: results: expected array, got <type>".
--
-- Each row needs a `name` and a v1 infohash, taken from `infohash` or from
-- the `xt=urn:btih:` part of `magnet_url`; rows without them are dropped.
-- A field of the wrong type or out of range is cleared and the row kept.
-- Numbers must be numbers: convert "1.2 GiB" or "12" yourself. Unknown
-- fields, including `trusted` or `remake`, are ignored. If every returned
-- row is invalid, the call fails with "invalid result". Rows after the
-- 200th are dropped.
export type ProviderTorrent = {
	-- The release name, as the site shows it. Required; empty after trimming
	-- counts as missing.
	name: string,
	-- 40 hex or 32 base32 characters. Must agree with `magnet_url` when both
	-- are set, or the row is dropped.
	infohash: string?,
	-- A `magnet:` URI. Its `xt=urn:btih:` supplies the hash when `infohash`
	-- is absent; a v2-only magnet (`btmh` only) has no v1 hash.
	magnet_url: string?,
	-- An http(s) link to the `.torrent` file on a declared host; cleared
	-- otherwise.
	torrent_url: string?,
	-- Total size in bytes, a whole number 0 or more.
	size: number?,
	-- Seeders, a whole number 0 or more.
	seeders: number?,
	-- Leechers, a whole number 0 or more.
	leechers: number?,
	-- Completed downloads, a whole number 0 or more.
	downloads: number?,
	-- When the release was published, in Unix seconds;
	-- `autumn.time.parse` produces it.
	published: number?,
	-- A hint that the torrent is a batch. Autumn still decides episodes from
	-- the name.
	is_batch: boolean?,
	-- The AniDB anime id, a whole number 1 or more.
	anidb_aid: number?,
	-- The AniDB episode id, a whole number 1 or more.
	anidb_eid: number?,
	-- The number of files in the torrent, a whole number 0 or more.
	num_files: number?,
	-- File names in the torrent; entries that aren't strings are dropped.
	files: { string }?,
}

-- What a torrent provider's chunk returns. Other keys are ignored.
--
-- If the chunk raises, the extension fails to load with its error message
-- and traceback. If it returns something else, it fails with
-- "invalid provider: provider: expected table, got <type>" or
-- "invalid provider: search: expected function, got <type>".
-- An error `search` doesn't catch fails the call with the error's message
-- (or `err.message` when the error is a table with a string `message`),
-- cut to 1 KiB, and a traceback whose frames read `<extension id>:<line>`.
export type Provider = {
	search: (request: ProviderRequest) -> { ProviderTorrent },
}

-- The argument of `autumn.http.fetch`.
export type HttpRequest = {
	-- An absolute http(s) URL. Its host must be one of the manifest's
	-- `permissions.hosts`, or a LAN host the user approved for this
	-- extension.
	url: string,
	-- "GET", "POST", "PUT", "PATCH" or "DELETE", in any case; "GET" when
	-- absent.
	method: string?,
	-- Request headers by name. Hop-by-hop headers (`Connection`,
	-- `Keep-Alive`, `Transfer-Encoding` and the like) are removed, and
	-- `User-Agent` defaults to Autumn's when not set.
	headers: { [string]: string }?,
	-- The request body as a byte string, at most 1 MiB.
	body: string?,
	-- Query parameters appended to `url` with keys sorted and form-encoded
	-- (a space becomes `+`), as `autumn.url.build` does. Values must be
	-- strings, numbers or booleans.
	query: { [string]: string | number | boolean }?,
}

-- What `autumn.http.fetch` returns.
export type HttpResponse = {
	-- The HTTP status. Any status, including 404 or 500, is returned, not
	-- raised; only 429 raises `rate_limited`.
	status: number,
	-- The URL that answered, after redirects (at most 10, each held to the
	-- same host rules).
	url: string,
	-- Response headers by lowercased name; a repeated header's values are
	-- joined with ", " in order. Values that aren't visible ASCII are left
	-- out.
	headers: { [string]: string },
	-- Every raw `Set-Cookie` value, in order. Autumn keeps no cookies
	-- between requests; send them back yourself in `headers.Cookie`.
	set_cookies: { string },
	-- The response body as a byte string, at most 4 MiB.
	body: string,
}

-- One match of `autumn.html.select`.
export type HtmlMatch = {
	-- The element's text content, trimmed.
	text: string,
	-- The element's outer HTML.
	html: string,
	-- The element's attributes by name.
	attrs: { [string]: string },
}

-- One field of `autumn.html.extract`, looked up inside each row.
--
-- A string is a CSS selector, and the field is the trimmed text of its first
-- match. The table form reads `attr` instead of the text when set, and an
-- array of every match when `all` is true. A field with no match is nil (or
-- an empty array with `all`).
export type HtmlField = string | {
	-- The CSS selector, matched inside the row.
	css: string,
	-- The attribute to read, for example "href"; the text when absent.
	attr: string?,
	-- true for an array of every match; false or absent for the first.
	all: boolean?,
}

declare autumn: {
	http: {
		-- Sends one HTTP request through Autumn's outbound guard and returns
		-- the response. The script waits without using CPU time; the call's
		-- 30 s wall clock still runs, and cancelling the call cancels the
		-- request.
		--
		-- Limits: 8 requests per call. The 9th aborts the whole call with a
		-- request-limit kill, which `pcall` cannot catch; the extension's VM
		-- is rebuilt for the next call. Each extension may also send 30
		-- requests a minute, in bursts of 10; requests over that rate wait.
		-- Bodies are capped at 1 MiB sent and 4 MiB received. No redirects
		-- past 10, and nothing is retried.
		--
		-- Raises (all catchable with `pcall`):
		--   invalid_argument  a bad request field: "url: missing",
		--                     "url: expected string, got <type>",
		--                     "url: <parse error>", "method: \"X\" is not GET,
		--                     POST, PUT, PATCH or DELETE",
		--                     "query.<key>: expected string, number or boolean,
		--                     got <type>", "headers.<name>: <reason>",
		--                     "body: expected string, got <type>"
		--   blocked           the host isn't declared, or is localhost, a
		--                     `.local` name or a private, loopback or reserved
		--                     address (also when a redirect leads there), as
		--                     "autumn.http.fetch: blocked: <host> <reason>". Left uncaught, the
		--                     call fails as a network block in the extension's
		--                     health.
		--   too_large         the body sent is over 1 MiB, or the response is
		--                     over 4 MiB
		--   rate_limited      the site answered 429, or the extension is
		--                     paused after one; the detail gives the wait
		--   timeout           the connection or response timed out
		--   network           any other transport failure (DNS, TLS,
		--                     connection reset)
		fetch: (req: HttpRequest) -> HttpResponse,
	},
	html: {
		-- Parses `html` once and returns every element matching `css`, in
		-- document order.
		--
		-- Raises: invalid_argument when `css` is not a valid selector
		-- ("css: ...") or an argument is not a string.
		select: (html: string, css: string) -> { HtmlMatch },
		-- Parses `html` once and returns one table per element matching
		-- `row_css`, with each of `fields` looked up inside that row. Works on
		-- table rows (`tbody tr`) and cells.
		--
		-- Raises: invalid_argument when `row_css` or a field's css is not a
		-- valid selector ("row_css: ...", "fields.<name>.css: ..."), or a
		-- field spec has the wrong shape ("fields.<name>: expected string or
		-- table", "fields.<name>.all: expected boolean").
		extract: (html: string, row_css: string, fields: { [string]: HtmlField }) -> { { [string]: any } },
	},
	json: {
		-- Decodes JSON text into Luau values. Objects and arrays become
		-- tables; JSON `null` becomes nil, so an array with nulls has holes.
		--
		-- Raises: parse when `text` is not valid JSON.
		decode: (text: string) -> any,
		-- Encodes a Luau value as compact JSON with object keys sorted. A
		-- table with keys 1..n encodes as an array, an empty table as `{}`.
		--
		-- Raises: invalid_argument when the value holds something JSON can't
		-- represent, such as a function.
		encode: (value: any) -> string,
	},
	url: {
		-- Percent-encodes one URL component: every byte except A-Z, a-z,
		-- 0-9 and `-._~` becomes %XX. A space becomes %20.
		encode: (text: string) -> string,
		-- Decodes %XX sequences in one component. A malformed sequence is
		-- kept as written; `+` stays `+`.
		decode: (text: string) -> string,
		-- Appends `query` to `base` with keys sorted, form-encoded (a space
		-- becomes `+`). An existing query in `base` is kept.
		--
		-- Raises: invalid_argument when `base` is not an absolute URL
		-- ("base: ...") or a value is not a string, number or boolean
		-- ("query.<key>: ...").
		build: (base: string, query: { [string]: string | number | boolean }) -> string,
		-- Resolves `href` (absolute, root-relative or relative) against the
		-- absolute URL `base`.
		--
		-- Raises: invalid_argument when `base` is not an absolute URL
		-- ("base: ...") or `href` can't be resolved ("href: ...").
		resolve: (base: string, href: string) -> string,
	},
	base64: {
		-- Encodes bytes as standard base64 with padding.
		encode: (bytes: string) -> string,
		-- Decodes standard base64 with padding into bytes.
		--
		-- Raises: parse when `text` is not valid base64.
		decode: (text: string) -> string,
	},
	hex: {
		-- Encodes bytes as lowercase hex.
		encode: (bytes: string) -> string,
		-- Decodes hex of either case into bytes.
		--
		-- Raises: parse when `text` has an odd length or a non-hex digit.
		decode: (text: string) -> string,
	},
	crypto: {
		-- The SHA-1 digest of `bytes`, as 40 lowercase hex characters.
		sha1: (bytes: string) -> string,
		-- The SHA-256 digest of `bytes`, as 64 lowercase hex characters.
		sha256: (bytes: string) -> string,
	},
	time: {
		-- The current time in whole Unix seconds (UTC).
		now: () -> number,
		-- Parses a date into whole Unix seconds. Accepts RFC 3339
		-- ("2024-01-02T03:04:05Z"), RFC 2822 ("Tue, 02 Jan 2024 03:04:05
		-- +0000") and "YYYY-MM-DD HH:MM[:SS]", read as UTC. Surrounding
		-- spaces are ignored. Returns nil for anything else; it never raises
		-- for a bad date.
		parse: (text: string) -> number?,
	},
	log: {
		-- Each function writes one line to Autumn's log, tagged with the
		-- extension id. Arguments are converted with `tostring` and joined
		-- with spaces. Values of the extension's secret settings are
		-- replaced with `<redacted>`. Lines over 4 KiB are cut. A call may
		-- log at most 200 lines; later lines are dropped and one
		-- "… N lines dropped" note is written when the call ends. Debug
		-- lines are hidden from the log file by default.
		debug: (...any) -> (),
		info: (...any) -> (),
		warn: (...any) -> (),
		error: (...any) -> (),
	},
	settings: {
		-- Returns the value of one of the extension's own settings, as
		-- declared in its manifest: text and select settings as strings,
		-- number settings as numbers, toggles as booleans, secret settings as
		-- strings read from the OS keychain. An unset setting gives its
		-- declared default, or nil. Values are a snapshot taken when the call
		-- started.
		--
		-- Raises: invalid_argument for a key the manifest doesn't declare,
		-- keychain when a secret can't be read.
		get: (key: string) -> (string | number | boolean)?,
	},
}
