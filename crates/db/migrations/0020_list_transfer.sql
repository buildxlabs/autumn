-- Resume points for one-shot list transfers (ticket #67, ADR 0004). One row
-- per direction, holding the reviewed choices and the entries already done.
CREATE TABLE list_transfer (
    direction TEXT PRIMARY KEY,
    value     TEXT NOT NULL CHECK (json_valid(value))
) STRICT;
