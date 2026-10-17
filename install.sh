#!/bin/sh
# Installs ScannerPulse:
#   curl -fsSL https://github.com/jhenion209/scannerpulse/releases/latest/download/install.sh | sh
# It finds the system and the processor, downloads the matching program and
# the signed checksums from the project's release, checks both, and installs
# /usr/local/bin/scannerpulse. Nothing else changes: setup is the next step.
set -eu

RELEASE=${SCANNERPULSE_RELEASE_URL:-https://github.com/jhenion209/scannerpulse/releases/latest/download}
BIN_DIR=${SCANNERPULSE_BIN_DIR:-/usr/local/bin}
SUPPORTED="Debian 12 and 13, Ubuntu 22.04 and 24.04, Fedora, Rocky Linux and AlmaLinux 8 to 10, on x86_64 or aarch64 (64-bit ARM, such as a Raspberry Pi 4 or 5)"

# The public part of the key that signs every release (OpenPGP, base64).
RELEASE_KEY=''

fail() {
    echo "install: $*" >&2
    exit 1
}

[ "$(uname -s)" = Linux ] || fail "ScannerPulse runs on Linux: $SUPPORTED."
case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) fail "this processor ($(uname -m)) is not supported; ScannerPulse runs on $SUPPORTED." ;;
esac
[ -r /etc/os-release ] || fail "cannot tell which system this is (no /etc/os-release); ScannerPulse runs on $SUPPORTED."
# os-release is written to be read by the shell.
. /etc/os-release
ID=${ID:-} VERSION_ID=${VERSION_ID:-}
case "$ID:$VERSION_ID" in
    debian:12 | debian:13 | ubuntu:22.04 | ubuntu:24.04) ;;
    fedora:?*) ;;
    rocky:8* | rocky:9* | rocky:10* | almalinux:8* | almalinux:9* | almalinux:10*) ;;
    *) fail "${PRETTY_NAME:-$ID $VERSION_ID} is not supported; ScannerPulse runs on $SUPPORTED." ;;
esac
[ -n "$RELEASE_KEY" ] || fail "this install script carries no release key, so it cannot check what it downloads."
command -v gpgv >/dev/null || fail "gpgv checks the release's signature: install it first (Debian and Ubuntu: apt-get install gpgv; Fedora, Rocky and Alma: dnf install gnupg2)."

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
program=scannerpulse-$arch-linux
for file in "$program" SHA256SUMS SHA256SUMS.sig; do
    curl -fsSL -o "$tmp/$file" "$RELEASE/$file" || fail "cannot download $RELEASE/$file"
done
printf '%s' "$RELEASE_KEY" | base64 -d >"$tmp/key.gpg"
GNUPGHOME=$tmp gpgv --keyring "$tmp/key.gpg" "$tmp/SHA256SUMS.sig" "$tmp/SHA256SUMS" 2>/dev/null ||
    fail "the release's signature does not check out; nothing was installed."
(cd "$tmp" && grep "  $program\$" SHA256SUMS | sha256sum -c --quiet -) ||
    fail "the program does not match its signed checksum; nothing was installed."

if [ -w "$BIN_DIR" ]; then
    as_root=
elif [ "$(id -u)" -eq 0 ]; then
    as_root=
elif command -v sudo >/dev/null; then
    as_root=sudo
else
    fail "installing in $BIN_DIR needs root: run this as root."
fi
$as_root install -m 0755 "$tmp/$program" "$BIN_DIR/scannerpulse"
echo "Installed $("$BIN_DIR/scannerpulse" --version) in $BIN_DIR/scannerpulse."
if [ -e /etc/scannerpulse/scannerpulse.env ]; then
    echo "This machine is set up already: restart its services to run this version:"
    echo "  sudo systemctl restart scannerpulse-web scannerpulse-ingest"
else
    echo "Next: sudo scannerpulse setup"
fi
