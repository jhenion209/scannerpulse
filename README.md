# ScannerPulse

Records, archives and plays back trunked radio calls. It ingests the audio
files a recorder such as SDRTrunk writes, files them in a permanent archive,
indexes them in PostgreSQL, and serves a web interface to browse, search and
listen.

**Status: early development.** A release installs one machine with its
database (Simple setup); high availability, a database elsewhere and the
other advanced choices come in later versions.

## Install

On Debian 12 or 13, Ubuntu 22.04 or 24.04, Fedora, or Rocky Linux or
AlmaLinux 8 to 10, on a 64-bit PC or 64-bit ARM (a Raspberry Pi 4 or 5
included):

```
curl -fsSL https://github.com/jhenion209/scannerpulse/releases/latest/download/install.sh | sh
sudo /usr/local/bin/scannerpulse setup
```

The script checks the system and the processor, downloads the program and
its checksums from the latest release, checks the checksums' signature and
the program's checksum, and installs `/usr/local/bin/scannerpulse`. Nothing
else changes. The program is named by its full path after `sudo`, since
some systems' sudo (Rocky Linux and AlmaLinux among them) leaves
`/usr/local/bin` off its path.

Fedora, Rocky Linux and AlmaLinux are untested outside a container. On a
real machine SELinux, which they enforce, may keep PostgreSQL from archiving
its WAL into the backups folder, and setup would then stop at "Take the
first full backup".

`scannerpulse setup` asks first for **Simple** or **Advanced** (Advanced
arrives in a later version). Simple asks, explaining each first:

1. the recordings folder, where SDRTrunk (or another recorder) saves its
   files; ScannerPulse reads each new file and moves it into the archive;
2. the archive, where every call is kept as
   `<archive>/YYYY-MM-DD/<talkgroup>/<file>.mp3` (a busy county system adds
   about half a gigabyte a day; a disk of its own is best);
3. the site's time zone;
4. the first admin account (a password of 12 characters or more);
5. a transcription server and model, or "later" (the admin page sets it).

Both folders must exist, be writable and have free space, and neither may be
inside the other. Setup then shows everything it will do, and writes nothing
until you say so. It:

- installs PostgreSQL 17 from the PostgreSQL project's own repository, with
  the database and its role (the password is random, in
  `/etc/scannerpulse/pgpass`);
- makes a `scannerpulse` system account, which the services run as, and
  gives it read and write access to both folders (on a share without POSIX
  ACLs, as NFSv4 or CIFS, the share's own permissions must let it write;
  setup checks that they do);
- writes the settings file `/etc/scannerpulse/scannerpulse.env`;
- makes a certificate for the machine's name and address and serves HTTPS on
  port 443 (or the next free port, saying what holds 443), which it opens in
  firewalld when firewalld runs. Nothing listens on port 80. The browser
  warns once about the certificate, since no authority signed it;
- sets up the backups in `<archive>/.scannerpulse-backups`: PostgreSQL sends
  its WAL to pgBackRest, and every night at 03:17 `scannerpulse backup` takes
  a pgBackRest backup (full on Sundays, differential otherwise; one to two
  weeks of point-in-time recovery) and a dump of the database (the newest 14
  kept);
- starts the services `scannerpulse-web` and `scannerpulse-ingest` and the
  timer `scannerpulse-backup`, now and at boot (a rerun restarts them, so
  they read its new settings).

The site is then at `https://<this machine>/`, and the admin page at
`/admin.html`.

For an unattended install, `setup --answers FILE` reads the answers from a
file of `NAME="value"` lines, readable by its owner only:

```
RECORDINGS="/srv/recordings"
ARCHIVE="/srv/archive"
ADMIN="admin"
ADMIN_PASSWORD="a long password"
# Optional:
TIME_ZONE="America/Chicago"
TRANSCRIPTION_URL="http://192.0.2.20:8000/v1/audio/transcriptions"
TRANSCRIPTION_MODEL="Systran/faster-whisper-medium.en"
```

`PORT` is optional too: without it setup takes 443, or the next free port;
a `PORT` given must be free.

`setup --dry-run` shows the summary and each step's command and changes
nothing.

`sudo /usr/local/bin/scannerpulse uninstall` removes the services, their
units, the settings, the account and the program. It keeps the archive, the database
and the backups unless `--remove-archive`, `--remove-database` or
`--remove-backups` names them, and each named one goes only after you type
its name. Backups kept inside the archive stay too, even with
`--remove-archive`, unless `--remove-backups` is named as well.
`--remove-archive` first asks a Yes/No question (default No) naming the
archive and how many calls it holds, and only a Yes leads to typing its name.

**Watching it** from Uptime Kuma or a similar service: an HTTPS monitor on
`https://<this machine>/healthz` expecting 200 (the web interface answers
and so does the database), with certificate errors ignored while the site
uses setup's own certificate. `/health` says only that the process is up.

**Updates.** Setup does not turn on automatic updates for the system.
PostgreSQL and pgBackRest come from the PostgreSQL project's repository (and
EPEL on the Red Hat family), so the system's own updates keep them patched.
Debian's and Ubuntu's `unattended-upgrades` leaves that repository out unless
told otherwise: add `"origin=apt.postgresql.org";` to
`Unattended-Upgrade::Origins-Pattern` in a file under `/etc/apt/apt.conf.d/`.
On the Red Hat family, `dnf-automatic` covers every enabled repository,
that one included. ScannerPulse itself is one program: running the install
script again installs the latest release, and
`sudo systemctl restart scannerpulse-web scannerpulse-ingest` puts it to
work.

## Commands

`scannerpulse --help` lists them. Every command first reads the machine's
settings file, `/etc/scannerpulse/scannerpulse.env` (or the file named by
`SCANNERPULSE_SETTINGS_FILE`): `NAME="value"` lines, the variables below,
which the services also read as their systemd `EnvironmentFile`. A variable
already set in the environment wins. So far:

- `setup`, `uninstall` and `backup` are described under Install.
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
  `SCANNERPULSE_WEB_STATIC` (a folder of the frontend's built files, served
  in place of the copy built into the binary and by the same rules: a file by
  its path, a page by its name, such as `/admin` for `admin.html`, and the
  main page for any other path without an extension; the binary carries the
  page that `web/` builds, or one saying it was not built) and
  `SCANNERPULSE_HOST` (its name in the list of open pages; default: the
  host's name) and `SCANNERPULSE_OIDC_CLIENT_SECRET`. It serves HTTPS with
  the certificate and key named by `SCANNERPULSE_TLS_CERT` and
  `SCANNERPULSE_TLS_KEY` (PEM files; `scannerpulse cert make` writes a pair
  for this machine's names and addresses). Without them it serves plain HTTP,
  and only on a loopback address such as `127.0.0.1`, where a browser treats
  the page as secure: an address other machines reach is refused without a
  certificate, since the admin page's session needs HTTPS. It never writes to
  the archive and needs no lease, so several hosts can serve the same site
  behind a load balancer. Its routes:
  - `/api/calls`: the list and the archive search: filters (`talkgroups`,
    `system`, `from`, `until`, and `q`, the search box: words, `"a phrase"`,
    `OR`, `-word` or `NOT word`, `prefix*`), `order` (`newest` or `oldest`),
    `size` (10, 25, 50, 100, 250 or 500), `after` (the `next` cursor of the
    previous page), `smart=true` (SmartSort) and `exact=true`; a count stops
    at 10,000 unless it is exact, and each read of a page or a count has
    30 seconds;
  - `/api/calls/<id>`, `/api/filters`, `/api/favorites`, and
    `/api/labels/todo?set=NAME` (for admins, and for everyone when the
    labelers' list is public);
  - `/api/status`: whether maintenance and transcription are on, the
    calls waiting to be transcribed (`queue`) and an estimate of when they
    will be done (`eta_seconds`, from the machine texts of the last ten
    minutes), the number of calls (`calls`, the planner's estimate from
    100,000 on, with `calls_estimated`), the newest call's time, the
    server's time and the site's time zone;
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
    first, at most 100,000 rows, as an Excel file, each row with a link to its
    `/audio/<id>` on the address the request reached; two exports at a time;
  - `/api/live`: a websocket of new calls and transcripts (`talkgroups`, and
    `since`, the last call a reconnecting page saw);
  - `POST /api/auth/login` (`login`, `password`) signs an admin in: the
    session is a cookie (`HttpOnly`, `Secure`, `SameSite=Lax`) that ends 12
    hours after the last request and at most 7 days after signing in (an
    admin route's read that carries `X-ScannerPulse-Poll: 1`, as the admin
    page's timed reads do, does not count as a request for that limit), and
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
    whatever its use, so the group is checked again at least that often. A
    signed-in admin goes on to the admin page. The
    first sign-in creates the account; an existing
    account of the same name is never taken over. Password sign-in stays
    the way in when the provider is down, and `admin password-signin off`
    turns it off once OIDC works;
  - `/api/admin/settings` lists every setting: each stored one with the time
    it was changed, each one that a part reads but that is not stored with a
    null `value` and `updated_at`, and a stored one that no part reads with
    `known` false (it can only be removed); `PUT` or `DELETE`
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
  talkgroup whose transcription is off stays failed); `asr sets` lists the
  evaluation sets.
  The admin page's `/api/admin/talkgroups`, `PUT
  /api/admin/talkgroups/<id>/transcribe` (`on`), `POST
  /api/admin/queue/retry`, `/api/admin/asr/sets` and
  `/api/admin/asr/report?set=NAME` do the same; the last two answer only
  while the improvement module is on.
- `talkgroup rename SYSTEM/NUMBER NAME` (and `POST
  /api/admin/talkgroups/<id>/rename`, `name`) renames a talkgroup: every
  call shows the new name at once. A name another talkgroup of the system
  has is refused (merge into that one instead), and so is a name with a
  double quote or a control character. The recorder's name wins: a
  talkgroup it still sends takes that name back with its next call. The
  archive's folders, file names and tags keep the old name until an
  administrator starts their correction, so a talkgroup with calls is
  listed by `talkgroups uncorrected` (and `/api/admin/data/uncorrected`),
  with the old name its files carry. A rename that ingest sees in the
  recorder's files lists the talkgroup the same way.
- `talkgroup correct SYSTEM/NUMBER [--confirm NAME]` (and `GET`, then
  `POST /api/admin/talkgroups/<id>/correct` with `confirm`) shows how many
  files carry an old name and, once the talkgroup's name is typed exactly,
  starts a job that corrects them: each file is written into
  `<day>/<number>-<name>/`, with the talkgroup token of its file name and
  its ID3 title rewritten, read back and checked with the reader's own
  rules, and keeps its modification time; its call then names it, and the
  old file is removed by the next step, after that is committed. A file
  whose talkgroup the rewrite cannot carry (a patch's title, or a name with
  `/` or `_` in a file name without a title) is refused and stays; files in
  read-only locations stay too. The talkgroup leaves the list when none of
  its calls is left in another folder.
- `talkgroup merge SYSTEM/NUMBER --into SYSTEM/NUMBER [--confirm NAME]`
  (and `GET /api/admin/talkgroups/<id>/merge?into=<id>`, then `POST` with
  `into` and `confirm`) joins two talkgroups of one system, for example two
  entries that the number does not join. Once the name of the talkgroup
  that goes is typed exactly, a job moves its calls to the other in steps,
  then its favorites, removes it, and corrects the moved calls' files to
  the other's number and name, as `talkgroup correct` does.
  The talkgroup that goes is deleted; if the recorder still sends its
  number, ingest adds it again as a new talkgroup with the recorder's name
  (rename or remove it in the recorder's talkgroup list first). Its
  transcription switch goes with it; the other keeps its own.
- `talkgroup purge SYSTEM/NUMBER [--confirm NAME]` (and `GET`, then
  `POST /api/admin/talkgroups/<id>/purge` with `confirm`) deletes a
  talkgroup's calls, with their transcripts, labels and queue rows, once its
  name is typed exactly. Each file is first copied into the holding folder
  (setting `purge.holding_path`, which the administrator empties), with the
  archive's layout so that an import could bring it back, read back and
  compared; then the call's row goes, and the archived file is removed by
  the next step. A purge refuses to start without that setting and
  while any of the calls is in a read-only location. The holding folder is
  checked by the host that runs the job: a folder it cannot see, or one inside
  an archive location or an ingest source (or holding one), fails the job.
  Put an empty file named `.scannerpulse-holding` in the holding folder:
  a purge needs it there before every file it copies, so a share that is
  not mounted (an empty mount point) is never written to; and a step during
  which a location or the holding folder becomes another directory rolls
  back. The talkgroup itself, its transcription switch and its favorites
  stay. A stopped correction, merge
  or purge removes the files its last step replaced before it ends.
- `report traffic` (and `/api/admin/reports/traffic`, kept ten minutes on
  each host) gives the calls per five minutes of the 120 days before today,
  in the site's time zone, as weekday and weekend profiles (calls a day),
  the three quietest hours of each (for maintenance), and the period's
  totals.
- `lease` shows which host holds the lease, and each host's heartbeat.
- `read --zone ZONE FILE...` prints what ingest reads from each file.

## Building

The web interface is a Svelte app in `web/`. Build it before the binary,
which embeds `web/dist`: `cd web && npm ci && npm run build`, then
`cargo build --release`. Node is needed only to build, never to run. Without
`web/dist` the binary still builds and serves a page that says the
interface was not built.

Tests: `cargo test --workspace` (with `DATABASE_URL`), and in `web/`
`npm test` (logic) and `npm run e2e`, which drives a browser against
`scannerpulse web` over a database filled by `web/e2e/seed.sh` with
invented calls.

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or
the MIT license (`LICENSE-MIT`), at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in this
work by you, as defined in the Apache-2.0 license, shall be dual licensed as
above, without any additional terms or conditions.
