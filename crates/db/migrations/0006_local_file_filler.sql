-- Filler marking from the recorded filler list (#122, OD-3). A scan sets the
-- flag from the metadata lookup; user overrides never change it.
ALTER TABLE local_file ADD COLUMN filler INTEGER NOT NULL DEFAULT 0 CHECK (filler IN (0, 1));
