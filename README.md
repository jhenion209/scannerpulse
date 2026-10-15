# ScannerPulse

Records, archives and plays back trunked radio calls. It ingests the audio
files a recorder such as SDRTrunk writes, files them in a permanent archive,
indexes them in PostgreSQL, and serves a web interface to browse, search and
listen.

**Status: early development. There is nothing to install yet.**

## Commands

`scannerpulse --help` lists them. So far:

- `db-update` creates or updates the database schema.
- `ingest` watches the source folders and files each new call into the
  archive. Two hosts can run it side by side: a lease in the database lets one
  of them work at a time, and the other takes over when it stops. It reads
  `DATABASE_URL`, `SCANNERPULSE_HOST` (default: the host's name) and
  `SCANNERPULSE_PREFERRED` (`true` on the host that should work when both run).
  The rest of its configuration is in the database: the `time_zone` setting,
  the primary archive location and the enabled sources. Until a web interface
  exists, these are added with SQL.
  When transcription is on, ingest also sends each new call of a talkgroup
  that has transcription on, and at least the minimum length, to an
  OpenAI-compatible transcription server (`/v1/audio/transcriptions`), newest
  calls first. The `transcription.*` settings give the servers (a URL and how
  many requests each takes at once), the model, the minimum length, and an
  optional language, prompt and term list. A server that needs a key reads it
  from `SCANNERPULSE_TRANSCRIPTION_API_KEY`; keys never go in the database.
  To turn it on, for example:

  ```sql
  INSERT INTO settings (key, value) VALUES
    ('transcription.enabled', 'true'),
    ('transcription.servers',
     '[{"url": "http://HOST:8000/v1/audio/transcriptions", "weight": 2}]'),
    ('transcription.model', '"model-name"'),
    ('transcription.min_duration_ms', '1500')
  ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now();
  ```

  Each talkgroup has its own switch, `talkgroups.transcribe` (off for a new
  talkgroup): `UPDATE talkgroups SET transcribe = true WHERE ...`.
- `transcribe --talkgroup SYSTEM/NUMBER --from DAY --to DAY [--yes]` queues the
  past calls of one talkgroup for transcription.
- `asr sample|todo|report|run` measure transcription against the text people
  saved, when the setting `improvement_module.enabled` is true:
  - `asr sample --set NAME --talkgroups FILE --calls N --days N
    [--min-seconds S] [--seed N] [--yes]` draws an evaluation set from the
    talkgroups listed in FILE (one `SYSTEM/NUMBER` a line), as evenly as their
    calls allow across the talkgroups and the months of the last N days. Only
    calls the machine has transcribed are drawn. A set that people have
    labeled is never replaced, so its scores stay comparable.
  - `asr todo --set NAME` lists the set's calls that have no person's text yet.
  - `asr report --set NAME` gives the word error rate of production's first
    output against the people's texts, over all the calls together, and how
    many calls labeled silent got text; then the set's kept runs.
  - `asr run --set NAME --server URL --model MODEL [--prompt-file FILE]`
    transcribes the set's labeled calls with a candidate, one at a time,
    without saving any transcript, and keeps its word error rate and seconds
    per call beside production's. It stops when the server fails three calls
    in a row.
- `web` serves the web interface and its API. It reads `DATABASE_URL`,
  `SCANNERPULSE_WEB_LISTEN` (default `127.0.0.1:8080`) and
  `SCANNERPULSE_WEB_STATIC` (the frontend's built files). It never writes to
  the archive and needs no lease, so several hosts can serve the same site
  behind a load balancer. Its routes:
  - `/api/calls`: the list and the archive search: filters (`talkgroups`,
    `system`, `from`, `until`, and `q`, the search box: words, `"a phrase"`,
    `OR`, `-word` or `NOT word`, `prefix*`), `order` (`newest` or `oldest`),
    `size` (10, 25, 50, 100, 250 or 500), `after` (the `next` cursor of the
    previous page), `smart=true` (SmartSort) and `exact=true`; a count stops
    at 10,000 unless it is exact, and each read of a page or a count has
    30 seconds;
  - `/api/calls/<id>`, `/api/filters`, `/api/favorites`, `/api/status`, and
    `/api/labels/todo?set=NAME` when the labelers' list is public;
  - `/audio/<id>`: a call's file, with byte ranges;
  - `POST /api/export/audio`: up to 200 calls, by `ids` or by the filters,
    joined into one mp3 of at most 256 MiB of audio; `/api/export/xlsx`: a search's matches, newest
    first, at most 100,000 rows, as an Excel file; two exports at a time;
  - `/api/live`: a websocket of new calls and transcripts (`talkgroups`, and
    `since`, the last call a reconnecting page saw);
  - `/healthz` (the database answers within 4 seconds and is the primary)
    and `/health` (the process is up), for a load balancer.
- `admin add|password|disable|enable NAME`, `admin list` and
  `admin password-signin on|off` manage the admin accounts. `add` and
  `password` ask for the password twice (at least 12 characters); a new
  password and a disabled account end that account's sessions; the last
  enabled account cannot be disabled. Passwords are kept as Argon2id hashes.
- `lease` shows which host holds the lease, and each host's heartbeat.
- `read --zone ZONE FILE...` prints what ingest reads from each file.

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or
the MIT license (`LICENSE-MIT`), at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in this
work by you, as defined in the Apache-2.0 license, shall be dual licensed as
above, without any additional terms or conditions.
