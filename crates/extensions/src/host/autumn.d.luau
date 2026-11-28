--!strict
-- Autumn host API, api_version 1 (#267). Ships with the app for the
-- extension playground and editors.
--
-- A torrent provider's chunk returns `{ search = function(request) ... end }`.
-- Host calls look synchronous. A failing call raises a string
-- "<function>: <kind>: <detail>", where kind is one of invalid_argument,
-- blocked, network, timeout, too_large, rate_limited, parse or keychain.
-- Limit kills (CPU, memory, wall time, request count) cannot be caught.

export type ProviderRequest = {
	-- Always set: the user's text, or titles[1] for a structured search.
	keyword: string,
	-- titles, season and episode are all set (structured search) or all nil.
	titles: { string }?,
	season: number?,
	-- 0 means specials.
	episode: number?,
	absolute_episode: number?,
	anidb_aid: number?,
	anidb_eid: number?,
}

-- One result row. `name` and a v1 infohash (from `infohash` or the magnet's
-- btih) are required; other fields are optional and unknown fields ignored.
export type ProviderTorrent = {
	name: string,
	infohash: string?,
	magnet_url: string?,
	torrent_url: string?,
	-- Bytes.
	size: number?,
	seeders: number?,
	leechers: number?,
	downloads: number?,
	-- Unix seconds; autumn.time.parse produces it.
	published: number?,
	is_batch: boolean?,
	anidb_aid: number?,
	anidb_eid: number?,
	num_files: number?,
	files: { string }?,
}

export type Provider = {
	search: (request: ProviderRequest) -> { ProviderTorrent },
}

export type HttpRequest = {
	url: string,
	method: string?,
	headers: { [string]: string }?,
	body: string?,
	query: { [string]: string | number | boolean }?,
}

export type HttpResponse = {
	status: number,
	-- The final URL after redirects.
	url: string,
	-- Lowercased names; repeated headers joined with ", ".
	headers: { [string]: string },
	set_cookies: { string },
	body: string,
}

export type HtmlMatch = {
	text: string,
	html: string,
	attrs: { [string]: string },
}

-- "css" takes the text of the first match; the table form reads an attribute
-- and/or every match.
export type HtmlField = string | { css: string, attr: string?, all: boolean? }

declare autumn: {
	http: {
		-- A non-2xx status is not an error. At most 8 requests per call.
		fetch: (request: HttpRequest) -> HttpResponse,
	},
	html: {
		select: (html: string, css: string) -> { HtmlMatch },
		-- One table per row match; a missing field match is nil.
		extract: (html: string, row_css: string, fields: { [string]: HtmlField }) -> { { [string]: any } },
	},
	json: {
		-- JSON null decodes to nil.
		decode: (text: string) -> any,
		encode: (value: any) -> string,
	},
	url: {
		-- Percent-encodes one component.
		encode: (text: string) -> string,
		decode: (text: string) -> string,
		-- Appends the query with keys sorted.
		build: (base: string, query: { [string]: string | number | boolean }) -> string,
		resolve: (base: string, href: string) -> string,
	},
	base64: {
		encode: (bytes: string) -> string,
		decode: (text: string) -> string,
	},
	hex: {
		encode: (bytes: string) -> string,
		decode: (text: string) -> string,
	},
	crypto: {
		-- Lowercase hex digests.
		sha1: (bytes: string) -> string,
		sha256: (bytes: string) -> string,
	},
	time: {
		-- Unix seconds, UTC.
		now: () -> number,
		-- RFC 3339, RFC 2822 or "YYYY-MM-DD HH:MM[:SS]" (UTC); nil otherwise.
		parse: (text: string) -> number?,
	},
	log: {
		-- Arguments are tostring'd and joined with spaces. Lines over 4 KiB
		-- are cut, and a call logs at most 200 lines.
		debug: (...any) -> (),
		info: (...any) -> (),
		warn: (...any) -> (),
		error: (...any) -> (),
	},
	settings: {
		-- The extension's own setting, as declared in its manifest.
		get: (key: string) -> (string | number | boolean)?,
	},
}
