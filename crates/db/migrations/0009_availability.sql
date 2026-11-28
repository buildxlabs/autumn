-- Availability badge results (#251, #181 Q13). One row per checked
-- (media, episode, indexer); the 24 h freshness rule is applied by `search`.
-- Only releases found on an indexer are stored: an indexer error leaves no
-- row, so a restart can check the episode again.
CREATE TABLE availability (
    media_id  INTEGER NOT NULL REFERENCES media (id) ON DELETE CASCADE,
    episode   INTEGER NOT NULL CHECK (episode > 0),
    indexer   TEXT    NOT NULL CHECK (indexer IN ('nyaa', 'animetosho')),
    found     INTEGER NOT NULL CHECK (found IN (0, 1)),
    checked_at INTEGER NOT NULL, -- Unix seconds
    PRIMARY KEY (media_id, episode, indexer)
) STRICT;
