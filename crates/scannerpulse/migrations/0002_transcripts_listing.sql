-- A search shows its newest matches first. With this index the database reads
-- the newest month first and stops when the page is full. Without it, it has
-- to collect every match of every month before it can sort them.
CREATE INDEX transcripts_listing ON transcripts (started_at DESC, call_id DESC);
