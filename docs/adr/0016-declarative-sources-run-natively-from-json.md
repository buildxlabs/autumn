# Declarative sources run natively in Rust from a JSON definition

A declarative torrent provider (`runtime: "declarative"`) is a JSON definition inlined in the extension manifest under `declarative` (or inlined from `payload_url` at install), versioned by the manifest's `api_version`. It runs natively in Rust (`scraper`, `quick-xml`, `serde_json`, `regex`) behind the same `search(ProviderRequest) -> Vec<ProviderTorrent>` contract, outbound guard, rate limit and timeouts as Luau extensions, without a Luau VM. Phase 4 covers one HTTP `GET` or `POST` per search returning HTML, XML/RSS or JSON, with restricted `{{ ... }}` placeholders, `html.extract`-shaped selectors and explicit typed filters; raw release names go straight to `release-parser`. Multi-step detail fetches, paging, login flows and request signing stay Luau-only. Decided on #271.

## Consequences

- Declarative definitions skip VM memory and startup cost and cannot loop or ReDoS (`regex` is linear-time), so ADR 0014's CPU and memory caps and auto-disable rule do not apply; they share row-by-row validation, the outbound guard and the playground with Luau providers.
- XML and RSS feeds use `quick-xml`, not `scraper`, because html5ever's void-element rules drop the text of RSS `<link>`.
- Unknown schema keys, filter names and output fields, and invalid selectors or regexes, fail at load (`InvalidProvider`) instead of silently dropping fields at search time.
- Settings reach a definition only through request templates evaluated in memory; `secret` values are redacted from logs and traces, and no template expands into code.

## Considered options

- **Interpreting definitions inside a Luau VM.** VM overhead and Lua escaping for a format that is not Turing-complete.
- **Cardigann-compatible YAML or TOML.** An extra parser, a second file format beside the JSON manifest, Cardigann's title-rewriting chains that duplicate `release-parser`, and coupling to an outside schema (D10).
- **Multi-step `download` blocks, paging or if/else in the schema.** It turns the schema into an ad-hoc scripting language when Luau already covers multi-request providers.
