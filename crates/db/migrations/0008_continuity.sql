-- Resume positions for episodes the user has not watched yet (#128, ADR 0001).
-- A watched episode's row is cleared, so it never resumes past its threshold.
CREATE TABLE continuity (
    media_id  INTEGER NOT NULL REFERENCES media (id) ON DELETE CASCADE,
    episode   INTEGER NOT NULL CHECK (episode > 0),
    position  REAL    NOT NULL CHECK (position >= 0),
    duration  REAL, -- NULL when the player has not reported a duration
    updated_at INTEGER NOT NULL, -- seconds since the Unix epoch
    PRIMARY KEY (media_id, episode)
) STRICT;
