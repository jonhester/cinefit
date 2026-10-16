---
title: "Commands"
description: "The cinefit command's subcommands for users: probe, display, gpu, settings, keys, play and pattern, and which are safe while the service runs."
---

# Commands

Every command takes `--help`, and `cinefit --version` prints the version. To run them as yourself, join the service's groups ([Installing, step 3](/install/)).

## Safe while the service runs

These only read.

| Command | What it does |
|---|---|
| `cinefit probe` | Capture devices, their persistent IDs, and the format arriving on each input. |
| `cinefit display` | The connected display, its modes, HDR and colorimetry support, and the GPU's planes. |
| `cinefit settings` | Every setting: its value, range, what changing it costs, and help. `--toml` prints a complete, commented settings file. |
| `cinefit gpu` | Checks Vulkan and libplacebo and renders test frames off screen. |

`cinefit probe --edid` also reads the card's input EDID. Even reading it can be a hotplug upstream, so it is opt-in.

## Need the service stopped

These take the capture card, the display or the remote. Stop the service first (`sudo systemctl stop cinefit`) and start it afterwards (`sudo systemctl start cinefit`).

| Command | What it does |
|---|---|
| `cinefit play` | What the service runs: capture to screen. `-t SECONDS` to stop after a while; `--set KEY=VALUE` to try a setting for one run without saving it; `--dry-run` to set up everything but the display. |
| `cinefit capture -t 10` | Captures for a while and summarizes what arrived. |
| `cinefit pattern -t 30` | A test pattern on the display, with frame pacing measured. `--hdr` sends it as HDR10. |
| `cinefit keys` | Prints what each remote button or key means (runs alongside the service, taking its keys). |

Ctrl-C ends a run and prints its summary; a second Ctrl-C exits at once.
