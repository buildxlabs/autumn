-- Settings: one JSON value per settings tab (decision #17). Replaces the
-- Phase 0 settings.json, which is imported once and retired.
CREATE TABLE settings (
    tab   TEXT PRIMARY KEY,
    value TEXT NOT NULL CHECK (json_valid(value))
) STRICT;

-- Set once the Phase 0 settings.json has been imported (or found corrupt), in
-- the same transaction as the rows, so the import never runs twice.
CREATE TABLE settings_import (
    id INTEGER PRIMARY KEY CHECK (id = 1)
) STRICT;
