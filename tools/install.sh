#!/bin/sh
# Puts a build where cinefit.service runs it (tools/cinefit.service), or
# switches the service to another build. No root: the service runs
# ~/cinefit-app/current, a symlink this script repoints.
#
#   tools/install.sh                     build this tree and make it current;
#                                        outside a source tree, --release
#   tools/install.sh --release [VERSION] a published release (default: the
#                                        latest)
#   tools/install.sh --rollback          swap current and previous
#   tools/install.sh --cpp               point current at the C++ build
#   tools/install.sh --status            show what current and previous point at
#
# Every switch keeps the build it replaced as `previous`, so --rollback undoes
# it and a second --rollback redoes it. A running service keeps its build until
# its next start (at boot, or after a crash): a restart is a resync on the
# projector, not something to do while someone is watching. RESTART=1 restarts
# it now.
#
# The service never runs out of a working tree: a rebuild there would change
# what runs mid-film. A build from this tree is copied into
# ~/cinefit-app/rust-<commit>/, a published one into rust-<version>/: one file,
# with libplacebo linked in. Published builds come from GitHub's releases
# (CINEFIT_REPO, default jonhester/cinefit), built for x86-64-v3, checked
# against SHA256SUMS.
#
# Each release also publishes this script, so a release installs without a
# clone or a build:
#   curl -fsSL https://github.com/jonhester/cinefit/releases/latest/download/install.sh | sh
# Run outside a source tree, it installs releases only, and keeps itself and
# the service's templates, from the installed release's tag, in
# ~/cinefit-app/tools/: that copy is the one to run for the next update.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
app="${CINEFIT_APP:-$HOME/cinefit-app}"

# A source tree builds and has the templates beside this script; anywhere
# else (piped from curl, or the copy in $app/tools) only releases install,
# and the templates are the ones fetched with the last release.
if [ -f "$root/Cargo.toml" ] && [ -f "$root/tools/cinefit.service" ]; then
    tools="$root/tools"
else
    tools="$app/tools"
    root=""
fi

die() { echo "install.sh: $*" >&2; exit 1; }

# Replace a symlink in one rename, so the service never starts from a missing
# or half-made link.
point() {
    ln -sfn "$2" "$app/.$1.new"
    mv -T "$app/.$1.new" "$app/$1"
}

show() {
    echo "current  -> $(readlink "$app/current" 2>/dev/null || echo '(none)')"
    echo "previous -> $(readlink "$app/previous" 2>/dev/null || echo '(none)')"
}

# Make $1 current, keeping what it replaces as previous.
switch_to() {
    old=$(readlink "$app/current" 2>/dev/null || true)
    if [ "$old" = "$1" ]; then
        echo "current already points at $1"
        return
    fi
    point current "$1"
    [ -n "$old" ] && point previous "$old"
    return 0
}

# Fill in the templates for this user, for the steps only root can take.
render() {
    sed -e "s|@USER@|$(id -un)|g" -e "s|@GROUP@|$(id -gn)|g" -e "s|@HOME@|$HOME|g" \
        "$tools/$1" > "$app/$1"
}

# Until the unit runs `current`, a switch here changes nothing the service
# runs. Installing the unit is the one step that needs root.
unit_ready() {
    systemctl show -p ExecStart --value cinefit 2>/dev/null | grep -q "path=$app/current "
}

check_unit() {
    unit_ready && return 0
    [ -f "$tools/cinefit.service" ] || die "no service templates in $tools: install a release first"
    render cinefit.service
    echo
    echo "The service does not run $app/current yet. Once, as the owner:"
    echo "  sudo cp $app/cinefit.service /etc/systemd/system/"
    if ! systemctl cat cinefit >/dev/null 2>&1; then
        render 50-cinefit.rules
        echo "  sudo cp $app/50-cinefit.rules /etc/polkit-1/rules.d/"
        echo "  sudo systemctl daemon-reload"
        echo "  sudo systemctl enable --now cinefit"
    else
        echo "  sudo systemctl daemon-reload"
        echo "  systemctl restart cinefit     # the projector will resync"
    fi
    return 1
}

restart() {
    check_unit || return 0
    if ! systemctl is-active -q cinefit; then
        echo "cinefit is not running; it starts on this build"
    elif [ "${RESTART:-0}" = "1" ]; then
        echo "restarting cinefit onto it (the projector will resync)..."
        # Needs tools/50-cinefit.rules; without it systemctl asks for a password.
        systemctl restart cinefit
    else
        echo "the running service keeps its build until its next start; RESTART=1 to restart now"
    fi
}

# Puts binary $1 in place as $app/$2/cinefit, with $3 in its VERSION file,
# checks the copy, and makes it current.
place() {
    dest="$app/$2"
    mkdir -p "$dest"
    # Copy beside it and rename over it: reinstalling the build that is
    # running must not leave it a half-written file.
    cp "$1" "$dest/.cinefit.new"
    chmod +x "$dest/.cinefit.new"
    mv -f "$dest/.cinefit.new" "$dest/cinefit"
    echo "$3 $(date -Iseconds)" > "$dest/VERSION"

    # Everything it links resolves, and libplacebo is inside it: a copy that
    # loaded one from a build tree would break the day that tree changed.
    links=$(ldd "$dest/cinefit") || die "ldd $dest/cinefit failed"
    if echo "$links" | grep -q "not found"; then
        die "$dest/cinefit is missing libraries:
$(echo "$links" | grep "not found")"
    fi
    if echo "$links" | grep -q libplacebo; then
        die "$dest/cinefit loads libplacebo as a shared library; it should carry it"
    fi
    # It starts, and reads the settings file the service will: a bad file
    # found now, not as a service failing every 5 s.
    "$dest/cinefit" settings > /dev/null ||
        die "$dest/cinefit settings failed; current left as it was"

    echo "installed $("$dest/cinefit" --version) into $dest"
    switch_to "$2/cinefit"
}

install_build() {
    [ -n "$root" ] || die "not in a source tree: --release installs a published build"
    cd "$root"
    # A version that names a commit, so `current` says what is running.
    version=$(git rev-parse --short=12 HEAD)
    if ! git diff --quiet HEAD; then
        [ "${ALLOW_DIRTY:-0}" = "1" ] ||
            die "this tree has uncommitted changes; commit them, or ALLOW_DIRTY=1"
        version="$version-dirty"
    fi

    # Built here rather than trusted: an up-to-date build costs seconds, and a
    # stale binary has passed for a fix before.
    cargo build --release --locked -p cinefit
    place "$root/target/release/cinefit" "rust-$version" "$version"
}

repo="${CINEFIT_REPO:-jonhester/cinefit}"

# GitHub's API, one JSON value out: $1 the path, $2 a Python expression over
# the parsed reply `r`. Nothing on any failure: the caller says what is missing.
api() {
    { curl -fsSL "https://api.github.com/repos/$repo/$1" |
        python3 -c "import json, sys; r = json.load(sys.stdin); print($2)"; } 2>/dev/null || true
}

# Releases are built for x86-64-v3; refuse a CPU without it, rather than
# install a binary that dies with an illegal instruction.
need_v3() {
    flags=$(grep -m1 '^flags' /proc/cpuinfo)
    for f in avx2 bmi1 bmi2 fma f16c abm movbe xsave; do
        echo "$flags" | grep -qw "$f" ||
            die "this CPU lacks $f: releases need x86-64-v3; build from the source instead (tools/install.sh)"
    done
}

# Downloads, checks and installs release version $1.
install_release() {
    version=$1
    need_v3
    name="cinefit-$version-x86_64-v3"
    base="https://github.com/$repo/releases/download/v$version"
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    echo "downloading $name..."
    curl -fsSL -o "$work/$name.tar.gz" "$base/$name.tar.gz" ||
        die "no $name.tar.gz in release v$version"
    curl -fsSL -o "$work/SHA256SUMS" "$base/SHA256SUMS" || die "no SHA256SUMS in release v$version"
    (cd "$work" && grep " $name.tar.gz\$" SHA256SUMS | sha256sum -c --quiet -) ||
        die "$name.tar.gz does not match its SHA256SUMS"
    tar -C "$work" -xzf "$work/$name.tar.gz"
    # Outside a source tree, this script and the service's templates come
    # from the release's own tag, so they match the binary they install.
    if [ -z "$root" ]; then
        raw="https://raw.githubusercontent.com/$repo/v$version/tools"
        mkdir -p "$work/tools"
        for f in install.sh cinefit.service 50-cinefit.rules; do
            curl -fsSL -o "$work/tools/$f" "$raw/$f" || die "no tools/$f at v$version"
        done
    fi
    place "$work/$name/cinefit" "rust-$version" "$version"
    if [ -z "$root" ]; then
        mkdir -p "$tools"
        chmod +x "$work/tools/install.sh"
        for f in install.sh cinefit.service 50-cinefit.rules; do
            mv -f "$work/tools/$f" "$tools/$f"
        done
    fi
}

# The newest release's version.
latest_release() {
    api releases/latest "r['tag_name'][1:]"
}

# The C++ build takes no subcommand and refuses `play`, so the service reaches
# it through a wrapper that drops it. It loads its shaders relative to the
# working directory, which is $app, where its own install script puts them.
install_cpp() {
    [ -x "$app/cinefit-m3" ] || die "no C++ build at $app/cinefit-m3 (its tools/install-cinefit.sh puts it there)"
    mkdir -p "$app/cpp"
    cat > "$app/cpp/.cinefit.new" <<EOF
#!/bin/sh
# Written by the Rust tree's tools/install.sh --cpp: the service runs
# \`current play\`, and the C++ build takes no subcommand.
exec "$app/cinefit-m3"
EOF
    chmod +x "$app/cpp/.cinefit.new"
    mv -f "$app/cpp/.cinefit.new" "$app/cpp/cinefit"
    switch_to cpp/cinefit
}

mkdir -p "$app"
case "${1:-}" in
    "" | --release)
        if [ -z "${1:-}" ] && [ -n "$root" ]; then
            install_build
        else
            v=${2:-$(latest_release)}
            [ -n "$v" ] || die "no release found"
            install_release "${v#v}"
        fi ;;
    --rollback)
        prev=$(readlink "$app/previous" 2>/dev/null) || die "nothing to roll back to"
        switch_to "$prev" ;;
    --cpp) install_cpp ;;
    --status) show; check_unit || true; exit 0 ;;
    -h | --help) sed -n '2,/^set -eu/p' "$0" | sed -e '/^set -eu/d' -e 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown argument '$1'; see --help" ;;
esac
show
restart
