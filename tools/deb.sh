#!/bin/sh
# Packs a built cinefit binary into a Debian package for Ubuntu:
#   tools/deb.sh BINARY VERSION OUTDIR
# gives OUTDIR/cinefit_VERSION_amd64.deb. The package installs
# /usr/bin/cinefit and the service in tools/deb/, which runs it as the cinefit
# system user with its files in /etc/cinefit, /var/lib/cinefit and
# /var/log/cinefit. Built for x86-64-v3, like the archive (tools/package.sh).
set -eu
[ $# -eq 3 ] || { echo "usage: $0 BINARY VERSION OUTDIR" >&2; exit 2; }
bin=$1 version=$2 out=$3
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

d="$work/cinefit"
install -Dm755 "$bin" "$d/usr/bin/cinefit"
strip "$d/usr/bin/cinefit"
install -Dm644 "$root/tools/deb/cinefit.service" "$d/usr/lib/systemd/system/cinefit.service"
install -Dm644 "$root/tools/deb/sysusers.conf" "$d/usr/lib/sysusers.d/cinefit.conf"
doc="$d/usr/share/doc/cinefit"
install -Dm644 "$root/LICENSE-MIT" "$doc/LICENSE-MIT"
install -Dm644 "$root/LICENSE-APACHE" "$doc/LICENSE-APACHE"
install -Dm644 "$root/crates/cinefit/assets/fonts/OFL-Inter.txt" "$doc/OFL-Inter.txt"
install -Dm644 "$root/crates/cinefit-decklink/vendor/decklink-sdk/LICENSE" "$doc/DeckLink-SDK.txt"
install -Dm644 "${CINEFIT_LIBPLACEBO:-$root/.deps/libplacebo}/LICENSE" "$doc/libplacebo-LGPL-2.1.txt"

mkdir -p "$d/DEBIAN"
for f in postinst prerm postrm; do
    install -m755 "$root/tools/deb/$f" "$d/DEBIAN/$f"
done
# What the binary links (install.sh checks the same on every install), and
# Mesa's Vulkan driver, which it loads. Desktop Video, the capture card's
# driver, is Blackmagic's own package, from their site: not a dependency apt
# could satisfy.
cat > "$d/DEBIAN/control" <<CONTROL
Package: cinefit
Version: $version
Architecture: amd64
Maintainer: Jon Hester <jonhester@users.noreply.github.com>
Section: video
Priority: optional
Homepage: https://github.com/jonhester/cinefit
Depends: libc6, libstdc++6, libgcc-s1, liblcms2-2, libvulkan1, libdrm2, libdrm-amdgpu1, libgbm1, libexpat1, mesa-vulkan-drivers, systemd
Installed-Size: $(du -sk "$d/usr" | cut -f1)
Description: video processor for a home theater projector
 Captures HDMI with a Blackmagic DeckLink card, crops black bars for constant
 image height, renders with libplacebo on Vulkan and drives the display
 through KMS, with HDR10 and Dolby Vision handled in process. Runs as a
 system service; needs Blackmagic Desktop Video 16.0 or newer.
CONTROL

mkdir -p "$out"
deb="$out/cinefit_${version}_amd64.deb"
dpkg-deb --root-owner-group --build "$d" "$deb" >/dev/null
echo "$deb"
