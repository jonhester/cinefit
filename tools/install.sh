#!/bin/sh
# Puts a build where cinefit.service runs it (tools/cinefit.service), or
# switches the service to another build. No root: the service runs
# ~/cinefit-app/current, a symlink this script repoints.
#
#   tools/install.sh             build this tree and make it current
#   tools/install.sh --rollback  swap current and previous
#   tools/install.sh --cpp       point current at the C++ build
#   tools/install.sh --status    show what current and previous point at
#
# Every switch keeps the build it replaced as `previous`, so --rollback undoes
# it and a second --rollback redoes it. A running service keeps its build until
# its next start (at boot, or after a crash): a restart is a resync on the
# projector, not something to do while someone is watching. RESTART=1 restarts
# it now.
#
# The service never runs out of a working tree: a rebuild there would change
# what runs mid-film. Builds are copied into ~/cinefit-app/rust-<commit>/ with
# the libplacebo they were linked against, which the binary finds beside itself
# ($ORIGIN, crates/cinefit/build.rs).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
app="${CINEFIT_APP:-$HOME/cinefit-app}"

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
        "$root/tools/$1" > "$app/$1"
}

# Until the unit runs `current`, a switch here changes nothing the service
# runs. Installing the unit is the one step that needs root.
unit_ready() {
    systemctl show -p ExecStart --value cinefit 2>/dev/null | grep -q "path=$app/current "
}

check_unit() {
    unit_ready && return 0
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

install_build() {
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
    bin="$root/target/release/cinefit"

    # The libplacebo the binary loads from the build tree, by its soname.
    lib=$(ldd "$bin" | awk '$1 ~ /^libplacebo\.so/ && $2 == "=>" { print $3 }')
    [ -f "$lib" ] || die "the build does not resolve libplacebo (ldd: '$lib')"

    name="rust-$version"
    dest="$app/$name"
    mkdir -p "$dest"
    # Copy beside each file and rename over it: reinstalling the build that is
    # running must not leave it a half-written file.
    for f in "$bin" "$lib"; do
        base=$(basename "$f")
        cp -L "$f" "$dest/.$base.new"
        mv -f "$dest/.$base.new" "$dest/$base"
    done
    echo "$version $(date -Iseconds)" > "$dest/VERSION"

    # The copy must load its own libplacebo, not the build tree's or the
    # system's; otherwise it breaks the day .deps changes.
    loaded=$(ldd "$dest/cinefit" | awk '$1 ~ /^libplacebo\.so/ { print $3 }')
    [ "$loaded" = "$dest/$(basename "$lib")" ] ||
        die "$dest/cinefit loads libplacebo from '$loaded', not beside itself"
    # It starts, and reads the settings file the service will: a bad file
    # found now, not as a service failing every 5 s.
    "$dest/cinefit" settings > /dev/null ||
        die "$dest/cinefit settings failed; current left as it was"

    echo "installed $version into $dest"
    switch_to "$name/cinefit"
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
    "") install_build ;;
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
