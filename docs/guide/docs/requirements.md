---
title: "What you need"
description: "The parts for a cinefit system, in plain terms: a small Linux PC with an AMD graphics card, a Blackmagic DeckLink capture card, an HDMI splitter, and your projector."
---

# What you need

A cinefit system is a small PC that sits in your rack between your sources and your projector. Here is the shopping list.

::: steps

::: step "A PC to run it on"
Almost any desktop PC from the last ten years will do. It needs a free PCIe slot for the capture card, and it will run **Ubuntu 26.04** with no desktop: cinefit starts by itself when the PC powers on, and you never need a monitor or keyboard on it after setup.
::: /step

::: step "A graphics card"
cinefit is built and tested with an **AMD Radeon RX 6400**, a small, quiet, low-power card that has plenty of headroom. Other recent AMD cards should work the same way. NVIDIA and Intel graphics have not been tried yet.
::: /step

::: step "A capture card"
A **Blackmagic DeckLink** card with an HDMI input, such as the **DeckLink Mini Recorder 4K**. This is how the picture gets into the PC.
::: /step

::: step "An HDMI splitter that removes HDCP"
Players copy-protect their output, and capture cards refuse protected signals. A splitter such as the **EZCOO EZ-SP12H2**, placed just before the capture card, takes care of that. [The signal chain](/signal-chain/) explains how to set it up.
::: /step

::: step "Your projector or TV"
Anything with an HDMI input. cinefit starts out set up for a 4096x2160 projector on a 2.35:1 scope screen, and two settings adapt it to yours.
::: /step

::: step "A remote (optional)"
Any remote that can send keyboard keys through a small USB receiver, such as a **flirc**. A plain USB keyboard works too, and there is a web page for phones.
::: /step

:::

::: callout info "Good to know before you buy"
- **4K at 60 frames per second**: the Mini Recorder 4K captures 4K only up to 30 frames per second. Films (24 fps) are fine. Set your player to match the content's frame rate.
- **Your projector's resolution**: check that it accepts its native resolution at 24 Hz. Nearly every 4K projector does.
- **The PC's CPU**: ready-made releases need a CPU from about 2015 or later (Intel Haswell, AMD Excavator or Zen). Older ones can [build from source](/build-from-source/).
:::

## The finer print

For anyone matching parts or troubleshooting:

- **GPU**: needs Vulkan (Mesa's RADV driver on AMD), a display output the Linux kernel drives directly, and an overlay plane for the on-screen menu. cinefit holds the GPU's clocks steady on AMD only; elsewhere it logs `gpu clocks: not held` and carries on.
- **Capture driver**: Blackmagic Desktop Video 16.0 or newer.
- **CPU**: published builds target x86-64-v3 (AVX2).
- **Nothing else on the display**: no desktop environment on the GPU's output. cinefit drives the display itself.
- **Optional extras**: a Denon receiver and an Apple TV on your network add input names and what's playing to the on-screen info. See [Receiver and Apple TV](/integrations/).
