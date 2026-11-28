-- Installed extensions (ADR 0012, #265 §4): one validated manifest and its
-- inlined payload per row. `extensions` validates both before they reach
-- this table. `enabled` is the user's toggle; `auto_disabled` and
-- `last_error` are extension health (#269), kept independent of it.
CREATE TABLE extensions (
    id            TEXT    NOT NULL PRIMARY KEY,
    origin_url    TEXT    NOT NULL,
    version       TEXT    NOT NULL,
    type          TEXT    NOT NULL,
    runtime       TEXT    NOT NULL,
    manifest_json TEXT    NOT NULL,
    payload       TEXT    NOT NULL,
    enabled       INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    auto_disabled INTEGER NOT NULL DEFAULT 0 CHECK (auto_disabled IN (0, 1)),
    last_error    TEXT,
    installed_at  INTEGER NOT NULL, -- Unix seconds
    updated_at    INTEGER NOT NULL  -- Unix seconds
) STRICT;
