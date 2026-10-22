-- Storage locations beside the main archive. What each host's ingest last
-- found in each location: whether it could read and write it (a location
-- an administrator added must hold its marker file, so that an unmounted
-- share's empty mount point is never taken for it), and the space on its
-- disk. A move into a location waits until every running host can use it.
CREATE TABLE location_health (
    host        text NOT NULL,
    location_id smallint NOT NULL REFERENCES locations (id),
    ok          boolean NOT NULL,
    problem     text,
    free_bytes  bigint,
    total_bytes bigint,
    checked_at  timestamptz NOT NULL,
    PRIMARY KEY (host, location_id)
);
