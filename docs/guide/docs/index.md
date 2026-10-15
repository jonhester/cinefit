---
title: "What cinefit is"
description: "cinefit is a headless Linux video processor for home theater projectors: HDMI in, cropped and scaled picture out, with HDR signaling under its own control."
---

# What cinefit is

cinefit is a video processor for a home theater, running on an ordinary Linux PC with no desktop. It sits between your sources and your projector:

```text
player or streamer -> receiver -> HDMI splitter -> capture card -> [cinefit PC] -> projector
```

Every frame that arrives on the capture card is:

1. **Measured**: cinefit finds the black bars around the picture.
2. **Placed**: the picture is scaled to fill your screen's height, so a 2.39:1 film and a 16:9 show are the same height on a scope screen (constant image height).
3. **Rendered** on the GPU with [libplacebo](https://code.videolan.org/videolan/libplacebo): cropped, scaled and, where needed, color converted or tone mapped.
4. **Sent** to the display at the source's own frame rate, with HDR signaling cinefit controls itself.

It has an on-screen menu driven by a remote, an info panel, and a web page on your home network for status and settings. Once installed it runs as a system service: power the PC on and it plays whatever arrives.

## What it handles

| Input | What goes out |
|---|---|
| HDR10 | HDR10 passed through for the display to tone map (default), or tone mapped by cinefit to SDR in a BT.2020 container |
| Dolby Vision, tunneled | Decoded by cinefit from the pixels and mapped to SDR BT.2020 |
| Dolby Vision, low latency (LLDV) | Already mapped by the player; shown as is in SDR BT.2020 |
| SDR | Placed in BT.2020 and scaled to the panel |
| HLG | Passed through |

See [Picture types](/picture/) for the details.

## What it is not

- **Not a desktop app.** It takes the display and the capture card for itself; nothing else may use them while it runs.
- **Not tested widely.** cinefit is developed on one system: an AMD RX 6400, a Blackmagic DeckLink Mini Recorder 4K and a JVC 4096x2160 projector on a 2.35:1 screen. Other GPUs and displays may work but have not been tried. [What you need](/requirements/) says which parts matter.
- **Not a scaler for HDCP content on its own.** Capture cards refuse HDCP-protected signals; see [The signal chain](/signal-chain/).

::: callout tip "Using an AI assistant to set up"
This guide is published with an `llms.txt` file so that ChatGPT, Claude and other assistants can read all of it at once. See [Getting help from an AI](/ai-help/).
:::
