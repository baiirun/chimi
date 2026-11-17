#!/bin/sh
# Vendors mbedTLS into compiler/chimi/runtime/native/bindings/tls/mbedtls from
# a release tarball, checking its sha256 first, and writes MANIFEST.sha256
# for the files it copied. Usage:
#   scripts/vendor-mbedtls.sh mbedtls-4.1.0.tar.bz2
# The sources copied are what the default configuration compiles: the TLS
# and X.509 library, TF-PSA-Crypto's core, platform, utilities, extras and
# built-in drivers, and the headers they include. Update PROVENANCE.md by
# hand with the new version and checksum.
set -eu

VERSION=4.1.0
SHA256=377a09cf8eb81b5fb2707045e5522d5489d3309fed5006c9874e60558fc81d10

tarball=$1
root=$(cd "$(dirname "$0")/.." && pwd)
target=$root/compiler/chimi/runtime/native/bindings/tls/mbedtls

echo "$SHA256  $tarball" | sha256sum -c -
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tar -xjf "$tarball" -C "$work"
source=$work/mbedtls-$VERSION

rm -rf "$target"
mkdir -p "$target"
cd "$source"
{
  find include library -name '*.c' -o -name '*.h'
  find tf-psa-crypto/include tf-psa-crypto/core tf-psa-crypto/platform \
    tf-psa-crypto/utilities tf-psa-crypto/extras tf-psa-crypto/dispatch \
    tf-psa-crypto/drivers/builtin -name '*.c' -o -name '*.h'
} | LC_ALL=C sort > "$work/files"
while read -r file; do
  mkdir -p "$target/$(dirname "$file")"
  cp "$file" "$target/$file"
done < "$work/files"
cp LICENSE "$target/LICENSE"
cp tf-psa-crypto/LICENSE "$target/tf-psa-crypto/LICENSE"
cd "$target"
find . -type f ! -name MANIFEST.sha256 ! -name PROVENANCE.md | sed 's|^\./||' | LC_ALL=C sort \
  | xargs sha256sum > MANIFEST.sha256
echo "vendored mbedTLS $VERSION: $(wc -l < MANIFEST.sha256) files"
