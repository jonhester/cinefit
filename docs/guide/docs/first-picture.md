---
title: "First picture"
description: "Check each part of the chain with cinefit probe, display and gpu, set the panel size and screen ratio, and confirm the first picture on screen."
---

# First picture

Work from the input to the screen. The first three checks are read-only and safe while the service runs.

## 1. Is the card receiving a signal?

```sh
~/cinefit-app/current probe
```

You should see your DeckLink, its `persistent ID`, and the format arriving on its input (for example `3840x2160 23.976`, YCbCr, HDR metadata if any). No signal here means the problem is upstream: the splitter, the EDID or HDCP. See [The signal chain](/signal-chain/).

## 2. Does the display offer the right modes?

```sh
~/cinefit-app/current display
```

This lists the connected display, the modes it offers, its HDR and colorimetry support and the GPU's planes. Check that your panel's native size (for example `4096x2160`) appears at 23.976 and 24.

## 3. Does the GPU work?

```sh
~/cinefit-app/current gpu
```

This checks Vulkan and libplacebo, and renders and reads back test frames. It touches no display.

## 4. Tell cinefit about your display and screen

Create `~/.config/cinefit-rs/cinefit.toml` with what differs from the defaults (4096x2160 panel, 2.35:1 screen):

```toml
[output]
width = 3840

[geometry]
screen_ar = 2.39
```

Every setting is in the [Settings reference](/settings/). Restart the service to pick up the panel size:

```sh
systemctl restart cinefit
```

## 5. Look at the screen

Play something with black bars. Within a few seconds of the signal arriving you should see:

- A **banner** naming the picture type, such as `HDR10 / 4K 23.976 · to the projector`.
- The picture **filling the screen's height**, the bars cropped away.

Press **Info** on the remote (or `I` on a keyboard) for the info panel: what is coming in, what is going out, the crop, and frames shown versus repeated in the last 10 seconds. Repeated frames during steady playback point at a performance problem; see [Troubleshooting](/troubleshooting/).

::: callout tip "Testing by hand"
To run cinefit in a terminal instead of the service, stop the service first (`systemctl stop cinefit`), then `~/cinefit-app/current play -t 30`. Ctrl-C prints a summary. `play --dry-run` sets up capture and the first render but leaves the display alone. Start the service again afterwards.
:::
