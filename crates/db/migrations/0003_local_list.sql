-- The Local platform's list entries (decision #16, ADR 0004): AniList's full
-- MediaList field set, one entry per media. Scores are raw 0-100, 0 meaning
-- unscored. Fuzzy dates keep each part in its own nullable column. Times are
-- Unix seconds.
CREATE TABLE local_list_entry (
    media_id                 INTEGER PRIMARY KEY REFERENCES media (id) ON DELETE CASCADE,
    status                   TEXT    NOT NULL CHECK (
        status IN ('current', 'planning', 'completed', 'dropped', 'paused', 'repeating')
    ),
    score                    INTEGER NOT NULL CHECK (score BETWEEN 0 AND 100),
    progress                 INTEGER NOT NULL CHECK (progress >= 0),
    progress_volumes         INTEGER NOT NULL CHECK (progress_volumes >= 0),
    repeat                   INTEGER NOT NULL CHECK (repeat >= 0),
    started_year             INTEGER,
    started_month            INTEGER,
    started_day              INTEGER,
    completed_year           INTEGER,
    completed_month          INTEGER,
    completed_day            INTEGER,
    notes                    TEXT    NOT NULL,
    private                  INTEGER NOT NULL CHECK (private IN (0, 1)),
    priority                 INTEGER NOT NULL CHECK (priority >= 0),
    score_story              INTEGER NOT NULL CHECK (score_story BETWEEN 0 AND 100),
    score_characters         INTEGER NOT NULL CHECK (score_characters BETWEEN 0 AND 100),
    score_visuals            INTEGER NOT NULL CHECK (score_visuals BETWEEN 0 AND 100),
    score_audio              INTEGER NOT NULL CHECK (score_audio BETWEEN 0 AND 100),
    score_enjoyment          INTEGER NOT NULL CHECK (score_enjoyment BETWEEN 0 AND 100),
    hidden_from_status_lists INTEGER NOT NULL CHECK (hidden_from_status_lists IN (0, 1)),
    created_at               INTEGER NOT NULL,
    updated_at               INTEGER NOT NULL
) STRICT;

-- Custom-list membership: an entry can be in several custom lists. A list's
-- media kind is its entries' kind.
CREATE TABLE local_custom_list_entry (
    media_id INTEGER NOT NULL REFERENCES local_list_entry (media_id) ON DELETE CASCADE,
    name     TEXT    NOT NULL,
    PRIMARY KEY (media_id, name)
) STRICT;
