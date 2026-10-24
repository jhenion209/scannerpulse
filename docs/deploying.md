# Deploying ScannerPulse

This guide is for an install that keeps working when one machine is down. It
explains each part, where to put it, how visitors reach the site, and how to
watch it. The README covers setup itself. One machine alone needs none of this:
setup's Simple path is enough.

## The parts

| Part | What it does | Setup |
|---|---|---|
| The first ScannerPulse machine | Serves the site; holds the plan; files and transcribes calls | `scannerpulse setup`, Advanced |
| Two database machines | A primary, and a standby that keeps a live copy | the install line, then setup's "join an existing install" |
| The referee (optional) | A third, small machine that promotes the standby by itself | the same, first |
| The second ScannerPulse machine (optional) | Serves the same site while the first is down | the same, last |
| The archive share | One NFS or SMB share that both ScannerPulse machines mount | yours, before setup |
| The shared address (optional) | One address that the healthy ScannerPulse machine holds | named on the first machine |

All of them must be able to reach each other by name. Each machine's name must
resolve to its address on the others, through your DNS or `/etc/hosts`. The
first machine must resolve the second's name before the second exists, since it
writes the shared address's settings for it.

## The shared address (VRRP and keepalived)

Two ScannerPulse machines can share one address, such as `192.0.2.50`.
Bookmarks, phones, the port forward and the site's DNS record all point at it.
Only one machine holds it at a time. keepalived, which setup installs and
configures on both, decides which one:

- Each machine checks its own site's `/healthz` every 5 seconds. That page
  answers 200 when the site and its database work.
- The first machine has the higher priority, so it holds the address while it
  is healthy. Two failed checks in a row lower its priority below the second's,
  and the second takes the address. That takes about 15 seconds after the
  first's site stops, and a few seconds after the machine itself goes away.
- When the first machine is healthy again, it takes the address back after 30
  to 45 seconds.
- If both machines fail their checks, the first keeps the address.

The two machines tell each other their state with VRRP. They send it straight
to each other (unicast), not to the whole network. Setup lets VRRP (IP protocol
112) through firewalld when firewalld runs. If another firewall sits between
the two machines, let VRRP through it too.

Both machines and the shared address must be on the same network segment, so
that the address can move between them. The address must be a free one there:
not used by any other machine, and outside your DHCP server's range.

The adverts carry no password, so any machine on the segment could claim the
address. Keep the two machines on a network you trust.

Do not use DNS round robin (two A records) instead. Browsers keep trying the
address of a machine that is down, and visitors see errors until the record
expires.

## The referee, and where to put it

With two database machines, neither can tell "the other machine died" from
"the network between us broke". A third machine, the referee
(pg_auto_failover's monitor), decides. A Raspberry Pi or a small VM is enough.

Put the referee where it is least likely to fail together with a database
machine:

- not on the same physical host as either database machine;
- on a different switch or power circuit where you can;
- on the same network, so that it reaches both database machines quickly.

While the referee is down, the primary carries on, but nothing is promoted
until the referee returns. If the referee and the standby are down at the same
time, writes (filing calls) wait for the referee; reading and playback carry
on. To replace a referee, give the new one the same name and join it; the README
gives the steps that register the database machines with it again.

Without a referee, you promote the standby by hand:
`sudo /usr/local/bin/scannerpulse db promote` on the standby.

## The archive share

Both ScannerPulse machines read and write the same archive, so the archive
must be a network share (NFS or SMB). Both machines mount it at the same path.
Setup checks this, and the first machine leaves a mark in the share that the
second checks before it joins. Mount the recordings folder the same way, so
that either machine can file the recorder's calls.

The scannerpulse account must be able to write on the share from both
machines. Setup checks this too, and stops with a message if it cannot.

**An archive already on the machine's own disk** moves to the share in three
steps, with the site up throughout:
1. `sudo /usr/local/bin/scannerpulse archive move /srv/shared/archive` compares
   the two and gives the copy to run (rsync, as the scannerpulse account).
2. Run that copy, then the first command again, until it finds nothing
   missing, or only calls filed since the copy (at most 1,000), which the
   switch copies.
3. `sudo /usr/local/bin/scannerpulse archive move /srv/shared/archive --switch`
   pauses filing (the recordings wait in their folder), copies the calls filed
   since your copy, checks them, and points the site at the share. Ctrl-C or
   a closed terminal waits for it to finish, and filing starts again however
   it ends.

The share's folder may not be the recordings folder, nor sit inside it or
hold it, nor be the backups' folder or sit inside it: the command refuses it.

The old archive stays as it was until you remove it; the same command the
other way moves back. The backups' folder (`.scannerpulse-backups`) is not
copied: `scannerpulse backups location FOLDER` moves the backups.

- **NFS:** give the scannerpulse account the same user number on both
  machines, or have the server map every user to one owner (for example
  `all_squash` with `anonuid` and `anongid`). Setup writes the mark as the
  scannerpulse account, never as root, so root squashing does not get in the
  way. The second reads the mark as root, so the archive's top folder must be
  readable by others.
- **SMB:** mount the share so that the scannerpulse account may write, for
  example with `uid=scannerpulse,gid=scannerpulse` among the mount options. The
  account exists only once setup has made it: setup stops at its check, you
  remount, and you run setup again.

## The database moved to database machines

An install set up Simple keeps its database on its own machine. It moves to
two database machines of its own, the standby promoted by hand or by a
referee, with the site up except for a few seconds:
1. On this machine, `sudo /usr/local/bin/scannerpulse db move DB1 DB2 --hub
   ADDRESS`, ADDRESS being this machine's address as the database machines
   reach it. It opens this machine's PostgreSQL to them (one restart, a few
   seconds), has the site name both this database and DB1, so that it
   follows DB1 once it is the primary, and says what to run next.
2. On DB1, run the install line, then `sudo /usr/local/bin/scannerpulse setup`,
   choose Advanced, then "join an existing install", and give ADDRESS. DB1
   copies this machine's database and follows it. The first command, run
   again, says how far behind it is.
3. `sudo /usr/local/bin/scannerpulse db move --switch` stops this machine's
   PostgreSQL cleanly, makes DB1 the primary once it has everything, and
   points the site at both machines. Writes pause for the seconds it takes;
   Ctrl-C or a closed terminal waits for it to finish.
4. Join DB2 the same way: it becomes DB1's standby.
5. Once you are satisfied, `sudo /usr/local/bin/scannerpulse db move --retire`
   removes this machine's old database. Its backups stay in the backups'
   folder until you remove them; each database machine takes a nightly dump.

**With a referee** (`--referee REF` after ADDRESS, and `--referee-port PORT`
when another PostgreSQL holds 5432 on the referee's machine), the referee
joins first, the same way. The first command, run again exactly as given
(the port too: run without it, it is refused), then makes this machine's
database the referee's first primary: pg_autoctl runs it from then on (one
restart, a few seconds). DB1 joins as its standby. `--switch` asks the
referee to make DB1 the primary; this machine's database follows it as a
standby the referee never promotes, until `--retire` takes it out of the
referee's watch and removes it.

**Going back before the switch:** `sudo /usr/local/bin/scannerpulse db move
--cancel` on this machine, until `--switch` has begun, takes the site back to
this machine's own database alone, with writes paused for a few seconds. By
hand, it closes this machine's database to the database machines again; with
a referee, this machine's database leaves the referee's formation, its data
kept, and PostgreSQL's own service runs it again with its own settings; the
referee's formation is emptied, so that it may serve a new move. Then run
`sudo /usr/local/bin/scannerpulse uninstall` on DB1 if it joined: its copy
is cut off and is no longer the site's database. The conversion's plan
stays, for setup to change or drop. The
archive goes back the same way it came: `archive move` with the old folder
and `--switch`.

## Converting an install on one machine, guided

An install set up Simple becomes highly available without being set up
again, with the site up except for the pauses each step names:
1. On it, `sudo /usr/local/bin/scannerpulse setup` asks the choices a new
   install's Advanced setup asks: the two database machines, a referee or a
   standby promoted by hand, a second ScannerPulse machine and the address the
   two share, and, with a second machine, the archive's new folder on a
   network share mounted here. It saves them in
   `/etc/scannerpulse/convert.json` and changes nothing else. Run it again to
   change or drop the plan before the database starts to move.
2. The admin page's Layout area then shows the steps in order, and opens the
   next only once the one before is checked: the archive's copy and switch,
   the database's move, the second machine, and a final check (both machines
   answer, the standby is in step, the referee answers), with a test
   switchover on request.
3. A step on another machine says what to run there: the install line, then
   setup's "join an existing install". A change on this machine waits for an
   admin's approval in the admin page; the root helper here then makes it,
   and the page shows each line it says.

The commands above (`archive move`, `db move`) and `second` do the same steps
by hand, and the guide follows them either way. Until the switch, `db move
--cancel` goes back (above).

## Reverse proxies

A reverse proxy is an alternative to the shared address. Use one if a proxy
already sits in front of your sites, or if your two ScannerPulse machines are
not on the same segment. The proxy sends each request to a machine whose
`/healthz` answers.

ScannerPulse serves HTTPS only, so the proxy talks HTTPS to it. With the
certificate setup made, the proxy cannot check it (it is self-signed); with a
certificate from an authority, turn that check on. Live pages hold a websocket
open, so give the proxy a long timeout for them.

Then, on the admin page, set **Settings → web.trusted_proxies** to the proxy's
address. ScannerPulse then reads each visitor's own address from the proxy's
`X-Forwarded-For`. Without it, every visitor appears to come from the proxy,
and one visitor's failed sign-ins lock everyone out.

In the examples below, the two ScannerPulse machines are `192.0.2.11` and
`192.0.2.12`, serving on port 443. The proxy's own certificate is
`/etc/ssl/sp/site.crt` and its key `/etc/ssl/sp/site.key`; for HAProxy the two
are in one file, `/etc/ssl/sp/site.pem`. HAProxy, nginx and Caddy were each
tried this way, with one machine's web stopped and with the machine cut off
from the network: the site kept answering. Nginx Proxy Manager was not tried.

### HAProxy

Each machine is checked at `/healthz` every 5 seconds. A request to a machine
that has just stopped is retried on the other, so visitors see no error while
the checks catch up.

```
frontend scannerpulse
    mode http
    bind :443 ssl crt /etc/ssl/sp/site.pem
    option forwardfor
    default_backend scannerpulse

backend scannerpulse
    mode http
    balance roundrobin
    # Live pages hold a websocket open.
    timeout tunnel 1h
    # A machine that stopped is retried on the other before its check fails.
    option redispatch
    retries 2
    timeout connect 3s
    # A new connection for each request: one kept open to a machine that
    # has vanished would hold a request until the server timeout.
    http-reuse never
    retry-on conn-failure empty-response response-timeout
    option httpchk GET /healthz
    http-check expect status 200
    server web1 192.0.2.11:443 ssl verify none check inter 5s fall 2 rise 2
    server web2 192.0.2.12:443 ssl verify none check inter 5s fall 2 rise 2
```

### nginx

Open-source nginx does not check `/healthz` by itself. It sets a machine aside
when connecting to it fails (`max_fails`), and it tries the other machine for
that request (`proxy_next_upstream`). Without `proxy_connect_timeout`, a
request sent to a machine that has vanished waits up to a minute before nginx
tries the other.

```
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}

upstream scannerpulse {
    # Shared by all of nginx's workers, so each does not learn alone that a
    # machine is down.
    zone scannerpulse 64k;
    server 192.0.2.11:443 max_fails=2 fail_timeout=10s;
    server 192.0.2.12:443 max_fails=2 fail_timeout=10s;
}

server {
    listen 443 ssl;
    ssl_certificate     /etc/ssl/sp/site.crt;
    ssl_certificate_key /etc/ssl/sp/site.key;
    client_max_body_size 16m;

    location / {
        proxy_pass https://scannerpulse;
        proxy_ssl_verify off;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_connect_timeout 3s;
        proxy_read_timeout 1h;
        proxy_next_upstream error timeout http_502 http_503;
    }
}
```

### Caddy

Caddy checks each machine at `/healthz` and adds `X-Forwarded-For` itself.
With a name that Let's Encrypt can prove for Caddy, put the name in place of
`:443` and drop the `tls` line. `lb_try_duration` lets Caddy try the other
machine when one does not answer, and `keepalive off` gives each request a new
connection, so that none is held by a machine that has vanished.

```
:443 {
	tls /etc/ssl/sp/site.crt /etc/ssl/sp/site.key
	reverse_proxy 192.0.2.11:443 192.0.2.12:443 {
		lb_policy round_robin
		lb_try_duration 5s
		health_uri /healthz
		health_interval 5s
		transport http {
			tls_insecure_skip_verify
			keepalive off
		}
	}
}
```

### Nginx Proxy Manager (not tried)

Add a proxy host:

- **Details:** the site's name; scheme `https`; forward to one machine's
  address and port 443; "Websockets Support" on.
- **SSL:** the site's certificate.
- **Advanced:** to use both machines, a custom location `/` with the nginx
  settings above (an `upstream` block cannot be added there; put it in a file
  under `/data/nginx/custom/` as the manager's documentation describes).

## Port forwarding

If the site is reached from the internet, forward port 443 (or the port setup
chose) to the shared address or to the proxy, never to one machine's own
address. A forward to one machine is down whenever that machine is.

## Watching it

From Uptime Kuma or a similar service:

- an HTTPS monitor on the site's address (the shared address or the proxy) at
  `/healthz`, expecting 200;
- an HTTPS monitor on each ScannerPulse machine's own address at `/healthz`.
  A failover keeps the site up, but this monitor still shows the machine that
  is down;
- `/health` when you want "the process is up" alone;
- a TCP monitor on each database machine's port 5432, and one on the referee's
  (5432, or the port it was given);
- a push monitor for each kind of backup, with an interval of a day and a few
  hours. `sudo /usr/local/bin/scannerpulse backups push pgbackrest` (and `dump`)
  on the machine whose database it backs up asks for the push address: with
  database machines, on each of them, since either may be the primary;
- a push monitor for each machine's disk, with an interval of about 15
  minutes: `sudo /usr/local/bin/scannerpulse disk push` on that machine asks
  for its address, and calls it every 5 minutes while `/` is less than 80%
  full.

Certificate errors can be ignored in the HTTPS monitors while the site uses
setup's own certificate.

The admin page's **Layout** area shows the same from inside, every 15 seconds:
- each ScannerPulse machine, whether it answers, and its version;
- which machine holds the shared address, and which files and transcribes the
  calls;
- each database machine's role, and how far the standby is behind;
- whether the referee answers;
- the command for a switchover. It swaps the database machines' roles while
  both are healthy, and is run on the first ScannerPulse machine.

## Changes later

- **A new release:** `sudo /usr/local/bin/scannerpulse update` on the first
  machine, or "Update now" in the admin page's Updates area on either one,
  updates every machine in turn: the referee and the database
  machines, then the second, then the first. Both ScannerPulse machines must
  answer first, and each restarts only while the other answers. The shared
  address does not move for a restart, so while the machine holding it
  restarts its web (a second or two), a request can fail. Every machine must
  be up; one that is not stops the update before anything changes.
- **A certificate changed on the first machine** (`tls install`, `tls self`,
  or Let's Encrypt): join the second again (`sudo /usr/local/bin/scannerpulse
  setup` there, Advanced, "join an existing install"). It takes the first
  machine's settings and certificate afresh. On the second itself, these
  commands are refused.
- **A different shared address:** `sudo /usr/local/bin/scannerpulse second
  NAME NEW-ADDRESS/PREFIX` on the first machine, then join the second again.
- **A different second machine:** uninstall ScannerPulse on the old one first,
  so that it no longer claims the address. Then run `second` with the new
  one's name on the first machine. The database machines join again, then the
  new second.
- **A second added to an install already set up:** `second NAME
  ADDRESS/PREFIX` on the first machine. The database machines join again, then
  the second.
