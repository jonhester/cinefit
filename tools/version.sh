#!/bin/sh
# cinefit's versions, worked out from the tags: YEAR.MONTH.N, N counting
# releases within the month (2026.10.1, 2026.10.2, then 2026.11.1).
# Pre-releases carry the version they lead to, so everything sorts in the
# order it happened:
#
#   2026.10.2                    a release        (tag v2026.10.2)
#   2026.10.3-nightly.20261014   a nightly        (tag v2026.10.3-nightly.20261014)
#   2026.10.3-rc.1               a candidate      (tag v2026.10.3-rc.1)
#   2026.10.3                    the next release
#
#   tools/version.sh next           the next release's version
#   tools/version.sh nightly        tonight's nightly version
#   tools/version.sh rc             the next release candidate's version
#   tools/version.sh check TAG      the version a tag names, if it is valid
#   tools/version.sh tag [--rc]     tag HEAD as the next release (or
#                                   candidate) and push the tag: the release
#                                   workflow builds and publishes it
#
# Months have no leading zero (2026.9.1): SemVer forbids one.
set -eu

die() { echo "version.sh: $*" >&2; exit 1; }

# The newest release tag's version, or nothing.
last_release() {
    git tag -l 'v*' | sed -n 's/^v\([0-9]*\.[0-9]*\.[0-9]*\)$/\1/p' | sort -t. -k1,1n -k2,2n -k3,3n | tail -1
}

# The next release: N+1 within this month, or this month's first.
next() {
    month=$(date -u +%Y.%-m)
    last=$(last_release)
    case "$last" in
        "$month".*) echo "$month.$((${last##*.} + 1))" ;;
        *) echo "$month.1" ;;
    esac
}

# PREFIX.N with the first free N from START (default 1).
free() {
    n=${2:-1}
    while git rev-parse -q --verify "refs/tags/v$1.$n" >/dev/null; do n=$((n + 1)); done
    echo "$1.$n"
}

nightly() {
    base="$(next)-nightly.$(date -u +%Y%m%d)"
    # A second nightly the same day (run by hand) gets .2, .3, ...
    if git rev-parse -q --verify "refs/tags/v$base" >/dev/null; then
        free "$base" 2
    else
        echo "$base"
    fi
}

check() {
    echo "$1" | grep -Eq '^v[0-9]{4}\.[1-9][0-9]?\.[1-9][0-9]*(-rc\.[1-9][0-9]*|-nightly\.[0-9]{8}(\.[1-9][0-9]*)?)?$' ||
        die "$1 is not vYEAR.MONTH.N, vYEAR.MONTH.N-rc.K or a nightly's tag"
    echo "${1#v}"
}

case "${1:-}" in
    next) next ;;
    nightly) nightly ;;
    rc) free "$(next)-rc" ;;
    check) check "${2:?check needs a tag}" ;;
    tag)
        [ -z "$(git status --porcelain --untracked-files=no)" ] || die "uncommitted changes"
        git fetch -q --tags origin
        [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] ||
            die "HEAD is not origin/main: tag what is on main"
        if [ "${2:-}" = "--rc" ]; then v=$(free "$(next)-rc"); else v=$(next); fi
        git tag -a "v$v" -m "cinefit $v"
        git push -q origin "v$v"
        echo "tagged v$v at $(git rev-parse --short HEAD); the release workflow builds it"
        ;;
    *) sed -n '2,/^set -eu/p' "$0" | sed -e '/^set -eu/d' -e 's/^# \{0,1\}//'; exit 2 ;;
esac
