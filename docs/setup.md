# Setting up cinefit

cinefit was built for one theatre first: a JVC RS3000 on a 142" 2.35:1 scope
screen, an AMD Radeon RX 6400, a Blackmagic DeckLink Mini Recorder 4K and
Ubuntu Server 26.04. Other setups are added as people test them. This guide
covers what is known to work and says plainly where something has not been
tried.

**Contents**
- [What you need](#what-you-need)
- [1. The system](#1-the-system)
- [2. Building](#2-building)
- [3. Checking the hardware](#3-checking-the-hardware)
- [4. Running it as the service](#4-running-it-as-the-service)
- [5. Settings](#5-settings)
- [6. The remote](#6-the-remote)
- [When something is wrong](#when-something-is-wrong)

## What you need

**It should work with:**
- a **recent AMD GPU** on Linux's `amdgpu` driver, with a display connected
  to it;
- a **Blackmagic DeckLink** card with an HDMI input;
- **passthrough**: HDR10 sent to the display untouched, so the display does
  its own tone mapping (a JVC's Frame Adapt HDR, for example).

**Tested on:**

| Part | Tested |
|---|---|
| GPU | AMD Radeon RX 6400 (RDNA 2), Mesa 26.0 |
| Capture | DeckLink Mini Recorder 4K, Desktop Video 16.4 |
| OS | Ubuntu Server 26.04, kernel 7.0, no desktop |
| Display | JVC RS3000 (DLA-NX9), 4096x2160 |

**Why this hardware.** cinefit started on an NVIDIA card. It moved to a low-end
AMD card on purpose:
- a modest GPU keeps the algorithms efficient, since whatever runs on an RX 6400
  runs on anything faster;
- it got HDR10 passed through to the projector's Frame Adapt HDR working first,
  without a detour through NVIDIA's Linux drivers.

NVIDIA support comes back later, on the card already here.

**Known limits:**
- **NVIDIA:** not supported yet. NVIDIA's Linux driver advertises the HDR
  properties but rejects every value but the default, so HDR signalling fails.
  See [findings](findings.md#nvidia).
- **The DeckLink Mini Recorder 4K captures 2160p only up to 30 fps** (60 fps at
  1080p). That is the card's limit. Films at 23.976 are fine; a 60 Hz 4K source
  is not.
- **HDCP:** cinefit does not support HDCP; for protected sources you are on
  your own. AVS Forum's video processor threads are where people discuss their
  setups.
- **Tone mapping** (`output.mode = sdr2020`, HDR mapped to SDR by cinefit)
  works but is the least mature part. libplacebo's tone mapping does not yet
  match a JVC's Frame Adapt HDR, which is why passthrough is the default. A
  better mapping is being worked on, but until it is as good as what the JVC
  does itself, it is not a replacement for passthrough.
- **Intel GPUs, Magewell and other capture cards:** not tried.

## 1. The system

The account that runs cinefit needs these groups:
- `video`, for the display;
- `render`, for the GPU;
- `input`, for the remote.

Add them with:

```sh
sudo usermod -aG video,render,input "$USER"     # then log out and back in
```

Install Blackmagic's **Desktop Video** driver for your card (from Blackmagic's
support site), and check that the card is seen:

```sh
cinefit probe          # after building, below; lists the DeckLink devices
```

## 2. Building

You need Rust 1.99 or newer (from [rustup](https://rustup.rs)), and on Ubuntu
26.04:

```sh
sudo apt install git meson ninja-build glslang-dev glslang-tools python3-jinja2 \
  libvulkan-dev liblcms2-dev libdrm-dev libgbm-dev mesa-vulkan-drivers zstd
```

Then:

```sh
git clone https://github.com/jonhester/cinefit
cd cinefit
scripts/build-libplacebo.sh        # once; the pinned libplacebo, about 10 s
cargo build --release
```

There are published releases too, but the only one so far (0.1.0) is older
than most of what this guide describes. Build from source until there is a
newer one.

## 3. Checking the hardware

These read only, and touch no display or capture:

```sh
target/release/cinefit probe       # the capture card: model, HDMI input, persistent ID
target/release/cinefit display     # the display: connectors, the EDID's HDR support, planes
target/release/cinefit gpu         # the GPU path: Vulkan, dma-buf import, a test render
```

`cinefit bench` times the GPU's render into each scanout layout the display
takes, and checks that they match. Stop cinefit first for clean numbers.

## 4. Running it as the service

cinefit runs as a systemd service that starts at boot and plays whatever
arrives. No terminal is needed after setup. `tools/install.sh` builds and
installs, and prints a one-time root step the first time:

```sh
tools/install.sh
```

[service.md](service.md) has the one-time setup, and how to keep Ubuntu's
automatic updates from restarting cinefit mid-film. After that, everything is
done without root:

```sh
tools/install.sh               # install this tree's build (the service keeps the old one until it restarts)
RESTART=1 tools/install.sh     # install and restart now
tools/install.sh --rollback    # back to the previous build
tools/install.sh --status      # what is installed
```

A restart makes the display resync, so do it when nobody is watching.

## 5. Settings

Every setting can be changed from the **web page**, at
`http://<this machine>:8080` from any device on the home network, and from the
**on-screen menu**. They are saved in `~/.config/cinefit-rs/cinefit.toml`.
`cinefit settings` lists them all with their ranges and help, and each one
says what a change costs: live, a display modeset, or a restart.

The ones to set first:

| Setting | What it is |
|---|---|
| `output.width`, `output.height` | the panel's resolution (4096 x 2160 for a 17:9 4K projector) |
| `geometry.screen_ar` | the screen's aspect ratio, e.g. `2.35` for a scope screen |
| `output.mode` | `passthrough` (HDR10 to the display) or `sdr2020` (cinefit tone-maps) |
| `input.lldv` | `on` if your source sends low-latency Dolby Vision |
| `input.device` | the capture input, by the persistent ID `cinefit probe` prints (needed only with several) |

Optional: `avr.denon` (a Denon receiver's address, so cinefit shows which input
is on) and `atv.host` (an Apple TV, so cinefit shows what it is playing; pair it
from the web page).

## 6. The remote

cinefit reads key presses from `/dev/input`, so any USB keyboard works, and so
does a remote through a USB IR receiver that acts as a keyboard (a Flirc with
its Kodi profile, for example):

| Key | Action |
|---|---|
| C or M | menu |
| I or D | info panel |
| arrows | move |
| Enter, keypad Enter or E | select |
| Backspace, Esc or B | back |

`cinefit keys` shows what cinefit makes of each button as you press it. It
takes the keyboards from a running cinefit while it runs.

## When something is wrong

- The run log of every start is in `~/cinefit-logs`, and the web page lists the
  recent ones.
- `journalctl -u cinefit` has the service's output.
- The info panel (I) shows what is coming in and going out, and the crop.
