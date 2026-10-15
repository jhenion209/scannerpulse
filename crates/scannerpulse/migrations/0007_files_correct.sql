-- A talkgroup whose name changed while it had calls keeps its old name in its
-- archive folder, file names and tags until an administrator starts the
-- correction. `files_name` has held that old name since 0003; `files_correct`
-- (0005) is the flag the admin page lists. Additive: it only sets the flag
-- for the renames ingest recorded before the flag was kept.
UPDATE talkgroups SET files_correct = false WHERE coalesce(files_name, '') <> '';
