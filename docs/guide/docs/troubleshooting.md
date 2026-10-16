---
title: "Troubleshooting"
description: "Symptoms, causes and fixes for cinefit: no signal, black picture, Can't show this format, display in use, mode not offered, repeated frames, unexpected resyncs."
---

# Troubleshooting

Start with the log: `journalctl -u cinefit -n 100`, or the newest file in `/var/log/cinefit/` (also on the web page under **Recent runs and logs**). Each entry below is a symptom, its usual cause, how to tell, and the fix.

## The slate says "No signal"

- **Cause**: nothing reaches the capture card, or the signal is one it does not accept, such as an HDCP-protected one.
- **Check**: `cinefit probe` shows no format on the input. Try another source, or connect the source straight to the splitter.
- **Fix**: check the cables (rated for 18 Gbps) and the source, and see [What the capture card accepts](/signal-chain/#accepts).

## "Can't show this format · set the player to YCbCr"

- **Cause**: the player sends a pixel format cinefit does not render.
- **Fix**: set the player's color mode to YCbCr (4:2:2 if offered) and deep color to Auto or 10-bit, or change the splitter's EDID so the player chooses YCbCr. See [The signal chain](/signal-chain/).

## The picture is black but there is a signal

- **Cause**: some receivers send a valid signal with a black picture after a hotplug (an EDID change anywhere in the chain).
- **Check**: the info panel shows **In** with a format, yet the picture stays black. The **Out** row says what cinefit is sending.
- **Fix**: switch the receiver to another input and back.

## The service keeps restarting: "another program holds the display (DRM master)"

- **Cause**: a desktop, display manager or other video program owns the GPU's output.
- **Fix**: remove or disable the desktop (`sudo systemctl disable --now gdm` or similar). Only one program may drive the display.

## "capture input is in use by another program"

- **Cause**: another program, often a manual `cinefit` run or Blackmagic's own tools, holds the card.
- **Fix**: close it. Stop the service before running `cinefit play` or `capture` by hand.

## "DeckLink driver not loaded" or "too old"

- **Fix**: install Desktop Video 16.0 or newer, then reboot. `ls /dev/blackmagic/` should list `io0`.

## "no 4096x2160 mode within 0.01 Hz of 23.976"

- **Cause**: the display does not offer the panel size cinefit was told, at the source's rate.
- **Check**: `cinefit display` lists what it offers.
- **Fix**: set [`output.width`](/settings/#output-width) and [`output.height`](/settings/#output-height) to the display's native size (3840x2160 for most TVs and consumer projectors), and restart the service.

## Repeated frames or judder

- **Check**: the info panel's **Last 10 s** row: frames repeated during steady playback, and the frame time against the frame period (41.7 ms at 24 fps).
- **Cause**: the source's frame rate beyond the card's limit (the Mini Recorder 4K stops at 2160p30), a slow GPU, or something else using the GPU.
- **Fix**: set the source to match the content's frame rate; check nothing else runs on the PC.

## The crop is wrong or does not change

- **Check**: the info panel's **Crop** row says why when it cannot read the picture: the detector does not read RGB or a Dolby Vision tunnel, and keeps the last crop.
- **Fix**: send YCbCr where possible. To show every frame uncropped, set [`detect.crop`](/settings/#detect-crop) to off.

## The display blanks for a few seconds at odd times

- **Cause**: a resync on a change of frame rate or HDR type is expected ([Picture types](/picture/#resyncs)). One in the middle of a film, with **Stopping cinefit.service** in the journal, is Ubuntu's automatic updates restarting it.
- **Fix**: the two settings in [Installing, step 4](/install/#updates).

## The menu, banners and slate never appear

- **Cause**: the GPU's output has no overlay plane the on-screen display can use; the log says `osd: the output has no overlay plane`.
- **Fix**: none for now on that GPU; playback is unaffected.

## The web page does not open

- **Check**: the log says `web: no web page, port N not available` when another program has the port.
- **Fix**: set [`web.port`](/settings/#web-port) to a free port above 1023 and restart the service.

## Disk filling up

cinefit records what its bar detector measured, and some full frames, under `/var/lib/cinefit/caught/` (about 150 MB a film plus up to [`detect.frames_gb`](/settings/#detect-frames-gb) of frames). This helps tune the detector. Turn it off with [`detect.record`](/settings/#detect-record), or keep only the small readings with `detect.frames_gb = 0`. Run logs in `/var/log/cinefit/` are not cleaned up automatically.
