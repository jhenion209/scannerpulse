-- The same call from two recorders: the longer copy is kept.
-- duplicate: a file that was a copy of the call (call_id, call_started_at)
-- another recorder had filed, no longer than it; it was deleted from its
-- recorder's folder and never written to the archive.
ALTER TABLE ingest_files DROP CONSTRAINT ingest_files_state;
ALTER TABLE ingest_files ADD CONSTRAINT ingest_files_state
    CHECK (state IN ('waiting', 'done', 'rejected', 'staged', 'duplicate'));

-- The recorders a call had a copy from besides its own (source_id): a copy
-- deleted as a duplicate, or the recording a longer copy replaced. A recorder
-- never sends two copies of one call, so no later file of these recorders is
-- taken for a copy of the call.
ALTER TABLE calls ADD COLUMN copies_from smallint[];

-- Archive files no call names any more: the recording a longer copy replaced,
-- deleted by the filer once the call names the new one.
CREATE TABLE archive_leftovers (
    location_id smallint NOT NULL REFERENCES locations (id),
    rel_path    text NOT NULL,
    PRIMARY KEY (location_id, rel_path)
);

-- A row of a file dealt with before keeps its call only while the file is
-- unchanged; rows left waiting by an older version start without one.
UPDATE ingest_files SET call_id = NULL, call_started_at = NULL WHERE state = 'waiting';
