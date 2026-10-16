---
title: "Settings"
description: "Every cinefit setting: what it does, its values, its default and what changing it costs."
---

<!-- Generated from crates/cinefit/src/config.rs: edit the table there, then run CINEFIT_UPDATE_GUIDE=1 cargo test -p cinefit config -->

# Settings

Every setting, grouped as on the [web page](/web-page/), where all of them can be changed. Those marked **in the menu** are on the remote's [menu](/using/#menu) too. Both save to the settings file, `/etc/cinefit/cinefit.toml`, which holds only what you have changed; `cinefit settings` lists every setting with its current value.

A change costs one of three things:

- **applies now**: the next frame uses it.
- **resyncs the display**: the display blanks for a few seconds while it locks on.
- **applies at the next start**: restart the service (`sudo systemctl restart cinefit`).

## Picture

### Crop black bars {#detect-crop}

on: find the black bars and crop them out, the picture placed at constant image height in the screen's band of the raster. off: every frame fills the raster as it came.

`detect.crop` · `on` or `off` · default `on` · applies now · **in the menu**

### Fill the screen up to {#geometry-fill-max-ar}

Content wider than the screen but no wider than this fills the screen's height, losing a sliver at each side (2.39 and 2.40 on a 2.35 screen give up about 1% a side and show no bars). Wider content (2.55, 2.76) keeps its full width with thin bars inside the screen. 0 is off.

`geometry.fill_max_ar` · 1 to 3, as a number or W:H, or 0 for off · default `2.4` · applies now · **in the menu**

### Banners {#osd-banners}

on: a short banner when the picture type changes (Dolby Vision, HDR10, SDR), saying what came in and what happens to it; after the projector's resync when the change needs one.

`osd.banners` · `on` or `off` · default `on` · applies now · **in the menu**

## Output

### HDR10 {#output-mode}

sdr2020: SDR in a BT.2020 container, HDR tone-mapped down to output.white_nits; set the display to an SDR BT.2020 mode. passthrough: HDR10 goes out as HDR10, for the display to map. Dolby Vision always goes out as SDR BT.2020.

`output.mode` · `sdr2020` or `passthrough` · default `passthrough` · resyncs the display · **in the menu**

### SDR white and low-latency Dolby Vision, nits {#output-white-nits}

The brightness, in nits, that SDR output's white stands for: HDR above it is tone-mapped. Also the peak a player's low-latency Dolby Vision is mapped to, which passes one to one: set it to what the EDID upstream declares (96 for the usual low-latency Dolby Vision EDIDs).

`output.white_nits` · 48 to 1000 · default `96` · applies now

### Rate while waiting {#output-wait-hz}

The output's rate while no source is playing, with black on screen. A source that starts at the same rate (and SDR) comes up without a resync: 23.976 for an Apple TV set to 23.976, 59.94 for a player whose menus are 59.94.

`output.wait_hz` · 23 to 61 · default `23.976` · resyncs the display · **in the menu**

### Tone curve {#tonemap-curve}

The tone curve for HDR into SDR output: libplacebo's, by name.

`tonemap.curve` · `spline`, `hable`, `bt2390`, `bt2446a`, `st2094-10`, `st2094-40`, `mobius`, `reinhard` or `clip` · default `spline` · applies now

### HDR10 source peak, nits (0: from the disc) {#tonemap-source-peak}

The HDR10 source's peak, in nits, that the curve maps from, in place of its MaxCLL or mastering peak; 0 uses the metadata.

`tonemap.source_peak` · 0 to 10000 · default `0` · applies now

## Screen

### Panel width {#output-width}

The panel's width in pixels: the output's size. 4096 for a 17:9 4K projector.

`output.width` · 640 to 8192 · default `4096` · applies at the next start

### Panel height {#output-height}

The panel's height in pixels.

`output.height` · 480 to 4320 · default `2160` · applies at the next start

### Screen ratio {#geometry-screen-ar}

The screen's aspect ratio: with the lens set so the raster's width fills the screen, the band of the raster that lands on it. 2.35 for a 2.35:1 screen; a ratio may be written "16:9".

`geometry.screen_ar` · 1 to 3, as a number or W:H · default `2.35` · applies now · **in the menu**

### Theater name {#slate-venue}

The theater's name, shown on the waiting slate above what cinefit is waiting for. Empty: no name, only the state.

`slate.venue` · text, up to 40 characters · default empty · applies now

## Input

### RGB range {#input-rgb-range}

What plain RGB input's codes mean: limited (16-235, HDMI's default for video) or full (0-255). The capture card does not report it. YCbCr and Dolby Vision are not affected.

`input.rgb_range` · `limited` or `full` · default `limited` · applies now · **in the menu**

### Dolby Vision low latency {#input-lldv}

on: YCbCr input labelled SDR BT.2020 is a player's low-latency Dolby Vision, PQ already mapped to output.white_nits. The capture card cannot tell the two apart, and SDR discs are BT.709.

`input.lldv` · `on` or `off` · default `on` · applies now · **in the menu**

### Capture device (persistent ID) {#input-device}

The capture device, by the persistent ID `cinefit probe` prints (for example 0x1620413): on a card with several inputs, which one. The SDK's list order can change across reboots and driver updates, so not an index. An ID that is not present stops the run with the list. Empty: the first device that can capture.

`input.device` · text, up to 20 characters · default empty · applies at the next start

## Receiver

For the [receiver and Apple TV](/integrations/) integrations.

### Denon AVR (hostname or IP) {#avr-denon}

The Denon receiver, by hostname or IP address (for example denon.local): cinefit reads which input is on, by its name on the receiver, and whether the receiver is on, for the info panel, the slate and the detector's dataset. Reads only. Empty: none.

`avr.denon` · text, up to 253 characters · default empty · applies now

### Apple TV (hostname or IP) {#atv-host}

The Apple TV, by hostname or IP address: cinefit reads what it is playing (the title, the app, where in it), for the info panel and the detector's dataset. Reads only. It needs pairing once; the pairing is kept in atv-credentials beside this file. Some apps say nothing of what they play. Empty: none.

`atv.host` · text, up to 253 characters · default empty · applies now

### Apple TV's receiver input {#atv-input}

The Denon input the Apple TV is plugged into, by its name on the receiver or its code (for example Apple TV, or MPLAY): what the Apple TV plays is shown, and labels the detector's dataset, only while the receiver is on it, so a paused show does not linger over a disc. Empty: always, as without a receiver.

`atv.input` · text, up to 64 characters · default empty · applies now

## Web page

### Port {#web-port}

The port of the web page with cinefit's status and settings, open to the home network: http://&lt;this machine>:8080. 0: no web page. Above 1023, as it runs without root.

`web.port` · 0 to 65535 · default `8080` · applies at the next start

## Detector tuning

For improving cinefit's bar detector; most people leave these alone. Recording is on by default and keeps what the detector measured, a little each film, and some frames within a disk limit; turn it off with the first setting here.

### Record for detector tuning {#detect-record}

on: record what the bar detector measured on every frame shown, and a small fingerprint of the picture, under ~/cinefit-clips/caught, for tuning the detector (about 150 MB a film). Full frames only where they teach something, within detect.frames_gb.

`detect.record` · `on` or `off` · default `on` · applies now

### Disk for kept frames, GB (0: none) {#detect-frames-gb}

While recording: the disk, in GB, that full frames may use, every session together. Frames are kept where the detector changed the crop or hesitated, and a few at random; the oldest sessions' are deleted first. 0: no full frames, only the readings.

`detect.frames_gb` · 0 to 10000 · default `20` · applies now

### Pairing passes: keep every Nth frame of each shot (0: off) {#detect-pair-every}

For pairing two passes of a title (Dolby Vision tunnel, then low-latency): keep in full the first frame of every shot and every this many frames after it, and no other frames, so both passes keep the same frames of the film. 48 at 24p is one every two seconds; a tunnel frame is about 15 MB kept, so mind detect.frames_gb. 0: off, frames kept as usual.

`detect.pair_every` · 0 to 100000 · default `0` · applies now
