-- Several recorders: each SDRTrunk folder ingest takes calls from is a row of
-- `sources`. `enabled` keeps its meaning, "the watcher lists it now", so that
-- an older version running beside this one during an update reads it rightly:
-- a paused recorder, one taken in when asked and a removed one have it false.
ALTER TABLE sources
    ADD COLUMN intake text NOT NULL DEFAULT 'watched'
        CONSTRAINT sources_intake CHECK (intake IN ('watched', 'asked')),
    -- A removed recorder's row stays, so that its calls stay credited to it.
    ADD COLUMN removed_at timestamptz,
    -- The last "Bring in now" of a recorder taken in when asked, and the
    -- request a watcher has since listed its folder for: the files there at
    -- bring_in_at become waiting files once.
    ADD COLUMN bring_in_at timestamptz,
    ADD COLUMN brought_in_at timestamptz;

-- What each host's watcher last found in each recorder's folder, so that the
-- admin page can say on which machine a folder cannot be read.
CREATE TABLE source_health (
    host       text NOT NULL,
    source_id  smallint NOT NULL REFERENCES sources (id),
    ok         boolean NOT NULL,
    problem    text,
    files      integer,
    checked_at timestamptz NOT NULL,
    PRIMARY KEY (host, source_id)
);

-- Each recorder's files by when they were first seen: its last file and its
-- counts of the day.
CREATE INDEX ingest_files_source_seen ON ingest_files (source_id, first_seen);
