-- The root helper (`scannerpulse agent`) on the first ScannerPulse machine:
-- what an admin asks it to do and what it said doing it, its own beat, and
-- the newest release its daily check found. Additive: new tables only.
CREATE TABLE agent_requests (
    id           bigserial PRIMARY KEY,
    kind         text NOT NULL CHECK (kind IN ('update', 'switchover')),
    requested_by text NOT NULL,
    requested_at timestamptz NOT NULL DEFAULT now(),
    started_at   timestamptz,
    finished_at  timestamptz,
    host         text,
    ok           boolean,
    said         text NOT NULL DEFAULT ''
);
-- One request open at a time: another is refused while one waits or runs.
CREATE UNIQUE INDEX agent_requests_one_open ON agent_requests ((finished_at IS NULL))
    WHERE finished_at IS NULL;

CREATE TABLE agents (
    host    text PRIMARY KEY,
    version text NOT NULL,
    seen_at timestamptz NOT NULL
);

CREATE TABLE newest_release (
    one        boolean PRIMARY KEY DEFAULT true CHECK (one),
    version    text NOT NULL,
    notes      text NOT NULL,
    checked_at timestamptz NOT NULL
);
