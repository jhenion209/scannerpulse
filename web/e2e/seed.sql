-- Invented calls for the end-to-end tests. psql variable :archive is the
-- folder the audio files are in. Every name here is made up.
SELECT ensure_month_partitions((now() AT TIME ZONE 'UTC')::date);
SELECT ensure_month_partitions(((now() - interval '3 days') AT TIME ZONE 'UTC')::date);

INSERT INTO locations (name, root, mode, is_primary) VALUES ('primary', :'archive', 'managed', true);
INSERT INTO sources (name, kind, path) VALUES ('live', 'sdrtrunk', '/nonexistent/in');
INSERT INTO systems (name) VALUES ('Metro'), ('Valley');
INSERT INTO talkgroups (system_id, number, name)
SELECT s.id, t.number, t.name
FROM (VALUES ('Metro', 101, 'Metro Fire Dispatch'), ('Metro', 102, 'Metro Police One'),
             ('Valley', 201, 'Valley Ambulance')) AS t (system, number, name)
JOIN systems s ON s.name = t.system;

-- Calls 1 to 30 in the last 90 minutes, newest first; 31 to 35 three days ago.
-- Call n's file is e2e/call-n.mp3; call 7 has no file.
INSERT INTO calls (started_at, duration_ms, talkgroup_id, source_id, location_id, rel_path, radio_id, transcript_state)
SELECT CASE WHEN n <= 30 THEN now() - n * interval '3 minutes' ELSE now() - interval '3 days' - n * interval '1 minute' END,
       2000,
       (SELECT id FROM talkgroups ORDER BY id OFFSET n % 3 LIMIT 1),
       (SELECT id FROM sources), (SELECT id FROM locations),
       'e2e/call-' || n || '.mp3', 4000 + n,
       CASE WHEN n % 2 = 1 THEN 'text' ELSE 'untried' END
FROM generate_series(1, 35) AS n
ORDER BY n DESC;

INSERT INTO transcripts (call_id, started_at, text, origin)
SELECT c.id, c.started_at,
       CASE WHEN c.rel_path = 'e2e/call-5.mp3' THEN '<b>not bold</b> & kept as typed'
            ELSE 'Engine ' || substring(c.rel_path FROM 'call-(\d+)') || ' responding to the station, a long text that goes on past one line of the list so that it has to be opened to be read in full.' END,
       'machine'
FROM calls c WHERE c.transcript_state = 'text';

INSERT INTO favorites (name) VALUES ('Fire and ambulance');
INSERT INTO favorite_talkgroups (favorite_id, talkgroup_id)
SELECT f.id, t.id FROM favorites f, talkgroups t WHERE t.number IN (101, 201);
