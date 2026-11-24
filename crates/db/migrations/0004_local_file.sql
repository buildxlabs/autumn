-- One row per local file found by a library scan. The path is the identity:
-- a re-scan updates the row in place and removes rows for files that are gone.
CREATE TABLE local_file (
    path        TEXT    PRIMARY KEY,
    size        INTEGER NOT NULL,
    modified    INTEGER NOT NULL, -- seconds since the Unix epoch
    title       TEXT,
    group_name  TEXT,
    season      INTEGER,
    episode     INTEGER,
    kind        TEXT    NOT NULL DEFAULT 'episode',
    media_id    INTEGER REFERENCES media (id) ON DELETE SET NULL,
    confidence  REAL,
    -- Reserved for #124; a scan never changes either flag.
    locked      INTEGER NOT NULL DEFAULT 0 CHECK (locked IN (0, 1)),
    ignored     INTEGER NOT NULL DEFAULT 0 CHECK (ignored IN (0, 1))
) STRICT;

CREATE INDEX local_file_media_id ON local_file (media_id);
