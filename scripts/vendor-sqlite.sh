#!/bin/sh
# Vendors the SQLite amalgamation into
# compiler/chimi/runtime/native/bindings/sqlite/sqlite from the release zip,
# checking its SHA3-256 (the checksum sqlite.org publishes) first, and writes
# MANIFEST.sha256 for the files it copied. Usage:
#   scripts/vendor-sqlite.sh sqlite-amalgamation-3540100.zip
# Only sqlite3.c and sqlite3.h are copied: the shell and the extension
# header are not compiled. Update PROVENANCE.md by hand with the new version
# and checksum.
set -eu

VERSION=3540100
SHA3_256=92281c86e2957f9b9305890f9ac04204caa7b677cc868dc326d942689df28e30

zip=$1
root=$(cd "$(dirname "$0")/.." && pwd)
target=$root/compiler/chimi/runtime/native/bindings/sqlite/sqlite

actual=$(openssl dgst -sha3-256 "$zip" | sed 's/.*= //')
if [ "$actual" != "$SHA3_256" ]; then
  echo "checksum mismatch: $actual" >&2
  exit 1
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
unzip -q "$zip" -d "$work"
source=$work/sqlite-amalgamation-$VERSION

rm -rf "$target"
mkdir -p "$target"
cp "$source/sqlite3.c" "$source/sqlite3.h" "$target/"
cd "$target"
find . -type f ! -name MANIFEST.sha256 ! -name PROVENANCE.md | sed 's|^\./||' | LC_ALL=C sort \
  | xargs sha256sum > MANIFEST.sha256
echo "vendored SQLite $VERSION: $(wc -l < MANIFEST.sha256) files"
