---
title: "What cinefit is"
description: "cinefit is a headless Linux video processor for home theater projectors: HDMI in, cropped and scaled picture out, with HDR signaling under its own control."
---

# What cinefit is

cinefit is a video processor for a home theater, running on an ordinary Linux PC with no desktop. It sits between your sources and your projector:

<!-- fig:path -->
<figure class="fig"><div class="scroll"><svg viewBox="0 0 760 424" role="img" aria-label="Every frame takes the same path, from the player to the screen" font-size="13"><defs><marker id="ah-path" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" class="ah"/></marker></defs><text x="20" y="18" text-anchor="start" class="t b" font-size="15">Every frame takes the same path, from the player to the screen</text><rect x="20" y="36" width="170" height="48" rx="8" class="card"/><text x="105.0" y="64.5" text-anchor="middle" class="t b">Player or streamer</text><rect x="230" y="36" width="170" height="48" rx="8" class="card"/><text x="315.0" y="56.5" text-anchor="middle" class="t b">Receiver</text><text x="315.0" y="72.5" text-anchor="middle" class="d">optional</text><rect x="440" y="36" width="170" height="48" rx="8" class="card"/><text x="525.0" y="56.5" text-anchor="middle" class="t b">HDMI splitter</text><text x="525.0" y="72.5" text-anchor="middle" class="d">chooses the EDID</text><path d="M190 60 L228 60" class="ln" marker-end="url(#ah-path)"/><path d="M400 60 L438 60" class="ln" marker-end="url(#ah-path)"/><rect x="20" y="112" width="720" height="236" rx="10" class="box"/><text x="728" y="134" text-anchor="end" class="t b">The cinefit PC: one frame every 42 ms at 24 fps</text><path d="M525 84 L525 98 L98 98 L98 156" class="ln" marker-end="url(#ah-path)"/><rect x="36" y="158" width="124" height="64" rx="8" class="card"/><text x="98.0" y="178.5" text-anchor="middle" class="t b">Capture</text><text x="98.0" y="194.5" text-anchor="middle" class="d">the frame, and</text><text x="98.0" y="210.5" text-anchor="middle" class="d">what kind it is</text><rect x="179" y="158" width="124" height="64" rx="8" class="hot"/><text x="241.0" y="178.5" text-anchor="middle" class="ht b">Find the bars</text><text x="241.0" y="194.5" text-anchor="middle" class="ht">where the picture</text><text x="241.0" y="210.5" text-anchor="middle" class="ht">really is</text><rect x="322" y="158" width="124" height="64" rx="8" class="card"/><text x="384.0" y="178.5" text-anchor="middle" class="t b">Place</text><text x="384.0" y="194.5" text-anchor="middle" class="d">constant image</text><text x="384.0" y="210.5" text-anchor="middle" class="d">height</text><rect x="465" y="158" width="124" height="64" rx="8" class="card"/><text x="527.0" y="178.5" text-anchor="middle" class="t b">Render</text><text x="527.0" y="194.5" text-anchor="middle" class="d">crop, scale, color</text><text x="527.0" y="210.5" text-anchor="middle" class="d">on the GPU</text><rect x="608" y="158" width="124" height="64" rx="8" class="card"/><text x="670.0" y="178.5" text-anchor="middle" class="t b">Output</text><text x="670.0" y="194.5" text-anchor="middle" class="d">HDMI out, with</text><text x="670.0" y="210.5" text-anchor="middle" class="d">HDR signaling</text><path d="M160 190 L177 190" class="ln" marker-end="url(#ah-path)"/><path d="M303 190 L320 190" class="ln" marker-end="url(#ah-path)"/><path d="M446 190 L463 190" class="ln" marker-end="url(#ah-path)"/><path d="M589 190 L606 190" class="ln" marker-end="url(#ah-path)"/><rect x="36" y="266" width="124" height="56" rx="8" class="card"/><text x="98.0" y="282.5" text-anchor="middle" class="t b">Dolby Vision</text><text x="98.0" y="298.5" text-anchor="middle" class="d">metadata read</text><text x="98.0" y="314.5" text-anchor="middle" class="d">from the pixels</text><path d="M98 222 L98 264" class="ln" marker-end="url(#ah-path)"/><path d="M160 294 L527 294 L527 224" class="ln" marker-end="url(#ah-path)"/><text x="190" y="286" text-anchor="start" class="d">tunneled Dolby Vision only</text><path d="M670 222 L670 372" class="ln" marker-end="url(#ah-path)"/><text x="660" y="364" text-anchor="end" class="d">HDMI</text><rect x="590" y="374" width="150" height="40" rx="8" class="card"/><text x="665.0" y="398.5" text-anchor="middle" class="t b">Projector or TV</text></svg></div><figcaption>One frame's path through cinefit. Finding the bars is the clever part: everything after it follows from where the picture is.</figcaption></figure>
<!-- /fig -->

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
- **Not a way around copy protection.** The capture card accepts only signals without HDCP, and cinefit does not remove or bypass it; see [The signal chain](/signal-chain/#accepts).

::: callout tip "Using an AI assistant to set up"
This guide is published with an `llms.txt` file so that ChatGPT, Claude and other assistants can read all of it at once. See [Getting help from an AI](/ai-help/).
:::
