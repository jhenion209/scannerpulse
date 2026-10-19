-- Each ScannerPulse machine's web service says here that it is up, its
-- version, and whether it holds the address the two machines share; the
-- admin page's layout reads it. Additive: a new table only.
CREATE TABLE machines (
    host         text PRIMARY KEY,
    version      text NOT NULL,
    holds_shared boolean NOT NULL,
    seen_at      timestamptz NOT NULL
);
