-- Ingest: the files seen in the sources, the transcription
-- queue, the processing lease, the settings, and the evaluation sets and runs
-- of the transcription improvement module. Additive, like every migration
-- after 0001.

-- The name that the archive's folder and files still carry after the recorder
-- renamed the talkgroup. The job that renames the files clears it.
ALTER TABLE talkgroups ADD COLUMN files_name text;

-- One row per file seen in a source; rel_path is inside the source's path.
-- waiting: to be filed. done: filed as the call (call_id, call_started_at).
-- rejected: the file could not be read, and reason says why. A rejected file
-- is read again only when its size or modification time changes.
CREATE TABLE ingest_files (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source_id       smallint NOT NULL REFERENCES sources (id),
    rel_path        text NOT NULL,
    size            bigint,
    file_mtime      timestamptz,
    first_seen      timestamptz NOT NULL,
    seen_at         timestamptz NOT NULL,
    state           text NOT NULL
        CONSTRAINT ingest_files_state CHECK (state IN ('waiting', 'done', 'rejected')),
    reason          text,
    call_id         bigint,
    call_started_at timestamptz,
    UNIQUE (source_id, rel_path)
);
-- The filer takes waiting files oldest first.
CREATE INDEX ingest_files_waiting ON ingest_files (first_seen, id) WHERE state = 'waiting';

-- Calls waiting for a transcript, and calls whose transcription failed for good.
CREATE TABLE transcription_queue (
    call_id     bigint NOT NULL,
    started_at  timestamptz NOT NULL,
    queued_at   timestamptz NOT NULL,
    attempts    smallint NOT NULL DEFAULT 0,
    next_try_at timestamptz NOT NULL,
    last_error  text,
    state       text NOT NULL
        CONSTRAINT transcription_queue_state CHECK (state IN ('queued', 'failed')),
    PRIMARY KEY (call_id, started_at),
    FOREIGN KEY (call_id, started_at) REFERENCES calls (id, started_at) ON DELETE CASCADE
);
-- The transcriber takes the calls that are due, newest first.
CREATE INDEX transcription_queue_newest ON transcription_queue (started_at DESC, call_id DESC)
    WHERE state = 'queued';

-- 'processing' names the host that files and transcribes, until expires_at.
-- 'ingest:<host>' is the heartbeat of each running host. holder is
-- '<host> <pid>-<process start>', and '' after a clean release.
CREATE TABLE leases (
    name       text PRIMARY KEY,
    holder     text NOT NULL,
    expires_at timestamptz NOT NULL,
    renewed_at timestamptz NOT NULL
);

-- The administrator's settings. Secrets never go here.
CREATE TABLE settings (
    key        text PRIMARY KEY,
    value      jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- The evaluation sets of the improvement module: the calls each set holds.
CREATE TABLE eval_calls (
    set_name   text NOT NULL,
    call_id    bigint NOT NULL,
    started_at timestamptz NOT NULL,
    drawn_at   timestamptz NOT NULL,
    PRIMARY KEY (set_name, call_id, started_at),
    FOREIGN KEY (call_id, started_at) REFERENCES calls (id, started_at) ON DELETE CASCADE
);

-- One row per model comparison run.
CREATE TABLE asr_runs (
    id               bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    run_at           timestamptz NOT NULL,
    set_name         text NOT NULL,
    server           text NOT NULL,
    model            text NOT NULL,
    prompt           text,
    calls            integer NOT NULL,
    wer              real NOT NULL,
    seconds_per_call real NOT NULL
);
