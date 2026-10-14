#!/bin/sh
# cinefit's versions: SemVer, MAJOR.MINOR.PATCH, from 0.1.0, one tag per
# release (v0.1.0). Which part to bump is a choice made when tagging.
#
#   tools/version.sh next [patch|minor|major]   the next release's version
#   tools/version.sh check TAG                  the version a tag names, if valid
#   tools/version.sh tag patch|minor|major      tag origin/main as the next
#                                               release and push the tag: the
#                                               release workflow publishes it
set -eu

die() { echo "version.sh: $*" >&2; exit 1; }

# The release after the newest tag, bumped at $1; 0.1.0 before any.
next() {
    case "${1:-patch}" in
        patch | minor | major) ;;
        *) die "bump patch, minor or major, not '$1'" ;;
    esac
    last=$(git tag -l 'v*' | sed -n 's/^v\([0-9]*\.[0-9]*\.[0-9]*\)$/\1/p' |
        sort -t. -k1,1n -k2,2n -k3,3n | tail -1)
    [ -n "$last" ] || { echo 0.1.0; return; }
    major=${last%%.*} rest=${last#*.}
    minor=${rest%%.*} patch=${rest#*.}
    case "${1:-patch}" in
        patch) echo "$major.$minor.$((patch + 1))" ;;
        minor) echo "$major.$((minor + 1)).0" ;;
        major) echo "$((major + 1)).0.0" ;;
    esac
}

check() {
    n='(0|[1-9][0-9]*)'
    echo "$1" | grep -Eq "^v$n\.$n\.$n\$" || die "$1 is not vMAJOR.MINOR.PATCH"
    echo "${1#v}"
}

case "${1:-}" in
    next) next "${2:-patch}" ;;
    check) check "${2:?check needs a tag}" ;;
    tag)
        level=${2:?tag needs patch, minor or major}
        [ -z "$(git status --porcelain --untracked-files=no)" ] || die "uncommitted changes"
        git fetch -q --tags origin
        [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] ||
            die "HEAD is not origin/main: tag what is on main"
        v=$(next "$level")
        git tag -a "v$v" -m "cinefit $v"
        git push -q origin "v$v"
        echo "tagged v$v at $(git rev-parse --short HEAD); the release workflow publishes it"
        ;;
    *) sed -n '2,/^set -eu/p' "$0" | sed -e '/^set -eu/d' -e 's/^# \{0,1\}//'; exit 2 ;;
esac
