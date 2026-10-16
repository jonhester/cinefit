---
title: "On screen"
description: "What cinefit puts on the screen: the info panel the remote's Info button opens, the banner on each change of picture type, and the slate while nothing is playing."
---

# On screen

Besides the [menu](/using/#menu), cinefit draws three things over the picture or in its place.

## The info panel

**Info** on the [remote](/using/) shows what cinefit is doing, refreshed every second, and closes after 30 seconds or on **Back**:

<figure class="shot"><img src="/assets/images/osd-info.png" alt="The info panel: IN Dolby Vision low latency, 3840 x 2160, 23.976, YCbCr 4:2:2 10-bit; OUT SDR BT.2020, unchanged from the player; CROP 2.40:1, 280 / 279 rows, zoom in 12 s ago; LAST 10 S 240 shown, 0 repeated, frame work 7.9 ms of 41.7" width="1143" height="1090" loading="lazy"><figcaption>The info panel as cinefit draws it, over a stand-in picture: low-latency Dolby Vision coming in, a 2.40:1 film cropped, no frames repeated.</figcaption></figure>

- **In**: the picture type (Dolby Vision tunnel or low latency, HDR10, HLG, SDR), size, rate, format and the brightest level in the current picture. With a Denon receiver, the input's name first.
- **Playing**: with an Apple TV, the title, app and position.
- **Out**: what goes to the display and what was done to it (passed through, tone mapped, placed in BT.2020).
- **Crop**: the picture's shape and when it last changed, or why the detector cannot read the input.
- **Last 10 s**: frames shown and repeated, and the time each frame took.

## Banners

A **banner** appears for a few seconds when the picture type changes, saying what came in and what happens to it. When the change needs a resync it waits until the display shows a picture again. Turn banners off with [`osd.banners`](/settings/#osd-banners).

<figure class="shot"><img src="/assets/images/osd-banner.png" alt="A banner in the top left corner of the picture: Dolby Vision, and under it 4K 23.976, mapped by the player, shown as is" width="2304" height="922" loading="lazy"><figcaption>A banner as low-latency Dolby Vision starts, in the top left of the picture, as cinefit draws it.</figcaption></figure>

## The waiting slate

The **slate** is shown whenever there is no picture: **No signal**, **Signal lost** (and how long ago), **Receiver off**, **Identifying the signal...**, or **Can't show this format**. It sits small in one corner and moves to another every five minutes, so nothing stays on the screen in one place.

### Your theater's name

Above the state, the slate shows your theater's name, where the picture below says **Your theater's name here**. Set it on the [web page](/web-page/) (**Screen**, then **Theater name**) or as [`slate.venue`](/settings/#slate-venue) in the settings file, up to 40 characters. It changes at once, with no restart. Left empty, the slate shows only the state.

<figure class="shot"><img src="/assets/images/osd-slate.png" alt="The slate on a black screen: Your theater's name here, and under it No signal, small, in the lower left corner" width="2400" height="1021" loading="lazy"><figcaption>The slate while nothing is playing, across the whole screen: the theater's name you set, and the state under it, small, in one corner.</figcaption></figure>
