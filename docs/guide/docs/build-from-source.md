---
title: "Building from source"
description: "Build and install cinefit from its source, or run it from your home directory without root: for unreleased changes, development, or a CPU too old for the published builds."
---

# Building from source

Most people should [install the package](/install/). The ways here run cinefit from `~/cinefit-app` in your home directory, as your own user, instead: for unreleased changes, for working on cinefit, or on a CPU too old for the published builds. Use one or the other: both install a service named `cinefit`.

Install the capture card's driver and join the groups first ([Installing](/install/), steps 1 and 3).

## Build tools

```sh
sudo apt install build-essential git curl meson ninja-build glslang-dev glslang-tools \
  python3-jinja2 libvulkan-dev liblcms2-dev libdrm-dev libdrm-amdgpu1 libgbm-dev mesa-vulkan-drivers
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## The source and libplacebo

```sh
git clone https://github.com/jonhester/cinefit.git
cd cinefit
scripts/build-libplacebo.sh
```

cinefit pins one version of libplacebo and links it into its binary; the script builds that version into `.deps/libplacebo` (about ten seconds).

## Build and install

```sh
tools/install.sh
```

Run from a checkout with no arguments, the installer builds the tree (`cargo build --release`) and installs the result in `~/cinefit-app/`, with `~/cinefit-app/current` pointing at it and the build it replaced kept as `previous`. Before switching it checks the new build starts and reads your settings. It refuses a tree with uncommitted changes, so that what runs always names a commit; `ALLOW_DIRTY=1` overrides that for experiments.

The first time, it prints the steps that need root, with your user filled in:

```sh
sudo cp ~/cinefit-app/cinefit.service /etc/systemd/system/
sudo cp ~/cinefit-app/50-cinefit.rules /etc/polkit-1/rules.d/
sudo systemctl daemon-reload
sudo systemctl enable --now cinefit
```

The service runs `~/cinefit-app/current play` as your user. The polkit rule lets your user start, stop and restart **this unit only** without a password, so updates need no `sudo`. Settings are in `~/.config/cinefit-rs/cinefit.toml`, logs in `~/cinefit-logs/` and recordings in `~/cinefit-clips/`.

## Updating, and going back

```sh
git pull
tools/install.sh                  # build and install
tools/install.sh --rollback       # swap current and previous
tools/install.sh --status         # what current and previous point at
RESTART=1 tools/install.sh        # install and restart onto it now
```

A running service keeps its build until it next starts; `RESTART=1` restarts it at once (the display resyncs).

## Releases, without root

The same installer also installs published releases into `~/cinefit-app`, with no clone and no build:

```sh
curl -fsSL https://github.com/jonhester/cinefit/releases/latest/download/install.sh | sh
```

It keeps a copy of itself in `~/cinefit-app/tools/install.sh`; run that for updates, with `--release VERSION` for a particular one, and `--rollback` and `--status` as above. The service steps are the same.
