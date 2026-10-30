-- Which model wrote a label's machine_text, so the pair (machine_text, text)
-- says which model made the mistake the person fixed. Labels saved before
-- this stay NULL: the model that wrote them is not recorded.
ALTER TABLE transcript_labels ADD COLUMN machine_model text;

-- A listener's star rating of a call's transcript, one per browser. `rater`
-- is the SHA-256 of the browser's token; the token itself is never stored.
-- `model` and `origin` are the transcript's at the time of the rating, so a
-- later edit or new transcription does not change what was judged.
CREATE TABLE transcript_ratings (
    call_id    bigint NOT NULL,
    started_at timestamptz NOT NULL,
    rater      text NOT NULL,
    stars      smallint NOT NULL CONSTRAINT transcript_ratings_stars CHECK (stars BETWEEN 1 AND 5),
    model      text,
    origin     text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (call_id, started_at, rater),
    FOREIGN KEY (call_id, started_at) REFERENCES calls (id, started_at) ON DELETE CASCADE
);
CREATE INDEX transcript_ratings_model ON transcript_ratings (model, stars);
