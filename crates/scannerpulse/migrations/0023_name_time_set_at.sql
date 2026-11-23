-- When the time in a recorder's file names was last changed: the clock checks measure only files seen since.
ALTER TABLE sources ADD COLUMN name_time_set_at timestamptz;
