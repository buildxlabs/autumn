# Media are identified by our own id, minted on the first user fact

Every stored fact about a media (list entries, library matches, continuity, rules, downloads) references Autumn's own `MediaId`, not an AniList id. The point is to keep AniList, MyAnimeList, AniDB, Kitsu and custom sources equal. External ids are cross-references: `UNIQUE(provider, external_id)`, so each one names at most one media. Before minting, every external id we know for the item is looked up and a match is reused. A `MediaId` is minted only when the user records something. Catalog data, caches, browse and search results refer to media by external id and never mint one. Anime and manga share the one id, at AniList's season/cour granularity.

## Consequences

- Conflicting cross-references are never merged automatically. The first link wins, and the conflict is logged. Merging two media is an explicit user action.
- When a linked AniList record is merged or deleted, the media keeps its id and all user data. It is flagged and can be relinked by replacing one cross-reference, so nothing has to be rewritten.
- A custom source item is the external id `Custom(source, key)` and mints like any other provider. It needs no reserved id range.
- Phase 1 stores only the AniList id and `idMal`. anime-offline-database and ani.zip mappings arrive with the Phase 2 matcher.

## Considered options

- **Raw AniList id everywhere** (seanime). Simplest, but it makes AniList the only real identity, and custom sources need id-range packing.
- **Provider-scoped key plus an alias table.** It needs no minting, but one show can sit under several keys until it is merged, and every read has to resolve aliases.
- **Key follows the active platform.** Switching platform would re-key every row.
