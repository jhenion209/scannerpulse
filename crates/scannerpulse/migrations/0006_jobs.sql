-- The job runner: which host stepped a job last and when, and the job's own
-- working state between steps (a cursor, running totals), so that a stop or
-- a failover resumes it where it was. Additive.
ALTER TABLE jobs
    ADD COLUMN updated_at timestamptz,
    ADD COLUMN host       text,
    ADD COLUMN work       jsonb NOT NULL DEFAULT '{}';
