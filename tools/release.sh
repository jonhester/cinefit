#!/bin/sh
# Makes a release, as `npm version` does:
#
#   tools/release.sh patch|minor|major
#
# bumps the version in Cargo.toml (0.1.0 -> 0.1.1 | 0.2.0 | 1.0.0), commits
# "release X.Y.Z" on main, tags it vX.Y.Z and pushes both. The release
# workflow then tests, builds and publishes it. Only from an up-to-date,
# clean main: a release is what main holds.
set -eu
cd "$(dirname "$0")/.."

die() { echo "release.sh: $*" >&2; exit 1; }

level=${1:-}
case "$level" in
    patch | minor | major) ;;
    *) die "usage: tools/release.sh patch|minor|major" ;;
esac
[ -z "$(git status --porcelain --untracked-files=no)" ] || die "uncommitted changes"
[ "$(git symbolic-ref --short -q HEAD)" = main ] || die "not on main"
git fetch -q origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] ||
    die "main is not origin/main: pull or push first"

old=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
major=${old%%.*} rest=${old#*.}
minor=${rest%%.*} patch=${rest#*.}
case "$level" in
    patch) new="$major.$minor.$((patch + 1))" ;;
    minor) new="$major.$((minor + 1)).0" ;;
    major) new="$((major + 1)).0.0" ;;
esac
git rev-parse -q --verify "refs/tags/v$new" >/dev/null && die "v$new is already tagged"

# The workspace's version, the first `version =` in the file; the lock file
# follows without touching any dependency.
sed -i "0,/^version = \"$old\"$/s//version = \"$new\"/" Cargo.toml
cargo update --workspace --offline -q
git commit -q -am "release $new"
git tag -a "v$new" -m "cinefit $new"
# Atomic: if main is refused (a merge landed since the fetch), the tag is
# refused with it, rather than publishing a release from a commit not on main.
# Refused or not sent, the local commit and tag are undone, so the next run
# starts again from origin/main (the tree was clean before the bump).
if ! git push --atomic -q origin main "v$new"; then
    git tag -d "v$new" >/dev/null
    git reset -q --hard HEAD^
    die "push refused; the release commit and tag were undone here"
fi
echo "released $old -> $new: the release workflow builds and publishes v$new"
