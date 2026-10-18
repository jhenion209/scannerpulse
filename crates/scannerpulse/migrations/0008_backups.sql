-- When each kind of backup last succeeded, written by `scannerpulse backup`
-- and shown on the admin page. Additive: a new table only.
CREATE TABLE backups (
    kind text PRIMARY KEY CHECK (kind IN ('pgbackrest', 'dump')),
    finished_at timestamptz NOT NULL
);
