-- The audit log (build step 7): one row for each change an admin made, or a
-- command run on a machine that changed something. Nobody edits or deletes
-- a row: the triggers refuse it, a super admin's session included.
CREATE TABLE audit_log (
    id      bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at      timestamptz NOT NULL DEFAULT now(),
    -- The admin's login, or "<account> on <machine>" for a command.
    actor   text NOT NULL,
    -- Where the admin's request came from; none for a command or a job.
    address text,
    what    text NOT NULL
);
CREATE INDEX audit_log_actor ON audit_log (actor, id);

CREATE FUNCTION audit_log_kept() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'the audit log is never changed or deleted';
END
$$;
CREATE TRIGGER audit_log_kept BEFORE UPDATE OR DELETE ON audit_log
    FOR EACH ROW EXECUTE FUNCTION audit_log_kept();
CREATE TRIGGER audit_log_not_truncated BEFORE TRUNCATE ON audit_log
    FOR EACH STATEMENT EXECUTE FUNCTION audit_log_kept();

-- The step of the conversion being carried out. Its session's advisory lock
-- says it runs (a step that dies frees it); the row says what it is, for
-- the refusal of a job that would change archive files meanwhile.
CREATE TABLE conversion_steps (
    id         bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    what       text NOT NULL,
    host       text NOT NULL,
    started_by text NOT NULL,
    started_at timestamptz NOT NULL DEFAULT now()
);

-- Who asked a job to stop, for the audit log's entry of its end.
ALTER TABLE jobs ADD COLUMN stopped_by text;
