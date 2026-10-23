# ScannerPulse

Records, archives and plays back trunked radio calls. It ingests the audio
files a recorder such as SDRTrunk writes, files them in a permanent archive,
indexes them in PostgreSQL, and serves a web interface to browse, search and
listen.

**Status: early development.** A release installs one machine with its
database (Simple setup), or a highly available install on several machines
(Advanced setup), and converts an install on one machine to high
availability, guided by the admin page.

## Install

On Debian 12 or 13, Ubuntu 22.04 or 24.04, Fedora, or Rocky Linux or
AlmaLinux 8 to 10, on a 64-bit PC or 64-bit ARM (a Raspberry Pi 4 or 5
included):

```
curl -fsSL https://github.com/jhenion209/scannerpulse/releases/latest/download/install.sh | sh
sudo /usr/local/bin/scannerpulse setup
```

A minimal Debian has no `curl`: `apt-get install curl` first.

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

`scannerpulse setup` asks first for **Simple** or **Advanced**. Simple asks,
explaining each first:

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

**Advanced** asks Simple's questions and, before them:

1. where the database runs: on this machine, as Simple does, or a PostgreSQL
   17 database you run (its host, port, database, role and password, and
   how the connection is protected: encrypted when the server offers it,
   always encrypted, or always encrypted with the server's certificate
   checked against its name and, if no public authority signed it, a CA
   certificate you give). Setup checks that it answers, is version 17, and
   that the role may make tables there; it then installs no PostgreSQL here
   and takes no backups, which stay yours, and `uninstall` never drops that
   database; or two database machines of its own (below);
2. the certificate: setup's own; yours (a PEM certificate with its chain and
   its key, checked to belong together and to be within their dates); or
   Let's Encrypt's, which the site gets and renews by itself, a third of the
   way before its end, for the names you give. Let's Encrypt is shown each
   name either over the web, when port 443 reaches this machine from the
   internet under that name (a router may forward 443 to the site's port;
   nothing listens on port 80), or through DNS, when the name's zone is at
   Cloudflare: give an API token with "Zone, DNS, Edit" on that zone, and the
   site writes and then removes a TXT record for each request. Until the
   first certificate arrives the site serves one setup makes;
3. the reverse proxies in front of the site, if any (`web.trusted_proxies`),
   so the visitors' own addresses are read from them;
4. a syslog server the logs also go to (`udp://host:port` or
   `tcp://host:port`, RFC 5424; `logs.syslog`), besides the journal;
5. the HTTPS port.

After Simple's questions, with the database on this machine, it asks where
the backups go (the archive's `.scannerpulse-backups`, or a folder you name
outside the recordings folder and the archive) and, for each kind, a push
address to call after each backup that succeeds: an Uptime Kuma push
monitor's, say, with an interval of a day and a little more. Push addresses
carry a token, so they are kept in the settings file, not the database, as
is the Cloudflare token. High availability comes in a later version.

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

Advanced's answers have names too, each optional:

```
DATABASE="theirs"          # or "here", the default; "theirs" needs:
DB_HOST="db.example.org"
DB_PORT="5432"
DB_NAME="scannerpulse"
DB_ROLE="scannerpulse"
DB_PASSWORD="its password"
DB_SSLMODE="require"       # prefer (the default), require or verify-full
DB_SSLROOTCERT="/root/db-ca.pem" # with verify-full, when no public CA signed it
CERT_FILE="/root/site.pem" # with KEY_FILE, or neither
KEY_FILE="/root/site.key"
LETSENCRYPT="web"          # or "dns-cloudflare", in place of CERT_FILE
LETSENCRYPT_NAMES="radio.example.org"
CLOUDFLARE_TOKEN="its token" # with dns-cloudflare
PROXIES="192.0.2.5 192.0.2.6"
SYSLOG="udp://192.0.2.30:514"
BACKUPS="/srv/backups"     # with the database here only
PUSH_PGBACKREST="https://uptime.example.org/api/push/abc123?status=up" # here only
PUSH_DUMP="https://uptime.example.org/api/push/def456?status=up"
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
`--remove-backups` removes ScannerPulse's own `pgbackrest` and `dump`
folders, and the backups' folder only when nothing else is left in it.
`--remove-archive` first asks a Yes/No question (default No) naming the
archive and how many calls it holds, and only a Yes leads to typing its name.

**Later changes.** The admin page's **This machine** shows the certificate
(its names and end, and with Let's Encrypt, how its names are proved and
its last request), the backups' folder and how old each kind's newest
backup is. Logs and reverse proxies are in its Settings. The rest needs root,
so the page shows the command, run on the machine:

- `sudo /usr/local/bin/scannerpulse tls install CERT KEY` serves your
  certificate from now on (checked first); `tls self` goes back to one the
  machine makes; `tls letsencrypt web NAME...` has the site get Let's
  Encrypt's certificate for each NAME, proved over the web, and `tls
  letsencrypt dns-cloudflare NAME...` the same proved through Cloudflare's
  DNS (it asks for the API token, or reads the first line of standard input,
  and checks that it may edit each name's zone, by writing and removing a TXT
  record there). The site keeps serving its
  certificate until the new one arrives; This machine shows the request's
  outcome and the last request. `tls install` and `tls self` turn Let's Encrypt
  off, another authority named for it included;
- `sudo /usr/local/bin/scannerpulse backups push pgbackrest|dump` asks for a
  push address and sets it, with the database on this machine (with no terminal it reads the first line of
  standard input; the address holds a token, so it is never an argument,
  which sudo would log); `... backups push dump off` removes it;
- `sudo /usr/local/bin/scannerpulse backups location FOLDER` keeps the
  backups in FOLDER from now on, starting with a full backup there; the old
  ones stay where they were until you remove them.

**Two database machines.** Advanced can put the database on two machines
of its own: the first starts as the primary, and the second keeps a live
copy as its standby. This machine (the install's first) plans them: their
names or addresses, and its own address as they reach it. It writes the plan
and a key of its own for them, installs itself, and waits. On each database
machine, run the install line and `sudo /usr/local/bin/scannerpulse setup`,
choose Advanced and then "join an existing install", and give the first
machine's address and a login there that may use sudo (the second waits
for the first). The joining machine reads its place over SSH, installs
PostgreSQL 17, lets in only this machine and the other database machine
(TLS and a password), and the second copies the first and follows it. Each
lets this machine's key run only `scannerpulse node serve` (status, stop,
start, promote, promote after, follow, and on the referee switchover; on
each, clock).
Then this machine makes the tables and starts. Its services are given both
machines (`SCANNERPULSE_DATABASE_HOSTS`) and work on the one that is the
primary, moving to the other within seconds after a change of roles; a
former primary that comes back still a primary is ignored, as the promoted
machine is on a newer timeline. The primary keeps a nightly dump in
`/var/backups/scannerpulse` (the newest 14). Uninstall on a database
machine or the referee takes off setup's own parts there and leaves
PostgreSQL and its data.

- If the primary dies, run `sudo /usr/local/bin/scannerpulse db promote` on the standby. It
  is refused while the other machine still answers as the primary, so two
  primaries never write at once.
- When the old primary is back, `sudo /usr/local/bin/scannerpulse db rejoin` there makes it
  the new primary's standby (so does a standby that has stopped receiving): rewound to where their histories split, or,
  if that fails and you agree, copied whole.
- `sudo /usr/local/bin/scannerpulse db switchover` on this machine swaps the roles while
  both are healthy and the standby is caught up: writes pause for seconds,
  and nothing is lost. The standby is promoted only once it has everything
  the stopped primary wrote; otherwise the primary starts again unchanged.

**A referee** promotes the standby by itself. With two machines, neither can
tell "the other died" from "the network between us broke", so a third,
small machine decides: pg_auto_failover's monitor, which a Raspberry Pi or a
small VM runs well. In Advanced, choose that the standby becomes the primary
"by itself", and name the referee. It joins first, the same way (the install
line, then setup's "join an existing install"), then the database machines,
which register with it through `pg_autoctl`: the second copies the first and
follows it, and a write waits until the standby has it. When the primary is
lost, the referee promotes the standby: writes pause for about a minute
while it makes sure the old primary has stopped, and the services move to
the new primary. When the old primary is back, it rejoins as the standby by
itself. `db promote` and `db rejoin` are not needed then; `db switchover`
asks the referee. While the referee is down the primary goes on, but
nothing is promoted until it returns; if the referee and the standby are
down together, writes (filing calls) pause until the referee returns (the
standby waits for it too), while reading and playback go on. Put the
referee on a different switch from the database machines where you can.

To replace a referee (a Raspberry Pi whose card failed, say), give the new
one the same name and join it as before; setup keeps the install's referee
once the database machines have joined. Then register each database
machine with it again, the primary first, then the standby, as root there:

```
runuser -u postgres -- env PATH=/usr/lib/postgresql/17/bin:/usr/bin:/bin \
  pg_autoctl disable monitor --force --pgdata /var/lib/postgresql/17/sp
runuser -u postgres -- env PATH=/usr/lib/postgresql/17/bin:/usr/bin:/bin \
  pg_autoctl enable monitor --pgdata /var/lib/postgresql/17/sp \
  'postgres://autoctl_node@ref.example.org:5432/pg_auto_failover?sslmode=require'
systemctl restart pgautofailover
```

(On the Red Hat family the programs are in `/usr/pgsql-17/bin` and the data
in `/var/lib/pgsql/17/sp`.) Joining a database machine again checks that the
referee knows it, and says these steps if it does not.

**A second ScannerPulse machine** serves the same site while this one is
down, with two database machines. Mount the archive as a network share (NFS
or SMB) at the same path on both machines, and the recordings folder the
same way, so either machine can file the recorder's calls. The share must let
the scannerpulse account write on both: setup makes that account and checks
that it can write, and says so if it cannot. In Advanced, after the database
machines, add a second ScannerPulse machine, name it, and give the address
the two share (address/prefix: a free address on the network segment
both machines are on). This
machine checks that the archive is a network share, leaves the install's
mark in it, and holds the shared address with keepalived. The second joins
last, the same way as the database machines: it checks that it mounts the
same archive (by the mark), takes this machine's settings, password file and
certificate, starts the site under its own name, and joins keepalived. When
this machine's `/healthz` fails, the shared address moves to the second
within about fifteen seconds; it comes back 30 seconds after this machine is healthy
again. The two hold a lease in the database: this machine files and
transcribes calls, and the second takes over when it stops. To add a second
machine to an install already set up, run
`sudo /usr/local/bin/scannerpulse second web2.example.org 192.0.2.50/24` here;
the database machines then join again (to let the second in), and then the
second. The same command finishes a setup that stopped after this machine's
site had started (when the second's name did not resolve here yet, say);
setup says so. The shared address is IPv4, as the site listens on IPv4.

**An install on one machine** converts to high availability without being
set up again: `sudo /usr/local/bin/scannerpulse setup`, run again on it,
plans the conversion (the same choices, and the archive's share), and the
admin page's Layout area then guides each step, the changes on that machine
made by its root helper once an admin approves them (`docs/deploying.md`).

- [docs/deploying.md](docs/deploying.md) explains the shared address, where to
  put the referee, the archive share, reverse proxies (HAProxy, nginx and Caddy,
  each tried), port forwarding to the shared address, and name resolution: each
  machine's name must resolve to its address on the others.
- With a second machine, Let's Encrypt is proved through DNS, not over the
  web, where only the machine holding the address could answer; each machine
  gets and renews its own certificate. Setup's own certificate covers the
  shared address and the second's name too. After `tls install` or `tls
  self` here, join the second again: a join runs again on a machine set up,
  and the second takes this machine's settings and certificate afresh.
  A certificate setup made before the second was added does not name the
  shared address: `tls self` here makes one that does.
- Uninstall on the second leaves the archive, the database and the backups
  to this machine, and gives up the shared address first. To replace the
  second, uninstall the old one first: while its keepalived runs, it would
  hold the address too. Then run `second` here with the new one's name.

An unattended join reads `JOIN` (the first machine's address) and, unless
the login is root, `JOIN_LOGIN` and `JOIN_SUDO_PASSWORD` from its answers
file; SSH must not ask for a password then. The first machine's answers
file sets `DATABASE="nodes"`, `DB_NODES="db1.example.org db2.example.org"`,
for a referee `FAILOVER="auto"` and `REFEREE="ref.example.org"`, for a
second ScannerPulse machine `SECOND="web2.example.org"` and
`SHARED="192.0.2.50/24"`, and, if its own address is not the one other
machines reach, `HUB`.

**Watching it** from Uptime Kuma or a similar service: an HTTPS monitor on
`https://<this machine>/healthz` expecting 200 (the web interface answers
and so does the database), with certificate errors ignored while the site
uses setup's own certificate. With two ScannerPulse machines, monitor the
shared address and each machine's own address:
[docs/deploying.md](docs/deploying.md). `/health` says only that the process is up.
For the backups, a push monitor for each kind (above), with an interval of a
day and a few hours: a backup missed or failed turns it red.

**Updates.** Setup does not turn on automatic updates for the system.
PostgreSQL and pgBackRest come from the PostgreSQL project's repository (and
EPEL on the Red Hat family), so the system's own updates keep them patched.
Debian's and Ubuntu's `unattended-upgrades` leaves that repository out unless
told otherwise: add `"origin=apt.postgresql.org";` to
`Unattended-Upgrade::Origins-Pattern` in a file under `/etc/apt/apt.conf.d/`.
On the Red Hat family, `dnf-automatic` covers every enabled repository,
that one included.

ScannerPulse itself is one program, updated by
`sudo /usr/local/bin/scannerpulse update` on the first ScannerPulse machine
(or on one machine alone). It finds the newest release and checks its
signature with the same key as the install script. It brings the database's
tables up to date, then updates the referee, the database machines, the
second ScannerPulse machine and this one, one at a time, each answering
before the next. A pair keeps serving: only while the machine holding the
shared address restarts its web (a second or two) can a request fail.
`update --check` only says what
is newest and what each machine runs.

The admin page's **Updates** area does the same from either ScannerPulse
machine. It says whether a newer release exists, with its notes, and its
"Update now" shows each step as the update runs. A root helper on the first
ScannerPulse machine (`scannerpulse agent`, which setup starts) carries the
update out, as the web service runs without root. It does only what an admin
asks there, and once a day it looks for a newer release, unless the setting
`updates.check` is `false`. The Layout area's "Switch the roles now" asks the
same helper for a switchover. An install from before 0.9.0 that is updated
with `scannerpulse update` needs `sudo /usr/local/bin/scannerpulse agent
install` once on its first machine; the install line does that itself.

From 0.7.0 or earlier, run the install line on each machine once instead,
then `sudo systemctl restart scannerpulse-web scannerpulse-ingest` on each
ScannerPulse machine. Then, on the second ScannerPulse machine, run
`sudo /usr/local/bin/scannerpulse setup` and "join an existing install"
again: that lets the first machine reach it for the next update. A program
copied in by hand needs `sudo /usr/local/bin/scannerpulse db-update` before
that restart.

**More recorders.** Setup's recordings folder is the first recorder. Add
others on the admin page's Recorders area, or with `scannerpulse recorder
add NAME FOLDER [--when-asked]`, after mounting each SDRTrunk machine's
share on every ScannerPulse machine, where the scannerpulse account can read
it and delete files (as root, the command checks that it can). A watched
recorder's files are filed as they arrive. One added `--when-asked` keeps its
files in its folder until `scannerpulse recorder bring-in NAME` (Bring in
now) takes the files the folder holds at that moment. Every file taken, and
every file that cannot be filed (no date and time in its name, no MPEG
audio, over 64 MiB), is deleted from the recorder's folder; the reason of a
rejected file stays on the Status area. `recorder pause`, `watch`, `ask` and
`remove` change one: a removed recorder's files stay in its folder, its
calls in the archive, and its name stays taken. The Recorders area shows each
recorder's files of the day and names any ScannerPulse machine that cannot
read its folder; `recorder list` prints the same. `recorder preview NAME`
(Preview) reads up to 50 files spread over a recorder's folder and says, for
each, what filing would do with it (filed, under a new talkgroup, with a name
to approve, a copy of a call another recorder filed and which copy stays,
staged, or rejected and why), with the counts first. It changes nothing: it
is the test of a new recorder.

**One call heard by two recorders is one call.** A file is a copy of a call
another recorder filed when it is of the same talkgroup, from the same radio
when both name one, and the two recordings overlap by more than a second and
by at least half the shorter one; of several such calls, the one it overlaps
most. A file's start comes from its name, to the second, so two
transmissions in a row can seem to overlap by up to a second: they stay two
calls, whether one recorder heard both or each heard one. The cost: a call
shorter than about 2 s whose copies are named a second apart stays two calls
(a duplicate kept, never a call lost). The longer recording stays, by the
length measured from the MP3's frames; at equal length, the one filed first.
A copy that is not longer is deleted from its recorder's folder and never
reaches the archive, and the recorder's count of duplicates removed goes up.
A longer copy replaces the call's recording: it is written into the archive
under the call's start (the call keeps its id, its start, its links and its
stars), the call takes the copy's recorder, length, site, radio and title,
the old recording is deleted, and a call the machine transcribed is
transcribed again (its transcript stays until the new one replaces it). A
call a person worked on (a transcript a person saved, a transcription label,
a place in an evaluation set) keeps its recording: a longer copy is deleted.
A recorder never sends two copies of one call, so a call that has a copy
from a recorder is never matched by another
file of that recorder.

**Names wait for approval.** A file that names a known talkgroup otherwise
is filed under the talkgroup, which keeps its name; the other name is a
proposal, listed under Waiting for you on the Recorders area and by
`scannerpulse names list`, with the recorders that sent it, its calls, and
when it was first and last seen. `names approve SYSTEM/NUMBER NAME` renames
the talkgroup as `talkgroup rename` does; `names dismiss SYSTEM/NUMBER NAME`
ignores that spelling from then on. The archive keeps each file as its
recorder sent it, so the talkgroup is listed by `talkgroups uncorrected`
until `talkgroup correct` runs. SDRTrunk's placeholder (`Unidentified…`) is
never proposed, and a real name replaces it at once; a talkgroup number never
seen before is added at once with the file's name.

**Unknown systems wait in staging.** A file whose system name no system has
is moved off the recorder's share into the archive's staging folder,
`<archive>/.scannerpulse-staging/<recorder id>/`, with its time kept, until
you decide. `staging list` (Waiting for you) lists each name with its files,
its recorders, when it was first and last seen, and some of its talkgroups.
`staging new-system NAME` makes it a new system. `staging map NAME SYSTEM`
makes it another spelling of SYSTEM for good: its files, and every later one,
are filed under SYSTEM, and `talkgroup correct` rewrites the system name in
their tags. `staging discard NAME [--confirm NAME]` deletes its staged files
once the name is typed. A decided name's files are filed within a few
seconds, as if they had just arrived. On a database with no system yet, the
first system name is added at once.

**Storage paths beside the archive.** New calls are always filed in the
main archive; other storage paths hold calls moved out of it, by date.
Mount the disk or share on every ScannerPulse machine at the same folder,
then add it on the admin page's Storage area or with `scannerpulse storage
add NAME FOLDER`: it must stay apart from the archive, the other storage
paths, the recorders' folders and the backups, and it gets a
`.scannerpulse-location` file, so that an empty mount point is never taken
for it. Each machine checks every storage path once a minute (can it list
it, find that file and write there, and how much room is left), and the
Storage area and `storage list` show it; a machine that cannot use one is
named on the Status area too, and its plays of the calls there fail until
the path is mounted again. `storage move FROM TO LOCATION` (Move a date
range) moves the calls of the days FROM to TO, wherever they are, into
LOCATION: it counts them and their size first, refuses a path a running
machine cannot use or without room for them and 1 GB or 2 % of its disk to
spare, and starts only once `move` is typed (`--confirm move`); the admin
page asks once more in a pop-up, since there is no undo. Each call's file is
copied into the path in the archive's layout, read back, its call then
points at the copy, and the old file is deleted; listeners notice nothing.
It runs as a job (Jobs area, `jobs show|stop ID`); a stopped move started
again takes the calls left, and moving calls back to the main archive is
another move. `storage remove NAME` removes a path that holds no call; its
folder is left as it is. It waits while the files moved out of it or
purged are still being deleted, unless no machine reaches it any more (a
disk that is gone): those files are then forgotten with it. `archive move` moves the main archive only.

**Purging older than a day.** `storage purge-before DAY` (Purge older
than…, red and apart on the Storage area) deletes every call before that
day, in the site's time zone, wherever it is: its record with its
transcripts, labels and queue row, and its file for good; nothing is kept.
It counts the calls of each location and the space their files use first,
refuses a day in the future (a mistyped year would take every call), and
starts only once the day is typed (`--confirm DAY`); the admin page
asks once more in a pop-up, since there is no undo. It runs as a job, 500
calls a step: each call's record goes first and its file is listed for the
filer, which deletes it soon after, so a stop never leaves a call without
its file. A read-only location's files stay: only their records go, and
the location keeps the day, so that a later import of it skips the files
from before it. A recorder's late file older than the day is filed as
usual: a purge is done once, not kept as a rule.

A location's file work (a move's copies, a correction's, merge's or
purge's reads and writes, and the filer's deletions) runs in a thread of its
own for a limit (60 s for a job): a share that stops answering ends the job
with its counts, and holds back neither the filing of new calls nor the
other machine.

**Importing an older archive.** `storage import NAME FOLDER` (Import an
older archive… on the Storage area) takes in an older install's folder of
recordings, such as another machine's SDRTrunk or TrunkingRecorder, where it
is: the folder, mounted on every ScannerPulse machine, becomes the read-only
location NAME, and nothing in it is ever written, renamed or deleted. Each
machine checks it by listing it; an empty folder counts as not mounted. A
large folder is listed over many steps, about 20,000 files a step; a single
folder is listed whole, within 4 minutes.

A review comes first and writes nothing (`--review`). It counts the mp3
files, and those whose names hold no date and time, which are skipped. It
reads up to 1,000 of the others, drawn at random over the folder, and
counts, at the offset the import keeps (none for a new one) and at each
whole-hour offset it tries:
- the new calls;
- the copies of calls already in the archive: the same talkgroup,
  overlapping by more than a second and by at least half the shorter, the
  same radio when both name one;
- the files of a system not named yet;
- the talkgroups the files name otherwise.

It compares the files with the archive only: the folder's own copies of a
call count there as new calls. It shows some of each. It also counts the
copies at each whole-hour offset from −2 h to +2 h, and suggests the offset
at which clearly more files are copies: the older install's clock or time
zone was off.

The import then runs as a job with that offset (`--offset ±HH:MM`). The
offset moves the times the file names give; the names stay as they are, and
a file's modification time never times a call. Each file becomes a call of
NAME, at its path in the folder, except:
- a copy of a call a recorder or another import filed is skipped, and
  never replaces its recording;
- of the folder's own copies of a call (several recorders filed into one
  folder), the longer is the call, and at equal length the one whose file
  arrived first, by its modification time, in whichever order they are
  taken; the others are skipped. A call a person worked on keeps its
  recording, as does one consolidated since;
- a file of a system not named yet waits in the folder, until the system is
  named on the Recorders area and the import runs again.

A talkgroup name a file gives otherwise waits for approval, as a recorder's
does. Imported calls are not announced to listeners, and not queued for
transcription: `scannerpulse transcribe --talkgroup … --from … --to …`
queues them. A call transcribed once is transcribed again when a longer copy
takes it over. Run again, an import takes only the files it has not taken,
with the offset it keeps, and skips the files from before a day it was
purged before.

**Consolidating an import.** `storage consolidate NAME` (Consolidate… on
the import's card), never by default, copies each call of the import into
the main archive's layout, reads it back, and points the call at the copy,
as a move does. The import's own files stay as they are. It counts first,
and starts only once `move` is typed (`--confirm move`); the admin page asks
once more in a pop-up. Once the import holds no call, `storage remove NAME`
removes it. Its register stays, so that an import of the folder again takes
none of the files it took.

## Commands

`scannerpulse --help` lists them. Every command first reads the machine's
settings file, `/etc/scannerpulse/scannerpulse.env` (or the file named by
`SCANNERPULSE_SETTINGS_FILE`): `NAME="value"` lines, the variables below,
which the services also read as their systemd `EnvironmentFile`. A variable
already set in the environment wins. So far:

- `setup`, `uninstall`, `backup`, `tls` and `backups` are described under
  Install. `backup` reads `SCANNERPULSE_BACKUPS` and the push addresses
  `SCANNERPULSE_BACKUP_PUSH_PGBACKREST` and `SCANNERPULSE_BACKUP_PUSH_DUMP`;
  `web` reads the same to show them on the admin page (whether a push
  address is set, never the address).
- `db-update` creates or updates the database schema.
- `recorder list|add|watch|ask|pause|bring-in|remove|preview` (and
  `/api/admin/recorders`, `GET /api/admin/recorders/<id>/preview`),
  `names list|approve|dismiss` and `staging list|new-system|map|discard`
  are described under Install, "More recorders". `GET /api/admin/names`
  lists what waits (`unknown`, `proposals`, their count `waiting`, and the
  `systems`); `POST /api/admin/names/approve` and `dismiss` take
  `talkgroup_id` and `name`; `POST /api/admin/staging/new-system`, `map`
  and `discard` take `system`, with `to` (map) or `confirm` (discard).
- `storage list|add|remove|move|purge-before|import|consolidate` are
  described under Install, "Storage paths beside the archive" and after it. `GET /api/admin/storage` lists the locations,
  each with its calls (counted at most ten minutes before), what each
  machine found there, and the machines whose ingest runs; `POST` adds one
  (`name`, `folder`) and `DELETE /api/admin/storage/<id>` removes one. `GET
  /api/admin/storage/move?to=&from=&until=` counts a move and says why it
  cannot start (`refused`); `POST /api/admin/storage/move` starts it with
  `to`, `from`, `until` and `confirm` (`move`). `GET
  /api/admin/storage/purge?before=` counts a purge (each location's calls,
  and whether it is read-only); `POST /api/admin/storage/purge` starts it
  with `before` and `confirm` (the day). `POST
  /api/admin/storage/import/review` starts a review (`name`, `folder`), and
  `GET /api/admin/storage/import/review/<job>` gives what it found (`done`
  once complete); `POST /api/admin/storage/import` starts the import, with
  `name`, `folder` and `offset` (`±HH:MM`, optional). `GET
  /api/admin/storage/consolidate?name=` counts a consolidation; `POST
  /api/admin/storage/consolidate` starts it with `name` and `confirm`
  (`move`).
- `db promote`, `db rejoin` and `db switchover`, as root, change the two
  database machines' roles (above); `node serve`, `node status` and `node
  backup` are a database machine's (or the referee's) own, and `join info`
  and `join done` the first machine's side of a join.
- `archive move` and `db move`, as root, move an install on one machine to a
  network share and to two database machines (`docs/deploying.md`); the
  conversion's guide has the root helper run them once an admin approves.
  Until the switch, `db move --cancel` takes the site back to its own
  database alone.
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
  for this machine's names and addresses). With `SCANNERPULSE_ACME` (`web`
  or `dns-cloudflare`) and `SCANNERPULSE_ACME_NAMES` it gets Let's Encrypt's
  certificate for those names and renews it, keeping it in
  `/var/lib/scannerpulse/acme` and serving that pair until it arrives;
  `dns-cloudflare` needs `SCANNERPULSE_CLOUDFLARE_TOKEN`, and
  `SCANNERPULSE_ACME_DIRECTORY` names another ACME authority (Let's
  Encrypt's staging one, say, or a private one, whose own certificate must
  then be in the system's store of authorities), and
  `SCANNERPULSE_CLOUDFLARE_API` a stand-in for Cloudflare's API, for a
  trial. Without them it serves plain HTTP,
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
    first sign-in creates the account, as an associate with every function
    off (see "Admin roles"); an existing
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
  - `/api/admin/machine`: this host's certificate as served (names, end,
    whether it signed itself), Let's Encrypt's names, proof and last request
    when it gives the certificate (never the token), where its database
    runs, its backups' folder, and each kind's newest backup and whether it
    pushes;
  - `/api/admin/jobs` lists the jobs, `/api/admin/jobs/<id>` shows one and
    `POST /api/admin/jobs/<id>/stop` stops it; `GET /api/admin/jobs/queue`
    (`talkgroup_id`, `from`, `to`) counts the calls a queue job would take
    and `POST` starts it; `POST /api/admin/jobs/asr-run` (`set`, `server`,
    `model`, `prompt`) starts a candidate's run;
  - `/api/admin/accounts` lists the admins, each with its role and the
    functions on, and every function there is (an associate sees only their
    own account), and adds one (`login`, `password`, `role`: `associate`
    when left out); `/api/admin/accounts/<login>/password`, `/disable` and
    `/enable` change one, `PUT /api/admin/accounts/<login>/role` (`role`)
    sets its role, and `PUT` or `DELETE`
    `/api/admin/accounts/<login>/functions/<function>` turns one of an
    associate's functions on or off, as the `admin` command does;
    `/api/auth/me` and the sign-in's answer give the admin's `role` and
    `functions`;
  - `/healthz` (the database answers within 4 seconds and is the primary)
    and `/health` (the process is up), for a load balancer.
- `admin add NAME [--associate]`, `admin password|disable|enable NAME`,
  `admin role NAME super|associate`, `admin allow|deny NAME FUNCTION...`,
  `admin list` and `admin password-signin on|off` manage the admin accounts.
  `add` and `password` ask for the password twice (at least 12 characters);
  a new password and a disabled account end that account's sessions. `add`
  makes a super admin, or with `--associate` an associate with every
  function off; `list` shows each admin's role and functions. Passwords are
  kept as Argon2id hashes.
- **Admin roles.** A super admin may change everything. An associate sees
  the whole admin page but may change only what a super admin turned on for
  them, one switch per function: `locations` (add or remove a storage
  location), `move`, `purge` (older than a day), `import`, `consolidate`,
  `rename`, `correct`, `merge`, `purge-talkgroup`, `recorders`, `names`
  (name proposals and staged systems), `update` (Update now), `switchover`,
  `convert` (each step of converting to high availability), `transcribe`
  (per talkgroup), `queue` (past calls, and retrying failed ones),
  `evaluation` (a candidate's run), `transcription-settings` (the
  `transcription.*` settings, `transcription.servers` among them: where every
  call's audio goes), `settings` (every other setting), `syslog`
  (`logs.syslog`: where the journal's lines go), `maintenance`, `favorites`,
  `stop-jobs` (another admin's job: whoever started a job may always stop
  it) and `rescan`. Only a super admin adds admins, sets roles and
  functions, sets another's password, and changes the settings that decide
  who signs in (`oidc.*`, `auth.password_signin`, `web.trusted_proxies`). A
  change is refused by the server (403), whatever the page shows; a role or
  a function changed counts from the admin's next request. The last enabled
  super admin can be neither disabled nor made an associate. An admin made
  before roles existed is a super admin.
- **The audit log.** Every change an admin makes is kept, naming the admin
  and the address the request came from:
  - signing in and out (a failed sign-in goes to the journal only);
  - a job started, and how it ended (done, failed, or stopped, and by whom);
  - accounts, roles, functions and passwords set (never the password);
  - a setting, with its old and new values;
  - locations, recorders, name and system decisions, transcription switches
    and favorites;
  - Update now, a switchover and each step of the conversion.

  A command run on a machine that changes something is kept as typed, named
  `<account> on <machine>` (a job it starts is started by that name too).
  Each entry is also a line in the journal (`audit: …`). The database
  refuses to change or delete an entry, a super admin's included, and keeps
  them with no time limit. `audit [--since DAY] [--admin NAME] [--follow]`
  prints them oldest first, each time in the site's zone; `--admin root`
  takes root's commands on every machine, and `--follow` keeps printing new
  entries as they come. The admin page does not show the log.
- **A step of the conversion and the jobs that change archive files exclude
  each other.** The jobs are a move, purge older than, import, consolidate,
  and a talkgroup's correct, merge and purge. The steps are the archive's
  check and switch, the database's move, switch and removal, and the second
  machine's addition, asked of the root helper or run as a command.
  Whichever is started second is refused, naming what runs and who started
  it; a job waiting its turn counts. This holds only while a step is carried
  out, not between steps, and for every admin and command alike. Update now
  and a switchover are not steps. A step that cannot reach the database
  goes on unchecked, and says so.
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
  Its `clocks` list names each clock to look at, as do the Layout area's
  machines and the Recorders area's cards; nothing sets a clock. Every 10
  minutes each ScannerPulse machine checks its own (its time service, by
  `timedatectl`, and how far it is from the primary database's clock), and
  the first machine's helper asks each database machine and the referee the
  same over their restricted key. A machine is named when its clock is not
  synchronized (its time service off or not: a container's is its host's,
  so off alone is not named), is a second or more from the database's,
  cannot be checked, or was last checked over 30 minutes ago; one that
  left the install is forgotten (a ScannerPulse machine after a week, as
  in the Layout area). A recorder is measured from its newest 200 files of the last day (5 at least; never an
  import; a recorder taken in when asked only by its copies). A file is
  seen once it has been still for 2 s after its call ended; counting from
  then, a recorder is named when one in ten of its files is seen more than
  a second early (its clock is ahead), or nine in ten more than a minute
  late (behind, an hour without daylight saving say, or slow to arrive): a
  rescan, an outage or a backlog only ever adds delay. It is named too when
  its copies of another recorder's calls are named a second or more apart
  from them, which can keep one short call as two.
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
  double quote or a control character. The rename lasts: a recorder that
  still sends another name makes it a proposal (`names list`). The
  archive's folders, file names and tags keep the old name until an
  administrator starts their correction, so a talkgroup with calls is
  listed by `talkgroups uncorrected` (and `/api/admin/data/uncorrected`),
  with the old name its files carry. A file filed under a talkgroup whose
  name it spells otherwise, or through a mapped system name, lists the
  talkgroup the same way.
- `talkgroup correct SYSTEM/NUMBER [--confirm NAME]` (and `GET`, then
  `POST /api/admin/talkgroups/<id>/correct` with `confirm`) shows how many
  files carry an old name and, once the talkgroup's name is typed exactly,
  starts a job that corrects them: each file is written into
  `<day>/<number>-<name>/`, with the talkgroup token of its file name and
  its ID3 title rewritten (and, for a call filed through a mapped system
  name, the system name in its tag, in place when its folder is right),
  read back and checked with the reader's own rules, and keeps its
  modification time; its call then names it, and the
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
  the next step. A purge refuses to start without that setting. A call in
  a read-only location loses only its record; its file stays where it is,
  and the count says how many calls this is. The holding folder is
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
