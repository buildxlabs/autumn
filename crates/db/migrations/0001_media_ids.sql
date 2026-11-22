-- Media identity (ADR 0001): Autumn's own id, minted on the first user fact.
CREATE TABLE media (
    id                   INTEGER PRIMARY KEY,
    kind                 TEXT    NOT NULL CHECK (kind IN ('anime', 'manga')),
    -- Set when AniList merged or deleted the linked record; cleared on relink.
    missing_from_anilist INTEGER NOT NULL DEFAULT 0 CHECK (missing_from_anilist IN (0, 1))
) STRICT;

-- Cross-references: each (provider, external id) names at most one media, and
-- a media has at most one external id per provider. Providers that number
-- anime and manga separately get one provider value per kind ('mal_anime',
-- 'mal_manga'); AniList shares one sequence ('anilist').
CREATE TABLE media_external_id (
    provider    TEXT    NOT NULL,
    external_id TEXT    NOT NULL,
    media_id    INTEGER NOT NULL REFERENCES media (id) ON DELETE CASCADE,
    PRIMARY KEY (provider, external_id),
    UNIQUE (media_id, provider)
) STRICT;
