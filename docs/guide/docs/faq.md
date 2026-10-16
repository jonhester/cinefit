---
title: "FAQ"
description: "Common questions about cinefit: what it costs, Dolby Vision and HDR10, latency, 4K60, TVs and scope screens, NVIDIA and Intel, privacy, updates and where to get help."
---

# FAQ

## General

### What is cinefit?

A video processor for a home theater that runs on a Linux PC you build: it captures what your player sends, crops away the black bars so every film is as tall as your screen, handles HDR10 and low-latency Dolby Vision, and drives the projector directly. [What cinefit is](/overview/) has the whole picture.

### Is it really free?

Yes. cinefit is open source, under the Apache-2.0 or MIT license, your choice: no price, no subscription, no license key, no paid tier and no features held back. Everything else it needs is free too: Ubuntu, and Blackmagic's Desktop Video driver for the capture card.

What costs money is the hardware ([What you need](/requirements/)). The project earns in two ways, both optional: the guide's Amazon links pay a small commission on what you buy through them, at no cost to you, and say so where they appear; and if cinefit is useful to you, you can leave a tip in the [tip jar](https://ko-fi.com/jonhester). Buying elsewhere, and not tipping, works just as well.

### Is it a finished product?

It is used every day in one theater, and developed and tested on one set of hardware. Other hardware may work but has not been tried, and the guide says so wherever it matters. There is no warranty or support contract; there is the [issue tracker](https://github.com/jonhester/cinefit/issues).

## Picture

### Does it support Dolby Vision?

**Low-latency** Dolby Vision, yes: the player maps it, and cinefit shows it as it arrives, in SDR BT.2020. **Tunneled** Dolby Vision, which cinefit would decode and map itself, is in progress and not ready yet. Which form a player sends depends on the EDID it sees ([The signal chain](/signal-chain/)). cinefit never sends Dolby Vision to the display. Details are in [Picture types](/picture/).

### What about HDR10?

By default it passes through as HDR10, for the projector's own tone mapping. Set [`output.mode`](/settings/#output-mode) to `sdr2020` and cinefit tone maps it to SDR BT.2020 instead.

### Does it upscale?

Yes. Everything is scaled to the display's panel: a 1080p source fills a 4K panel, and the banner says so ("scaled to 4K").

### What happens to subtitles in the black bars?

They are cropped with the bars. Player menus and on-screen controls are never cropped. To see the whole frame, turn off [Crop black bars](/settings/#detect-crop) from the menu.

### Does it add latency?

Some: each frame is captured, processed on the GPU and sent out. The total delay has not been measured yet. cinefit is built for films and shows, not for games.

### Can it do 4K at 60 frames per second?

That depends on the capture card. The DeckLink Mini Recorder 4K captures 4K up to 30 frames per second, which covers films at 24; set the player to match the content's frame rate. A source that stays at 4K60 needs a card that captures it ([What you need](/requirements/#capture)).

### Why does the screen go black for a few seconds sometimes?

That is a resync: the output changes frame rate, or between SDR, HDR10 and HLG, and the display takes a few seconds to lock on. A change of HDR metadata alone does not cause one. See [Picture types](/picture/#resyncs).

### Why do I get no picture from my player?

The capture card accepts only signals without HDCP copy protection, which most commercial players apply; cinefit does not remove or bypass it. See [What the capture card accepts](/signal-chain/#accepts) and [Troubleshooting](/troubleshooting/).

## Hardware

### Do I need a projector and a scope screen?

No. cinefit is designed around a projector on a 2.35:1 scope screen, where cropping gives constant image height. On a 16:9 screen or TV, set the [screen ratio](/settings/#geometry-screen-ar) to `16:9`: films keep their bars, and cinefit still handles HDR and scaling.

### Does it work with NVIDIA or Intel graphics?

Not yet: cinefit is developed and tested on AMD. Support for others is planned but not scheduled.

### Can I run it on Windows, a Mac or a Raspberry Pi?

No. cinefit runs on Linux on an x86-64 PC, with the capture card and graphics card in its PCIe slots.

### Can the PC do other things too?

It is meant to be an appliance: cinefit needs the display and the capture card to itself, and anything else using the GPU can cost it frames. A desktop on the same graphics card will not work.

## Using it

### Do I need a monitor and keyboard on the PC?

Only to install it. After that it starts by itself at power on, and the [remote](/using/) and the [web page](/web-page/) do the rest.

### Will an update interrupt a film?

No. Installing an update never restarts the running service; the new version takes over at the next start. Ubuntu's own automatic updates can restart it, unless you make the two settings in [Installing](/install/#updates).

### Does cinefit send anything off my network?

No. It talks only to the devices you point it at on your own network: a Denon receiver and an Apple TV, if you set them. By default it does keep the bar detector's readings, and a few frames within a disk limit, on the PC itself, to improve the detector; [Detector tuning](/settings/#detect-record) turns that off.

### Is the web page secure?

It has no password: anyone on your home network can change settings. Change its port, or turn it off, with [`web.port`](/settings/#web-port).

### Where do I get help?

Start with [Troubleshooting](/troubleshooting/). An AI assistant can read the whole guide ([Getting help from an AI](/ai-help/)). Bugs and questions go on [GitHub](https://github.com/jonhester/cinefit/issues).
