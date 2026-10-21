-- Names change only with the administrator's approval.

-- A talkgroup name a recorder sent that differs from the stored one: a
-- proposal, until it is approved (the talkgroup is renamed and the row goes)
-- or dismissed (the row stays, marked, and that spelling is ignored).
CREATE TABLE name_proposals (
    talkgroup_id integer NOT NULL REFERENCES talkgroups (id) ON DELETE CASCADE,
    name         text NOT NULL,
    source_ids   smallint[] NOT NULL DEFAULT '{}',
    calls        integer NOT NULL DEFAULT 0,
    first_seen   timestamptz NOT NULL DEFAULT now(),
    last_seen    timestamptz NOT NULL DEFAULT now(),
    dismissed    boolean NOT NULL DEFAULT false,
    PRIMARY KEY (talkgroup_id, name)
);

-- Another spelling of a system's name, mapped to it for good.
CREATE TABLE system_spellings (
    name      text PRIMARY KEY,
    system_id integer NOT NULL REFERENCES systems (id) ON DELETE CASCADE
);

-- A file whose system name is unknown waits in the archive's staging folder,
-- `.scannerpulse-staging/<source id>/<path in its recorder's folder>`, until
-- the administrator names its system. An older version ignores these rows.
ALTER TABLE ingest_files DROP CONSTRAINT ingest_files_state;
ALTER TABLE ingest_files ADD CONSTRAINT ingest_files_state
    CHECK (state IN ('waiting', 'done', 'rejected', 'staged'));
ALTER TABLE ingest_files
    ADD COLUMN staged_system text,
    -- The file's talkgroup as it names it, `<number> <name>`, for the list.
    ADD COLUMN staged_talkgroup text;
CREATE INDEX ingest_files_staged ON ingest_files (staged_system) WHERE state = 'staged';

-- Calls filed through a mapped spelling: their files still name the
-- spelling until the correction rewrites it.
CREATE TABLE mapped_calls (
    call_id    bigint NOT NULL,
    started_at timestamptz NOT NULL,
    PRIMARY KEY (call_id, started_at),
    FOREIGN KEY (call_id, started_at) REFERENCES calls (id, started_at) ON DELETE CASCADE
);
