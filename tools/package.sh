#!/bin/sh
# Packs a built cinefit binary into a release archive:
#   tools/package.sh BINARY VERSION OUTDIR
# gives OUTDIR/cinefit-VERSION-x86_64-v3.tar.gz, holding the binary and every
# licence it ships under. Releases are built for x86-64-v3 (AVX2 and friends:
# x86 CPUs from about 2015 on).
#
# The binary needs from the system only what a working machine has anyway:
# Blackmagic Desktop Video for the capture card, and Mesa's Vulkan driver,
# GBM and libdrm for the GPU. libplacebo is inside it.
set -eu
[ $# -eq 3 ] || { echo "usage: $0 BINARY VERSION OUTDIR" >&2; exit 2; }
bin=$1 version=$2 out=$3 flavour=x86_64-v3
root=$(cd "$(dirname "$0")/.." && pwd)
name="cinefit-$version-$flavour"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

d="$work/$name"
mkdir -p "$d/licenses"
cp "$bin" "$d/cinefit"
strip "$d/cinefit"
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$d/licenses/"
cp "$root/crates/cinefit/assets/fonts/OFL-Inter.txt" "$d/licenses/"
cp "$root/crates/cinefit-decklink/vendor/decklink-sdk/LICENSE" "$d/licenses/DeckLink-SDK.txt"
cp "${CINEFIT_LIBPLACEBO:-$root/.deps/libplacebo}/LICENSE" "$d/licenses/libplacebo-LGPL-2.1.txt"
cat > "$d/README.txt" <<TXT
cinefit $version ($flavour)

  ./cinefit --version
  ./cinefit settings

Needs Blackmagic Desktop Video (the capture card's driver) and Mesa's Vulkan
driver. Installed as the service by tools/install.sh in the source:
https://github.com/jonhester/cinefit

libplacebo is linked statically, under the GNU LGPL 2.1 or later
(licenses/libplacebo-LGPL-2.1.txt). To link cinefit against a modified
libplacebo, build the source of this version (tag v$version) with
CINEFIT_LIBPLACEBO pointing at your libplacebo's prefix; see
scripts/build-libplacebo.sh.
TXT
mkdir -p "$out"
tar -C "$work" -czf "$out/$name.tar.gz" "$name"
echo "$out/$name.tar.gz"
