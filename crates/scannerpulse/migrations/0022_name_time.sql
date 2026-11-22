-- Which time a recorder writes in its file names: when the call started
-- (most recorders) or when it ended (SDRTrunk 0.6.1). A file of an 'end'
-- recorder starts at its name's time less its length. Files filed before
-- keep their times.
ALTER TABLE sources ADD COLUMN name_time text NOT NULL DEFAULT 'start'
    CONSTRAINT sources_name_time CHECK (name_time IN ('start', 'end'));
