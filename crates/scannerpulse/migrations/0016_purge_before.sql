-- Purging older than a date: a read-only location (an import) keeps the day
-- it was purged before, so that a later run of its import skips the files
-- older than that day. Additive: an older version ignores the column.
ALTER TABLE locations ADD COLUMN purged_before date;
