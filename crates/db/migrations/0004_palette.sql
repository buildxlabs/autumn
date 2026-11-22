-- Palette search cache (#63). Catalog results keep naming media by AniList
-- id (ADR 0001): a title is cached only when the user actually sees a result,
-- so browsing and searching never mint a MediaId. Local list entries join to
-- these titles through media_external_id.

CREATE TABLE catalog_title (
    anilist_id INTEGER NOT NULL,
    kind       TEXT    NOT NULL CHECK (kind IN ('anime', 'manga')),
    title      TEXT    NOT NULL,
    -- Alternate titles, one per line, matched fuzzily alongside `title`.
    aliases    TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (anilist_id, kind)
) STRICT;

CREATE TABLE recent_search (
    query      TEXT PRIMARY KEY,
    -- Sub-second, so queries made in the same second keep their order.
    created_at REAL NOT NULL
) STRICT;

CREATE INDEX recent_search_order ON recent_search (created_at DESC);

CREATE TABLE recent_view (
    anilist_id INTEGER NOT NULL,
    kind       TEXT    NOT NULL CHECK (kind IN ('anime', 'manga')),
    viewed_at  REAL    NOT NULL,
    PRIMARY KEY (anilist_id, kind)
) STRICT;

CREATE INDEX recent_view_order ON recent_view (viewed_at DESC);
