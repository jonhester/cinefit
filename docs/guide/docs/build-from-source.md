---
title: "Building from source"
description: "Build and install cinefit from its source: for unreleased changes, development, or a CPU too old for the published x86-64-v3 builds."
---

# Building from source

Most people should [install a release](/install/). Build from source to run changes that are not released yet, to work on cinefit, or on a CPU too old for the published builds.

Do steps 1 and 2 of [Installing](/install/) first (the capture driver, libraries and groups). Then:

## 1. Build tools

```sh
sudo apt install build-essential git curl meson ninja-build glslang-dev glslang-tools \
  python3-jinja2 libvulkan-dev liblcms2-dev libdrm-dev libgbm-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## 2. The source and libplacebo

```sh
git clone https://github.com/jonhester/cinefit.git
cd cinefit
scripts/build-libplacebo.sh
```

cinefit pins one version of libplacebo and links it into its binary; the script builds that version into `.deps/libplacebo` (about ten seconds).

## 3. Build and install

```sh
tools/install.sh
```

Run from a checkout with no arguments, the installer builds the tree (`cargo build --release`) and installs the result the same way as a release: into `~/cinefit-app/`, as `current`, keeping the build it replaced as `previous`. It refuses a tree with uncommitted changes, so that what runs always names a commit; `ALLOW_DIRTY=1` overrides that for experiments.

Then do step 4 of [Installing](/install/) (the service) if you have not already.

## Updating

```sh
git pull
tools/install.sh
```

`tools/install.sh --release` from the checkout installs a published release instead, and `--rollback` and `--status` work as described in [Updating](/updating/).
