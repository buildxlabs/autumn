-- One row per library scan, with its outcome. Only the newest twenty are kept.
-- Every unmatched file carries why it was left without a media; the best score
-- and candidate are stored only for a match below the threshold.
CREATE TABLE scan_summary (
    id              INTEGER PRIMARY KEY,
    started_at      INTEGER NOT NULL, -- Unix seconds
    duration_ms     INTEGER NOT NULL,
    folders         TEXT    NOT NULL CHECK (json_valid(folders)),
    files_walked    INTEGER NOT NULL,
    files_matched   INTEGER NOT NULL,
    files_unmatched INTEGER NOT NULL,
    locked_count    INTEGER NOT NULL,
    ignored_count   INTEGER NOT NULL,
    shelved_count   INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE scan_summary_media (
    summary_id        INTEGER NOT NULL REFERENCES scan_summary (id) ON DELETE CASCADE,
    media_id          INTEGER NOT NULL REFERENCES media (id) ON DELETE CASCADE,
    matched_files     INTEGER NOT NULL,
    lowest_confidence REAL NOT NULL,
    PRIMARY KEY (summary_id, media_id)
) STRICT;

CREATE TABLE scan_summary_unmatched (
    summary_id     INTEGER NOT NULL REFERENCES scan_summary (id) ON DELETE CASCADE,
    path           TEXT NOT NULL,
    reason         TEXT NOT NULL CHECK (
        reason IN ('no_title', 'no_candidate', 'below_threshold', 'ignored', 'shelved')
    ),
    best_score     REAL,
    best_candidate TEXT,
    PRIMARY KEY (summary_id, path)
) STRICT;
