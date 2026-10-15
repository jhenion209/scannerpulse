-- The transcription servers as the transcriber last saw them, for the admin
-- page: a server whose cooldown has not ended is sent no request. Additive.
CREATE TABLE transcription_servers (
    url           text PRIMARY KEY,
    cooling_until timestamptz,
    failed_rounds integer NOT NULL DEFAULT 0,
    last_error    text,
    updated_at    timestamptz NOT NULL
);
