# Episode matching and auto-select are client-side on `release-parser` output

Both indexers are raw keyword sources and `release-parser` is infallible, while fansubs number episodes seasonally or absolutely and batches need file-level proof. `search` does its own kind, episode and season matching against `release-parser` output: it rejects a known-season mismatch, accepts an unknown season, never matches `S00`/specials, treats `NNvK` versions as the same episode while preferring the highest, and confirms a batch by finding the episode's file before accepting it. Auto-select applies hard filters first, then a fixed order of ranked criteria, verifies the top three candidates at file level, and falls back to the manual release list on `NoTorrentsMatch`, `NoFileFound` or `IndexerError` rather than silently relaxing anything. Since Phase 4, `IndexerError` means every indexer failed: auto-select runs on the indexers that answered and names the ones that did not (amended by ADR 0011, #264).

## Consequences

- The `library` matcher is not reused: matching and ranking are different jobs, and adding that crate edge would change ARCHITECTURE §3.
- Title relevance rests on query construction and AniDB `aid`; matching stays pure and testable, and Phase 5's `debrid` shares the result type.
- A missing indexer is not a relaxed filter: the match criteria are unchanged, so running on the indexers that answered keeps the no-relaxing rule (ADR 0011).

## Considered options

- **Reuse `library::matcher`.** It solves a different problem and would add a `search → library` edge.
- **Server-side provider filtering.** Neither indexer offers it beyond raw keywords.
- **Relaxed retry instead of a manual fallback.** It would invent a match or play an unverified file.
