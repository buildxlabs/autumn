# Torrent providers join search as isolated indexers, and a failed indexer no longer fails the search

Phase 3's search hard-coded Nyaa and AnimeTosho and failed when either failed. Phase 4 adds user-installed torrent-provider extensions, which fail, stall and get killed far more often. Every indexer now runs in its own lane: an extension lane gets one `search(request)` call per search, a 10 s search deadline, no host retry and a 60 s cache keyed by `(id, version, request)`. Its results must carry a v1 infohash and cannot set trust flags, are labelled `Indexer::Extension { id, name }`, and dedupe after the built-ins. A search fails only when every lane fails; otherwise it returns its results plus a per-indexer failure list. Decided on #264, with the result contract from #268.

## Consequences

- Auto-select runs on the lanes that answered and falls back to the release list with `IndexerError` only when all fail (this amends ADR 0007's fallback trigger); partial failures are named in its progress or result.
- Extensions take part in the release list, auto-select and batch suggestions, but not availability badges, which stay on the built-in indexers to keep unattended traffic off third-party hosts.
- Extension results are validated row by row: rows without a name or v1 infohash are dropped, bad optional fields are cleared, unknown fields are ignored; a call fails with `InvalidResult` only if its return is not an array or every returned row is invalid. The provider contract is versioned by the manifest's single `api_version`, additively within a version.
- The search deadline cancels the call; it is not a VM kill and does not count toward ADR 0014's auto-disable.
- Updating, disabling or removing an extension purges its cached results; rows already on screen keep their copied label and still stream, since a torrent never calls back into its extension.

## Considered options

- **Keep the whole-search failure.** One broken extension would hide Nyaa.
- **Host-side `plan` fan-out against each extension.** Up to eight calls per extension per search against a 30 req/min limit; the extension chooses its own variants within ADR 0014's caps.
- **Hash-less results, or extension-asserted trust/best-release flags.** Everything downstream keys on the infohash, and the flags are unverifiable.
- **Extensions in availability badges.** Background load on user-installed hosts and a change to the persisted badge schema.
