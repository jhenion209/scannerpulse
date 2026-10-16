#!/usr/bin/env bash
# Builds the end-to-end tests' database: the schema, invented calls, and
# silent audio files for them.
# Usage: seed.sh BINARY ARCHIVE   (DATABASE_URL names an empty database)
set -euo pipefail
binary=$1
archive=$(mkdir -p "$2/e2e" && cd "$2" && pwd)
"$binary" db-update --quiet
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -o /dev/null -v archive="$archive" -f "$(dirname "$0")/seed.sql"
# The admin the admin page's tests sign in as (e2e/admin.ts); the password is
# made up for the tests.
printf 'e2e admin password\ne2e admin password\n' | "$binary" admin add e2e-admin >/dev/null
# 56 silent MPEG-2 layer III frames (16 kHz mono, 36 ms each): about 2 s.
python3 - "$archive/e2e" <<'PY'
import sys
frame = bytes([0xFF, 0xF3, 0x48, 0xC4]) + bytes(140)
for n in range(1, 36):
    if n != 7:
        open(f"{sys.argv[1]}/call-{n}.mp3", "wb").write(frame * 56)
PY
