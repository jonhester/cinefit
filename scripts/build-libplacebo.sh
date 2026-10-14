#!/usr/bin/env bash
# Builds the libplacebo cinefit is pinned to, as a static library, and
# installs it into a prefix (default: .deps/libplacebo in the repository,
# where the build looks for it).
#
# The pinned commit lives here and only here. Moving it is a deliberate change:
# update COMMIT, run this, update PL_API_VER in
# crates/cinefit-render/shim/placebo.c and the minimum version in its build.rs.
#
# Needs: git, meson, ninja, glslang, python3 with jinja2, the Vulkan headers,
# and lcms2. On Ubuntu 26.04:
#   apt install git meson ninja-build glslang-dev glslang-tools python3-jinja2 \
#     libvulkan-dev liblcms2-dev
set -euo pipefail

COMMIT=1dd022687b9ac1dd996855b51433bf770ae76aab  # 2026-10-10, API 375
REPO=https://code.videolan.org/videolan/libplacebo.git

root=$(cd "$(dirname "$0")/.." && pwd)
prefix=$(realpath -m "${1:-$root/.deps/libplacebo}")

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

git init -q "$work/src"
git -C "$work/src" remote add origin "$REPO"
git -C "$work/src" fetch -q --depth 1 origin "$COMMIT"
git -C "$work/src" checkout -q FETCH_HEAD
# Everything but the demos' GUI toolkit.
git -C "$work/src" submodule update -q --init --depth 1 \
  3rdparty/glad 3rdparty/jinja 3rdparty/markupsafe 3rdparty/Vulkan-Headers 3rdparty/fast_float

# Quiet unless something fails, then the whole log.
log="$work/build.log"
run() { "$@" >> "$log" 2>&1 || { cat "$log" >&2; exit 1; }; }
# Static: it goes inside the cinefit binary, so an installed copy carries
# nothing beside it and can't load another version by mistake. LGPL-2.1
# permits this with cinefit's source public (README, "Licences").
run meson setup "$work/build" "$work/src" \
  --prefix="$prefix" \
  --buildtype=release \
  --default-library=static \
  -Dvulkan=enabled -Dglslang=enabled -Dshaderc=disabled \
  -Dopengl=disabled -Dd3d11=disabled \
  -Dlcms=enabled -Ddovi=enabled -Dlibdovi=disabled \
  -Dunwind=disabled -Dxxhash=disabled \
  -Ddemos=false -Dtests=false -Dbench=false
run ninja -C "$work/build" install

echo "$COMMIT" > "$prefix/COMMIT"
# Its licence travels with the binaries it is linked into.
cp "$work/src/LICENSE" "$prefix/LICENSE"
echo "libplacebo ${COMMIT:0:12} installed in $prefix"
