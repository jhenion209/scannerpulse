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
- `lease` shows which host holds the lease, and each host's heartbeat.
- `read --zone ZONE FILE...` prints what ingest reads from each file.

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or
the MIT license (`LICENSE-MIT`), at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in this
work by you, as defined in the Apache-2.0 license, shall be dual licensed as
above, without any additional terms or conditions.
