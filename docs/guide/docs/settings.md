---
title: "Settings reference"
description: "Every cinefit setting: what it does, its values, its default and what changing it costs."
---

<!-- Generated from crates/cinefit/src/config.rs: edit the table there, then run CINEFIT_UPDATE_GUIDE=1 cargo test -p cinefit config -->

# Settings reference

Settings live in `/etc/cinefit/cinefit.toml` (in `~/.config/cinefit-rs/` when cinefit runs from your home directory), one `[section]` per group below. A file that is not there means every setting is at its default, and the file only needs the settings you change. The on-screen menu and the web page change the same settings and save them to the same file; to edit it by hand, `sudo -u cinefit editor /etc/cinefit/cinefit.toml`. `cinefit settings` prints them all with their current values, and `cinefit settings --toml` prints a complete file to start from.

Each setting says what changing it costs while cinefit is running:

- **Immediately**: the next frame uses it, with no interruption.
- **Immediately, with a resync**: the output is set up again, and the display blanks for a few seconds while it locks on to the new signal.
- **When cinefit starts**: read once at startup; restart the service to apply it.

## [output]

### output.width {#output-width}

The panel's width in pixels: the output's size. 4096 for a 17:9 4K projector.

- In the file: `width` under `[output]`
- Values: 640-8192
- Default: `4096`
- Applies: when cinefit starts

### output.height {#output-height}

The panel's height in pixels.

- In the file: `height` under `[output]`
- Values: 480-4320
- Default: `2160`
- Applies: when cinefit starts

### output.mode {#output-mode}

sdr2020: SDR in a BT.2020 container, HDR tone-mapped down to output.white_nits; set the display to an SDR BT.2020 mode. passthrough: HDR10 goes out as HDR10, for the display to map. Dolby Vision always goes out as SDR BT.2020.

- In the file: `mode` under `[output]`
- Values: sdr2020 | passthrough
- Default: `"passthrough"`
- Applies: immediately, with a resync

### output.white_nits {#output-white-nits}

The brightness, in nits, that SDR output's white stands for: HDR above it is tone-mapped. Also the peak a player's low-latency Dolby Vision is mapped to, which passes one to one: set it to what the EDID upstream declares (96 for the usual low-latency Dolby Vision EDIDs).

- In the file: `white_nits` under `[output]`
- Values: 48-1000
- Default: `96`
- Applies: immediately

### output.wait_hz {#output-wait-hz}

The output's rate while no source is playing, with black on screen. A source that starts at the same rate (and SDR) comes up without a resync: 23.976 for an Apple TV set to 23.976, 59.94 for a player whose menus are 59.94.

- In the file: `wait_hz` under `[output]`
- Values: 23-61
- Default: `23.976`
- Applies: immediately, with a resync

## [input]

### input.rgb_range {#input-rgb-range}

What plain RGB input's codes mean: limited (16-235, HDMI's default for video) or full (0-255). The capture card does not report it. YCbCr and Dolby Vision are not affected.

- In the file: `rgb_range` under `[input]`
- Values: limited | full
- Default: `"limited"`
- Applies: immediately

### input.lldv {#input-lldv}

on: YCbCr input labelled SDR BT.2020 is a player's low-latency Dolby Vision, PQ already mapped to output.white_nits. The capture card cannot tell the two apart, and SDR discs are BT.709.

- In the file: `lldv` under `[input]`
- Values: on | off
- Default: `"on"`
- Applies: immediately

### input.device {#input-device}

The capture device, by the persistent ID `cinefit probe` prints (for example 0x1620413): on a card with several inputs, which one. The SDK's list order can change across reboots and driver updates, so not an index. An ID that is not present stops the run with the list. Empty: the first device that can capture.

- In the file: `device` under `[input]`
- Values: text, up to 20 characters
- Default: empty
- Applies: when cinefit starts

## [tonemap]

### tonemap.curve {#tonemap-curve}

The tone curve for HDR into SDR output: libplacebo's, by name.

- In the file: `curve` under `[tonemap]`
- Values: spline | hable | bt2390 | bt2446a | st2094-10 | st2094-40 | mobius | reinhard | clip
- Default: `"spline"`
- Applies: immediately

### tonemap.source_peak {#tonemap-source-peak}

The HDR10 source's peak, in nits, that the curve maps from, in place of its MaxCLL or mastering peak; 0 uses the metadata.

- In the file: `source_peak` under `[tonemap]`
- Values: 0-10000
- Default: `0`
- Applies: immediately

## [detect]

### detect.crop {#detect-crop}

on: find the black bars and crop them out, the picture placed at constant image height in the screen's band of the raster. off: every frame fills the raster as it came.

- In the file: `crop` under `[detect]`
- Values: on | off
- Default: `"on"`
- Applies: immediately

### detect.record {#detect-record}

on: record what the bar detector measured on every frame shown, and a small fingerprint of the picture, under ~/cinefit-clips/caught, for tuning the detector (about 150 MB a film). Full frames only where they teach something, within detect.frames_gb.

- In the file: `record` under `[detect]`
- Values: on | off
- Default: `"on"`
- Applies: immediately

### detect.frames_gb {#detect-frames-gb}

While recording: the disk, in GB, that full frames may use, every session together. Frames are kept where the detector changed the crop or hesitated, and a few at random; the oldest sessions' are deleted first. 0: no full frames, only the readings.

- In the file: `frames_gb` under `[detect]`
- Values: 0-10000
- Default: `20`
- Applies: immediately

### detect.pair_every {#detect-pair-every}

For pairing two passes of a title (Dolby Vision tunnel, then low-latency): keep in full the first frame of every shot and every this many frames after it, and no other frames, so both passes keep the same frames of the film. 48 at 24p is one every two seconds; a tunnel frame is about 15 MB kept, so mind detect.frames_gb. 0: off, frames kept as usual.

- In the file: `pair_every` under `[detect]`
- Values: 0-100000
- Default: `0`
- Applies: immediately

## [geometry]

### geometry.screen_ar {#geometry-screen-ar}

The screen's aspect ratio: with the lens set so the raster's width fills the screen, the band of the raster that lands on it. 2.35 for a 2.35:1 screen; a ratio may be written "16:9".

- In the file: `screen_ar` under `[geometry]`
- Values: 1-3, or W:H
- Default: `2.35`
- Applies: immediately

### geometry.fill_max_ar {#geometry-fill-max-ar}

Content wider than the screen but no wider than this fills the screen's height, losing a sliver at each side (2.39 and 2.40 on a 2.35 screen give up about 1% a side and show no bars). Wider content (2.55, 2.76) keeps its full width with thin bars inside the screen. 0 is off.

- In the file: `fill_max_ar` under `[geometry]`
- Values: 0 (off), 1-3, or W:H
- Default: `2.4`
- Applies: immediately

## [slate]

### slate.venue {#slate-venue}

The theater's name, shown on the waiting slate above what cinefit is waiting for. Empty: no name, only the state.

- In the file: `venue` under `[slate]`
- Values: text, up to 40 characters
- Default: empty
- Applies: immediately

## [osd]

### osd.banners {#osd-banners}

on: a short banner when the picture type changes (Dolby Vision, HDR10, SDR), saying what came in and what happens to it; after the projector's resync when the change needs one.

- In the file: `banners` under `[osd]`
- Values: on | off
- Default: `"on"`
- Applies: immediately

## [avr]

### avr.denon {#avr-denon}

The Denon receiver, by hostname or IP address (for example denon.local): cinefit reads which input is on, by its name on the receiver, and whether the receiver is on, for the info panel, the slate and the detector's dataset. Reads only. Empty: none.

- In the file: `denon` under `[avr]`
- Values: text, up to 253 characters
- Default: empty
- Applies: immediately

## [atv]

### atv.host {#atv-host}

The Apple TV, by hostname or IP address: cinefit reads what it is playing (the title, the app, where in it), for the info panel and the detector's dataset. Reads only. It needs pairing once; the pairing is kept in atv-credentials beside this file. Some apps say nothing of what they play. Empty: none.

- In the file: `host` under `[atv]`
- Values: text, up to 253 characters
- Default: empty
- Applies: immediately

### atv.input {#atv-input}

The Denon input the Apple TV is plugged into, by its name on the receiver or its code (for example Apple TV, or MPLAY): what the Apple TV plays is shown, and labels the detector's dataset, only while the receiver is on it, so a paused show does not linger over a disc. Empty: always, as without a receiver.

- In the file: `input` under `[atv]`
- Values: text, up to 64 characters
- Default: empty
- Applies: immediately

## [web]

### web.port {#web-port}

The port of the web page with cinefit's status and settings, open to the home network: http://&lt;this machine>:8080. 0: no web page. Above 1023, as it runs without root.

- In the file: `port` under `[web]`
- Values: 0-65535
- Default: `8080`
- Applies: when cinefit starts
