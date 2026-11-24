-- A file on a disconnected library drive keeps its match and is shelved
-- instead of deleted. The next scan of the returned folder unshelves it.
ALTER TABLE local_file ADD COLUMN shelved INTEGER NOT NULL DEFAULT 0 CHECK (shelved IN (0, 1));
CREATE INDEX local_file_shelved ON local_file (shelved);
