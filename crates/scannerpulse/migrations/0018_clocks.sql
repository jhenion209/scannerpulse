-- The clock checks: each machine of the install as last checked (a
-- ScannerPulse machine by itself, the database machines and the referee by
-- the first machine's agent). The offset is the machine's clock less the
-- primary database's, in milliseconds. Additive: an older version ignores
-- the table.
CREATE TABLE clock_checks (
    host         text PRIMARY KEY,
    role         text NOT NULL
        CONSTRAINT clock_checks_role CHECK (role IN ('scannerpulse', 'database', 'referee')),
    ntp          boolean,
    synchronized boolean,
    offset_ms    bigint,
    problem      text,
    checked_at   timestamptz NOT NULL
);

-- The time a filed file's name gives (its recorder's clock), kept with its
-- row: a recorder's clock is measured from when its files are first seen
-- (the database's clock) and from the copies of one call other recorders
-- named. Rows filed before stay without one. Additive.
ALTER TABLE ingest_files ADD COLUMN named_at timestamptz;
