---
title: "Updating"
description: "Update cinefit to a new release, roll back to the previous build, and choose when the service restarts onto it."
---

# Updating

```sh
~/cinefit-app/tools/install.sh                   # the latest release
~/cinefit-app/tools/install.sh --release 0.2.0   # or a particular one
```

The new build becomes `~/cinefit-app/current`, and the one it replaced is kept as `previous`.

## When it takes effect

**A running service keeps its build** until it next starts: at boot, or when you restart it. A restart is a resync on the display, so it is never done behind your back. To switch now:

```sh
RESTART=1 ~/cinefit-app/tools/install.sh
```

or, after installing, `systemctl restart cinefit`.

## Going back

```sh
~/cinefit-app/tools/install.sh --rollback   # swap current and previous
~/cinefit-app/tools/install.sh --status     # what current and previous point at
```

A second `--rollback` undoes the first.

## Settings across updates

Settings live in `~/.config/cinefit-rs/cinefit.toml`, outside the install, and carry over. If a release retires a setting, cinefit says at start what replaced it rather than ignoring the line.
