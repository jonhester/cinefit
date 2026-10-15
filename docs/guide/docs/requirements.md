---
title: "What you need"
description: "Hardware and software requirements for cinefit: an x86-64 Linux PC with Ubuntu 26.04, a Blackmagic DeckLink capture card, a Vulkan GPU (AMD tested) and a KMS display output."
---

# What you need

## The PC

- **CPU**: x86-64. Published releases need x86-64-v3 (AVX2, roughly any Intel or AMD CPU from 2015 on). An older CPU can still run cinefit built from source.
- **Operating system**: Ubuntu 26.04, server install (no desktop). Releases are built on 26.04; other distributions are untested.
- **No desktop or display manager on the output GPU.** cinefit drives the display directly through the kernel (KMS) and must be the only program doing so.

## The GPU

- **Tested**: AMD Radeon RX 6400 with Mesa's Vulkan driver (RADV).
- **Needed**: Vulkan, KMS with an output plane that takes 10-bit RGB, and an overlay plane for the on-screen menu and banners. Without an overlay plane cinefit plays but shows no menu, slate or banners.
- **Other vendors**: untested. cinefit links `libdrm_amdgpu`, so that library must be installed even on another GPU, and it holds the GPU's clocks steady only on AMD (elsewhere it logs `gpu clocks: not held` and carries on).

## The capture card

- **A Blackmagic DeckLink with an HDMI input.** Developed with the DeckLink Mini Recorder 4K.
- **Blackmagic Desktop Video 16.0 or newer**, the card's Linux driver, from Blackmagic's support site.
- **Mind the card's limits.** The Mini Recorder 4K captures 2160p only up to 30 frames per second (60 at 1080p and below). Films at 23.976 or 24 are fine; a 2160p60 source is not. Check your card's specifications.

## The display

- Any display connected to the GPU's HDMI output. cinefit's defaults are for a **4096x2160** projector on a **2.35:1** screen; for a 3840x2160 display, set [`output.width`](/settings/#output-width) to 3840.
- The display must offer its native size at the source's rates (23.976, 24, 59.94...). cinefit refuses to start a mode the display does not list.
- For constant image height on a scope screen, set the lens so the picture's **width** fills the screen; cinefit then uses the band of the panel that lands on the screen.

## Between the source and the card

- **An HDMI splitter that strips HDCP**, because the capture card refuses protected signals. See [The signal chain](/signal-chain/).

## Optional

- **An IR remote and a USB receiver** that types keyboard keys (developed with a Logitech Harmony and a flirc). Any USB keyboard works too. See [Remote and menu](/using/).
- **A Denon receiver** and an **Apple TV** on the network, for input names and now-playing information. See [Receiver and Apple TV](/integrations/).
