---
title: "First picture"
description: "Check each part of the chain with cinefit probe, display and gpu, set the panel size and screen ratio, and confirm the first picture on screen."
---

# First picture

Work from the input to the screen. The first three checks are read-only and safe while the service runs.

## 1. Is the card receiving a signal?

```sh
cinefit probe
```

You should see your DeckLink, its `persistent ID`, and the format arriving on its input (for example `3840x2160 23.976`, YCbCr, HDR metadata if any). No signal here means the problem is upstream: the cables, the EDID, or a signal the card does not accept. See [The signal chain](/signal-chain/).

## 2. Does the display offer the right modes?

```sh
cinefit display
```

This lists the connected display, the modes it offers, its HDR and colorimetry support and the GPU's planes. Check that your panel's native size (for example `4096x2160`) appears at 23.976 and 24.

## 3. Does the GPU work?

```sh
cinefit gpu
```

This checks Vulkan and libplacebo, and renders and reads back test frames. It touches no display.

## 4. Tell cinefit about your display and screen

Set what differs from the defaults (a 4096x2160 panel and a 2.35:1 screen). The screen ratio is in the on-screen menu and on the web page; the panel size is set on the web page or in the settings file (`sudo -u cinefit editor /etc/cinefit/cinefit.toml`):

```toml
[output]
width = 3840

[geometry]
screen_ar = 2.39
```

Every setting is in the [Settings reference](/settings/). Restart the service to pick up the panel size:

```sh
sudo systemctl restart cinefit
```

## 5. Look at the screen

Play something with black bars. Within a few seconds of the signal arriving you should see:

- A **banner** naming the picture type, such as `HDR10 / 4K 23.976 · to the projector`.
- The picture **filling the screen's height**, the bars cropped away.

Press **Info** on the remote (or `I` on a keyboard) for the info panel: what is coming in, what is going out, the crop, and frames shown versus repeated in the last 10 seconds. Repeated frames during steady playback point at a performance problem; see [Troubleshooting](/troubleshooting/).

::: callout tip "Testing by hand"
To run cinefit in a terminal instead of the service, stop the service first (`sudo systemctl stop cinefit`), then `cinefit play -t 30`. Ctrl-C prints a summary. `play --dry-run` sets up capture and the first render but leaves the display alone. Start the service again afterwards.
:::
