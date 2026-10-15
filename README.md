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
  `SCANNERPULSE_WEB_LISTEN` (default `127.0.0.1:8080`),
  `SCANNERPULSE_WEB_STATIC` (the frontend's built files) and
  `SCANNERPULSE_HOST` (its name in the list of open pages; default: the
  host's name) and `SCANNERPULSE_OIDC_CLIENT_SECRET`. It never writes to
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
    `/api/labels/todo?set=NAME` (for admins, and for everyone when the
    labelers' list is public);
  - `PATCH /api/calls/<id>/transcript` (`text`): anyone may correct a
    transcript; the machine's first output is kept beside it, with the
    admin's name when an admin saves it (only when the request carries the
    `X-CSRF-Token` header; without it the save is a visitor's). A text has at most 4,000 characters
    and no `<`, `>` or control character other than a new line; one address
    may save 30 times a minute per host; saving pauses while the setting
    `maintenance.enabled` is true;
  - `/audio/<id>`: a call's file, with byte ranges;
  - `POST /api/export/audio`: up to 200 calls, by `ids` or by the filters,
    joined into one mp3 of at most 256 MiB of audio; `/api/export/xlsx`: a search's matches, newest
    first, at most 100,000 rows, as an Excel file; two exports at a time;
  - `/api/live`: a websocket of new calls and transcripts (`talkgroups`, and
    `since`, the last call a reconnecting page saw);
  - `POST /api/auth/login` (`login`, `password`) signs an admin in: the
    session is a cookie (`HttpOnly`, `Secure`, `SameSite=Lax`) that ends 12
    hours after the last request and at most 7 days after signing in, and
    every admin change must carry the session's CSRF token in an
    `X-CSRF-Token` header (`/api/auth/me` gives it). Five failed sign-ins for
    one name, or from one address, within 15 minutes lock that name or that
    address for 15 minutes. `POST /api/auth/logout` signs out;
    `/api/auth/methods` says which ways to sign in are on. Behind a proxy, list
    its addresses in the setting `web.trusted_proxies` (for example
    `["192.0.2.10"]`): only then is its `X-Forwarded-For` believed;
  - `/api/auth/oidc` signs an admin in through an OpenID Connect provider
    (the authorization code flow with PKCE), when the settings `oidc.issuer`,
    `oidc.client_id`, `oidc.admin_group` and `oidc.redirect_url` (this
    site's `/api/auth/oidc/callback`, as registered with the provider) are
    set and the host has `SCANNERPULSE_OIDC_CLIENT_SECRET`; the secret never
    goes in the database. The ID token's signature, issuer, audience, expiry
    and nonce are checked, and its `groups` claim must hold the admin group
    at every sign-in; an OIDC session ends 12 hours after signing in,
    whatever its use, so the group is checked again at least that often. The
    first sign-in creates the account; an existing
    account of the same name is never taken over. Password sign-in stays
    the way in when the provider is down, and `admin password-signin off`
    turns it off once OIDC works;
  - `/api/admin/settings` lists the settings, and `PUT` or `DELETE`
    `/api/admin/settings/<key>` (`value`) changes one, as `settings` does;
  - `PUT /api/admin/favorites/<name>` (`talkgroups`, a list of talkgroup
    ids) creates a favorite list or replaces its talkgroups, and `DELETE`
    removes it;
  - `/api/admin/viewers`: every open page of every host (its address,
    whether that address is local, its browser and whether an admin opened
    it), refreshed every 30 seconds; a page's row goes when it closes;
  - `/api/admin/jobs` lists the jobs, `/api/admin/jobs/<id>` shows one and
    `POST /api/admin/jobs/<id>/stop` stops it; `GET /api/admin/jobs/queue`
    (`talkgroup_id`, `from`, `to`) counts the calls a queue job would take
    and `POST` starts it; `POST /api/admin/jobs/asr-run` (`set`, `server`,
    `model`, `prompt`) starts a candidate's run;
  - `/api/admin/accounts` lists and adds admins;
    `/api/admin/accounts/<login>/password`, `/disable` and `/enable` change
    one, as the `admin` command does;
  - `/healthz` (the database answers within 4 seconds and is the primary)
    and `/health` (the process is up), for a load balancer.
- `admin add|password|disable|enable NAME`, `admin list` and
  `admin password-signin on|off` manage the admin accounts. `add` and
  `password` ask for the password twice (at least 12 characters); a new
  password and a disabled account end that account's sessions; the last
  enabled account cannot be disabled. Passwords are kept as Argon2id hashes.
- `settings list`, `settings set KEY JSON` and `settings unset KEY` show and
  change the administrator's settings, such as `settings set time_zone
  '"America/Chicago"'`. A change is checked as the part that reads the
  setting checks it, so a bad value is refused before it is saved; a name no
  part reads is refused too. The admin page's `/api/admin/settings` does the
  same.
- `favorites list`, `favorites set NAME SYSTEM/NUMBER...` and
  `favorites remove NAME` manage the favorite lists; `viewers` lists the
  open pages.
- `jobs list`, `jobs show ID` and `jobs stop ID` follow and stop the long
  jobs, which the host holding the lease runs one at a time, a step at a
  time: a stop takes effect between steps, and after a failover the other
  host resumes a job where it was. `jobs queue [--talkgroup SYSTEM/NUMBER]
  [--from DAY] [--to DAY] [--yes]` queues past calls for transcription (one
  talkgroup's, or the backlog of every talkgroup whose transcription is on),
  and `jobs asr-run --set NAME --server URL --model MODEL [--prompt-file
  FILE]` is `asr run` as a job.
- While the setting `maintenance.enabled` is true, ingest's filing,
  transcription and jobs pause on every host (requests under way finish) and
  the lease is kept, so the archive or a source can be serviced; the public
  edit answers 503. `rescan` (or `POST /api/admin/rescan`) asks every
  running ingest to list its sources at once.
- `status` (and `/api/admin/status`) gives the admin page's Status tab as
  JSON: the lease and each host's heartbeat, ingest's waiting and rejected
  files, the transcription queue and its failed calls, the servers, the
  newest calls, estimated totals, whether the database is the primary and
  its standbys, and the checks to make after a failover (stuck jobs, failed
  calls, calls filed twice, files left in a source). It names the server it
  reached and shows the maintenance switch. The database already refuses a
  second filing of a file within a month, so "calls filed twice" watches the
  month edge.
  It is read in one snapshot, and each of its queries gives up after 10
  seconds rather than wait on a locked or slow table.
- `talkgroups` lists the talkgroups and their transcription switches;
  `talkgroup transcribe SYSTEM/NUMBER on|off` sets one (new calls only:
  `jobs queue --talkgroup` queues the past ones); `queue retry-failed` puts
  every failed call back in the queue in one statement (a call of a
  talkgroup whose transcription is off stays failed); `asr sets` lists the evaluation sets.
  The admin page's `/api/admin/talkgroups`, `PUT
  /api/admin/talkgroups/<id>/transcribe` (`on`), `POST
  /api/admin/queue/retry`, `/api/admin/asr/sets` and
  `/api/admin/asr/report?set=NAME` do the same; the last two answer only
  while the improvement module is on.
- `report traffic` (and `/api/admin/reports/traffic`, kept ten minutes on
  each host) gives the calls per five minutes of the 120 days before today,
  in the site's time zone, as weekday and weekend profiles (calls a day),
  the three quietest hours of each (for maintenance), and the period's
  totals.
- `lease` shows which host holds the lease, and each host's heartbeat.
- `read --zone ZONE FILE...` prints what ingest reads from each file.

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or
the MIT license (`LICENSE-MIT`), at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in this
work by you, as defined in the Apache-2.0 license, shall be dual licensed as
above, without any additional terms or conditions.
