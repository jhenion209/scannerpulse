-- An older archive imported where it is: its folder is a read-only location,
-- and its calls name its source, whose ingest rows are its register. The
-- register stays when the location is removed (location_id then NULL), so a
-- later import of the same folder takes only the files it has not taken.
-- The offset corrects the times its file names give. Additive: an older
-- version ignores the table.
CREATE TABLE imports (
    source_id      smallint PRIMARY KEY REFERENCES sources (id),
    location_id    smallint UNIQUE REFERENCES locations (id),
    offset_minutes integer NOT NULL DEFAULT 0
        CONSTRAINT imports_offset CHECK (offset_minutes BETWEEN -1440 AND 1440),
    created_at     timestamptz NOT NULL DEFAULT now()
);

-- An import's files are listed, not waiting: the filer of every version takes
-- the waiting files and deletes each from its folder once filed, and an
-- older version's filer does not know the imports above.
ALTER TABLE ingest_files DROP CONSTRAINT ingest_files_state;
ALTER TABLE ingest_files ADD CONSTRAINT ingest_files_state
    CHECK (state IN ('waiting', 'done', 'rejected', 'staged', 'duplicate', 'listed'));
