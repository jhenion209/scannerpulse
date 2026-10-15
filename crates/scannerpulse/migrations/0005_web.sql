-- The web service: admin accounts, their sessions and failed sign-ins, the
-- open pages, and the long jobs that ingest runs. Additive.

-- An admin signs in with a password, through the OIDC provider, or both.
-- password_hash is an Argon2id hash, or '' for an account without a password.
CREATE TABLE admins (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    login         text NOT NULL UNIQUE,
    password_hash text NOT NULL DEFAULT '',
    oidc_issuer   text,
    oidc_subject  text,
    disabled      boolean NOT NULL DEFAULT false,
    created_at    timestamptz NOT NULL DEFAULT now(),
    last_sign_in  timestamptz,
    CONSTRAINT admins_oidc_pair CHECK ((oidc_issuer IS NULL) = (oidc_subject IS NULL)),
    UNIQUE (oidc_issuer, oidc_subject)
);

-- A signed-in admin. The cookie holds the token; only its SHA-256 is kept. An
-- OIDC session ends 12 hours after signing in, so the provider's group is
-- checked again at least that often.
CREATE TABLE sessions (
    token_hash bytea PRIMARY KEY,
    admin_id   bigint NOT NULL REFERENCES admins (id) ON DELETE CASCADE,
    csrf       text NOT NULL,
    oidc       boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL,
    last_seen  timestamptz NOT NULL
);
CREATE INDEX sessions_admin ON sessions (admin_id);

-- Failed password sign-ins, by login name and by address, for the lockout.
CREATE TABLE sign_in_failures (
    kind  text NOT NULL CONSTRAINT sign_in_failures_kind CHECK (kind IN ('name', 'address')),
    value text NOT NULL,
    at    timestamptz NOT NULL
);
CREATE INDEX sign_in_failures_key ON sign_in_failures (kind, value, at);

-- Every open page (one live websocket), written by the web host that holds it.
CREATE TABLE viewers (
    id           text PRIMARY KEY,
    host         text NOT NULL,
    address      text NOT NULL,
    user_agent   text NOT NULL,
    admin        text,
    connected_at timestamptz NOT NULL,
    seen_at      timestamptz NOT NULL
);
CREATE INDEX viewers_host ON viewers (host);

-- A long job, created by the web service and run in steps by the holder of
-- the processing lease.
CREATE TABLE jobs (
    id             bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind           text NOT NULL,
    params         jsonb NOT NULL DEFAULT '{}',
    state          text NOT NULL DEFAULT 'queued'
        CONSTRAINT jobs_state CHECK (state IN ('queued', 'running', 'done', 'failed', 'stopped')),
    done           bigint NOT NULL DEFAULT 0,
    total          bigint,
    message        text NOT NULL DEFAULT '',
    started_by     text NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    started_at     timestamptz,
    finished_at    timestamptz,
    stop_requested boolean NOT NULL DEFAULT false
);
CREATE INDEX jobs_waiting ON jobs (id) WHERE state IN ('queued', 'running');

-- False while the archive's files of a renamed talkgroup still carry its old
-- name; the correction job sets it back.
ALTER TABLE talkgroups ADD COLUMN files_correct boolean NOT NULL DEFAULT true;
