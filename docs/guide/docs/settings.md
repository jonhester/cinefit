---
title: "Settings"
description: "Every cinefit setting: what it does, its values, its default and what changing it costs."
---

<!-- Generated from crates/cinefit/src/config.rs: edit the table there, then run CINEFIT_UPDATE_GUIDE=1 cargo test -p cinefit config -->

# Settings

Every setting, grouped as on the [web page](/web-page/), where all of them can be changed. Those tagged **In the menu** are on the remote's [menu](/using/#menu) too. Both save to the settings file, `/etc/cinefit/cinefit.toml`, which holds only what you have changed; `cinefit settings` lists every setting with its current value.

Each setting's tag says what a change costs:

- **Applies now**: the next frame uses it.
- **Resyncs the display**: the display blanks for a few seconds while it locks on.
- **At the next start**: restart the service (`sudo systemctl restart cinefit`).

## Picture

### Crop black bars {#detect-crop}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span><span class="tag tag-menu">In the menu</span></div><ul><li><code>on</code>: find the black bars and crop them out, the picture placed at constant image height in the screen's band of the raster.</li><li><code>off</code>: every frame fills the raster as it came.</li></ul><dl><div><dt>Values</dt><dd><code>on</code> or <code>off</code></dd></div><div><dt>Default</dt><dd><code>on</code></dd></div><div><dt>Key</dt><dd><code>detect.crop</code></dd></div></dl></div>

### Fill the screen up to {#geometry-fill-max-ar}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span><span class="tag tag-menu">In the menu</span></div><p>Content wider than the screen but no wider than this fills the screen's height, losing a sliver at each side (2.39 and 2.40 on a 2.35 screen give up about 1% a side and show no bars). Wider content (2.55, 2.76) keeps its full width with thin bars inside the screen. 0 is off.</p><dl><div><dt>Values</dt><dd>1 to 3, as a number or W:H; 0 is off</dd></div><div><dt>Default</dt><dd><code>2.4</code></dd></div><div><dt>Key</dt><dd><code>geometry.fill_max_ar</code></dd></div></dl></div>

### Banners {#osd-banners}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span><span class="tag tag-menu">In the menu</span></div><p>on: a short banner when the picture type changes (Dolby Vision, HDR10, SDR), saying what came in and what happens to it; after the projector's resync when the change needs one.</p><dl><div><dt>Values</dt><dd><code>on</code> or <code>off</code></dd></div><div><dt>Default</dt><dd><code>on</code></dd></div><div><dt>Key</dt><dd><code>osd.banners</code></dd></div></dl></div>

## Output

### HDR10 {#output-mode}

<div class="setting"><div class="tags"><span class="tag tag-resync">Resyncs the display</span><span class="tag tag-menu">In the menu</span></div><ul><li><code>sdr2020</code>: SDR in a BT.2020 container, HDR tone-mapped down to output.white_nits; set the display to an SDR BT.2020 mode.</li><li><code>passthrough</code>: HDR10 goes out as HDR10, for the display to map. Dolby Vision always goes out as SDR BT.2020.</li></ul><dl><div><dt>Values</dt><dd><code>sdr2020</code> or <code>passthrough</code></dd></div><div><dt>Default</dt><dd><code>passthrough</code></dd></div><div><dt>Key</dt><dd><code>output.mode</code></dd></div></dl></div>

### SDR white and low-latency Dolby Vision, nits {#output-white-nits}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span></div><p>The brightness, in nits, that SDR output's white stands for: HDR above it is tone-mapped. Also the peak a player's low-latency Dolby Vision is mapped to, which passes one to one: set it to what the EDID upstream declares (96 for the usual low-latency Dolby Vision EDIDs).</p><dl><div><dt>Values</dt><dd>48 to 1000</dd></div><div><dt>Default</dt><dd><code>96</code></dd></div><div><dt>Key</dt><dd><code>output.white_nits</code></dd></div></dl></div>

### Rate while waiting {#output-wait-hz}

<div class="setting"><div class="tags"><span class="tag tag-resync">Resyncs the display</span><span class="tag tag-menu">In the menu</span></div><p>The output's rate while no source is playing, with black on screen. A source that starts at the same rate (and SDR) comes up without a resync: 23.976 for an Apple TV set to 23.976, 59.94 for a player whose menus are 59.94.</p><dl><div><dt>Values</dt><dd>23 to 61</dd></div><div><dt>Default</dt><dd><code>23.976</code></dd></div><div><dt>Key</dt><dd><code>output.wait_hz</code></dd></div></dl></div>

### Tone curve {#tonemap-curve}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span></div><p>The tone curve for HDR into SDR output: libplacebo's, by name.</p><dl><div><dt>Values</dt><dd><code>spline</code>, <code>hable</code>, <code>bt2390</code>, <code>bt2446a</code>, <code>st2094-10</code>, <code>st2094-40</code>, <code>mobius</code>, <code>reinhard</code> or <code>clip</code></dd></div><div><dt>Default</dt><dd><code>spline</code></dd></div><div><dt>Key</dt><dd><code>tonemap.curve</code></dd></div></dl></div>

### HDR10 source peak, nits (0: from the disc) {#tonemap-source-peak}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span></div><p>The HDR10 source's peak, in nits, that the curve maps from, in place of its MaxCLL or mastering peak; 0 uses the metadata.</p><dl><div><dt>Values</dt><dd>0 to 10000</dd></div><div><dt>Default</dt><dd><code>0</code></dd></div><div><dt>Key</dt><dd><code>tonemap.source_peak</code></dd></div></dl></div>

## Screen

### Panel width {#output-width}

<div class="setting"><div class="tags"><span class="tag tag-start">At the next start</span></div><p>The panel's width in pixels: the output's size. 4096 for a 17:9 4K projector.</p><dl><div><dt>Values</dt><dd>640 to 8192</dd></div><div><dt>Default</dt><dd><code>4096</code></dd></div><div><dt>Key</dt><dd><code>output.width</code></dd></div></dl></div>

### Panel height {#output-height}

<div class="setting"><div class="tags"><span class="tag tag-start">At the next start</span></div><p>The panel's height in pixels.</p><dl><div><dt>Values</dt><dd>480 to 4320</dd></div><div><dt>Default</dt><dd><code>2160</code></dd></div><div><dt>Key</dt><dd><code>output.height</code></dd></div></dl></div>

### Screen ratio {#geometry-screen-ar}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span><span class="tag tag-menu">In the menu</span></div><p>The screen's aspect ratio: with the lens set so the raster's width fills the screen, the band of the raster that lands on it. 2.35 for a 2.35:1 screen; a ratio may be written "16:9".</p><dl><div><dt>Values</dt><dd>1 to 3, as a number or W:H</dd></div><div><dt>Default</dt><dd><code>2.35</code></dd></div><div><dt>Key</dt><dd><code>geometry.screen_ar</code></dd></div></dl></div>

### Theater name {#slate-venue}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span></div><p>The theater's name, shown on the waiting slate above what cinefit is waiting for. Empty: no name, only the state.</p><dl><div><dt>Values</dt><dd>text, up to 40 characters</dd></div><div><dt>Default</dt><dd>empty</dd></div><div><dt>Key</dt><dd><code>slate.venue</code></dd></div></dl></div>

## Input

### RGB range {#input-rgb-range}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span><span class="tag tag-menu">In the menu</span></div><p>What plain RGB input's codes mean: limited (16-235, HDMI's default for video) or full (0-255). The capture card does not report it. YCbCr and Dolby Vision are not affected.</p><dl><div><dt>Values</dt><dd><code>limited</code> or <code>full</code></dd></div><div><dt>Default</dt><dd><code>limited</code></dd></div><div><dt>Key</dt><dd><code>input.rgb_range</code></dd></div></dl></div>

### Dolby Vision low latency {#input-lldv}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span><span class="tag tag-menu">In the menu</span></div><p>on: YCbCr input labelled SDR BT.2020 is a player's low-latency Dolby Vision, PQ already mapped to output.white_nits. The capture card cannot tell the two apart, and SDR discs are BT.709.</p><dl><div><dt>Values</dt><dd><code>on</code> or <code>off</code></dd></div><div><dt>Default</dt><dd><code>on</code></dd></div><div><dt>Key</dt><dd><code>input.lldv</code></dd></div></dl></div>

### Capture device (persistent ID) {#input-device}

<div class="setting"><div class="tags"><span class="tag tag-start">At the next start</span></div><p>The capture device, by the persistent ID `cinefit probe` prints (for example 0x1620413): on a card with several inputs, which one. The SDK's list order can change across reboots and driver updates, so not an index. An ID that is not present stops the run with the list. Empty: the first device that can capture.</p><dl><div><dt>Values</dt><dd>text, up to 20 characters</dd></div><div><dt>Default</dt><dd>empty</dd></div><div><dt>Key</dt><dd><code>input.device</code></dd></div></dl></div>

## Receiver

For the [receiver and Apple TV](/integrations/) integrations.

### Denon AVR (hostname or IP) {#avr-denon}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span></div><p>The Denon receiver, by hostname or IP address (for example denon.local): cinefit reads which input is on, by its name on the receiver, and whether the receiver is on, for the info panel, the slate and the detector's dataset. Reads only. Empty: none.</p><dl><div><dt>Values</dt><dd>text, up to 253 characters</dd></div><div><dt>Default</dt><dd>empty</dd></div><div><dt>Key</dt><dd><code>avr.denon</code></dd></div></dl></div>

### Apple TV (hostname or IP) {#atv-host}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span></div><p>The Apple TV, by hostname or IP address: cinefit reads what it is playing (the title, the app, where in it), for the info panel and the detector's dataset. Reads only. It needs pairing once; the pairing is kept in atv-credentials beside this file. Some apps say nothing of what they play. Empty: none.</p><dl><div><dt>Values</dt><dd>text, up to 253 characters</dd></div><div><dt>Default</dt><dd>empty</dd></div><div><dt>Key</dt><dd><code>atv.host</code></dd></div></dl></div>

### Apple TV's receiver input {#atv-input}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span></div><p>The Denon input the Apple TV is plugged into, by its name on the receiver or its code (for example Apple TV, or MPLAY): what the Apple TV plays is shown, and labels the detector's dataset, only while the receiver is on it, so a paused show does not linger over a disc. Empty: always, as without a receiver.</p><dl><div><dt>Values</dt><dd>text, up to 64 characters</dd></div><div><dt>Default</dt><dd>empty</dd></div><div><dt>Key</dt><dd><code>atv.input</code></dd></div></dl></div>

## Web page

### Port {#web-port}

<div class="setting"><div class="tags"><span class="tag tag-start">At the next start</span></div><p>The port of the web page with cinefit's status and settings, open to the home network: http://&lt;this machine&gt;:8080. 0: no web page. Above 1023, as it runs without root.</p><dl><div><dt>Values</dt><dd>0 to 65535</dd></div><div><dt>Default</dt><dd><code>8080</code></dd></div><div><dt>Key</dt><dd><code>web.port</code></dd></div></dl></div>

## Detector tuning

For improving cinefit's bar detector; most people leave these alone. Recording is on by default and keeps what the detector measured, a little each film, and some frames within a disk limit; turn it off with the first setting here.

### Record for detector tuning {#detect-record}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span></div><p>on: record what the bar detector measured on every frame shown, and a small fingerprint of the picture, under ~/cinefit-clips/caught, for tuning the detector (about 150 MB a film). Full frames only where they teach something, within detect.frames_gb.</p><dl><div><dt>Values</dt><dd><code>on</code> or <code>off</code></dd></div><div><dt>Default</dt><dd><code>on</code></dd></div><div><dt>Key</dt><dd><code>detect.record</code></dd></div></dl></div>

### Disk for kept frames, GB (0: none) {#detect-frames-gb}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span></div><p>While recording: the disk, in GB, that full frames may use, every session together. Frames are kept where the detector changed the crop or hesitated, and a few at random; the oldest sessions' are deleted first. 0: no full frames, only the readings.</p><dl><div><dt>Values</dt><dd>0 to 10000</dd></div><div><dt>Default</dt><dd><code>20</code></dd></div><div><dt>Key</dt><dd><code>detect.frames_gb</code></dd></div></dl></div>

### Pairing passes: keep every Nth frame of each shot (0: off) {#detect-pair-every}

<div class="setting"><div class="tags"><span class="tag tag-now">Applies now</span></div><p>For pairing two passes of a title (Dolby Vision tunnel, then low-latency): keep in full the first frame of every shot and every this many frames after it, and no other frames, so both passes keep the same frames of the film. 48 at 24p is one every two seconds; a tunnel frame is about 15 MB kept, so mind detect.frames_gb. 0: off, frames kept as usual.</p><dl><div><dt>Values</dt><dd>0 to 100000</dd></div><div><dt>Default</dt><dd><code>0</code></dd></div><div><dt>Key</dt><dd><code>detect.pair_every</code></dd></div></dl></div>
