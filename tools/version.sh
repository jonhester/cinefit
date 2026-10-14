#!/bin/sh
# cinefit's versions, worked out from the tags: SemVer, MAJOR.MINOR.PATCH,
# from 0.1.0. Which part to bump is a choice made when tagging. Pre-releases
# carry the version they lead to, so everything sorts in the order it
# happened:
#
#   0.1.0                    a release        (tag v0.1.0)
#   0.1.1-nightly.20261014   a nightly        (tag v0.1.1-nightly.20261014)
#   0.2.0-rc.1               a candidate      (tag v0.2.0-rc.1)
#   0.2.0                    the next release
#
# A nightly names the next patch; if the next release is a minor or major
# bump instead, the nightly still sorts below it.
#
#   tools/version.sh next [patch|minor|major]   the next release's version
#   tools/version.sh nightly                    tonight's nightly version
#   tools/version.sh check TAG                  the version a tag names, if valid
#   tools/version.sh tag patch|minor|major [--rc]
#       tag origin/main as the next release (or its next candidate) and push
#       the tag: the release workflow builds and publishes it
set -eu

die() { echo "version.sh: $*" >&2; exit 1; }

# The newest release tag's version, or nothing.
last_release() {
    git tag -l 'v*' | sed -n 's/^v\([0-9]*\.[0-9]*\.[0-9]*\)$/\1/p' |
        sort -t. -k1,1n -k2,2n -k3,3n | tail -1
}

# The release after the last, bumped at $1; 0.1.0 before any.
next() {
    case "${1:-patch}" in
        patch | minor | major) ;;
        *) die "bump patch, minor or major, not '$1'" ;;
    esac
    last=$(last_release)
    [ -n "$last" ] || { echo 0.1.0; return; }
    major=${last%%.*} rest=${last#*.}
    minor=${rest%%.*} patch=${rest#*.}
    case "${1:-patch}" in
        patch) echo "$major.$minor.$((patch + 1))" ;;
        minor) echo "$major.$((minor + 1)).0" ;;
        major) echo "$((major + 1)).0.0" ;;
    esac
}

# PREFIX.N with the first free N from START (default 1).
free() {
    n=${2:-1}
    while git rev-parse -q --verify "refs/tags/v$1.$n" >/dev/null; do n=$((n + 1)); done
    echo "$1.$n"
}

nightly() {
    base="$(next patch)-nightly.$(date -u +%Y%m%d)"
    # A second nightly the same day (run by hand) gets .2, .3, ...
    if git rev-parse -q --verify "refs/tags/v$base" >/dev/null; then
        free "$base" 2
    else
        echo "$base"
    fi
}

check() {
    n='(0|[1-9][0-9]*)'
    echo "$1" | grep -Eq "^v$n\.$n\.$n(-rc\.[1-9][0-9]*|-nightly\.[0-9]{8}(\.[1-9][0-9]*)?)?\$" ||
        die "$1 is not vMAJOR.MINOR.PATCH, with -rc.K or a nightly's suffix at most"
    echo "${1#v}"
}

case "${1:-}" in
    next) next "${2:-patch}" ;;
    nightly) nightly ;;
    check) check "${2:?check needs a tag}" ;;
    tag)
        level=${2:?tag needs patch, minor or major}
        [ -z "$(git status --porcelain --untracked-files=no)" ] || die "uncommitted changes"
        git fetch -q --tags origin
        [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] ||
            die "HEAD is not origin/main: tag what is on main"
        v=$(next "$level")
        if [ "${3:-}" = "--rc" ]; then v=$(free "$v-rc"); fi
        git tag -a "v$v" -m "cinefit $v"
        git push -q origin "v$v"
        echo "tagged v$v at $(git rev-parse --short HEAD); the release workflow builds it"
        ;;
    *) sed -n '2,/^set -eu/p' "$0" | sed -e '/^set -eu/d' -e 's/^# \{0,1\}//'; exit 2 ;;
esac
