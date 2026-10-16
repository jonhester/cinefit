---
title: "Remote and menu"
description: "Using cinefit day to day: the remote's buttons, the on-screen menu, the info panel, banners, the waiting slate and the web page."
---

# Remote and menu

## Buttons

cinefit reads any USB keyboard-like device with arrow keys: a keyboard, or an IR receiver such as a flirc that types keys for a remote. These keys are taken, so they do not also reach the console behind the picture.

| Action | Keys |
|---|---|
| Up / down / left / right | Arrow keys |
| Select | Enter, `E` |
| Back | Backspace, Esc, `B` |
| Menu | `C`, `M` |
| Info | `I`, `D` |

Program your remote's buttons to send these keys (with a flirc, a media-center profile covers the arrows, Enter and Back; add `M` or `C` for Menu and `I` for Info). To see what each button sends:

```sh
cinefit keys
```

It lists each press for 30 seconds. While it runs it takes the keys from the service.

## The menu

**Menu** opens it.

1. **Up / Down** pick a page: Picture, Output, Screen, Input.
2. **Right** or **Select** enters the page; **Up / Down** pick a row.
3. **Left / Right** change the value. A change applies at once and is saved to the settings file.
4. **Back** returns to the pages, and Back or Menu again closes it. It closes itself after a minute without a key.

The bottom of the menu says what a change costs: **Applies now, no resync**, **Resyncs the projector** (the display blanks for a few seconds), or **Set in the settings file**.

| Page | Rows |
|---|---|
| Picture | Crop black bars, Fill the screen up to, Banners |
| Output | HDR10 (to the projector, or tone mapped to SDR BT.2020), Rate while waiting |
| Screen | Screen ratio; panel size and theater name, shown only |
| Input | RGB range, Dolby Vision low latency |

Settings not in the menu are on the web page and in the [settings file](/settings/).

## The info panel

**Info** shows what cinefit is doing, refreshed every second, and closes after 30 seconds or on **Back**:

- **In**: the picture type (Dolby Vision tunnel or low latency, HDR10, HLG, SDR), size, rate, format and the brightest level in the current picture. With a Denon receiver, the input's name first.
- **Playing**: with an Apple TV, the title, app and position.
- **Out**: what goes to the display and what was done to it (passed through, tone mapped, placed in BT.2020).
- **Crop**: the picture's shape and when it last changed, or why the detector cannot read the input.
- **Last 10 s**: frames shown and repeated, and the time each frame took.

## Banners and the slate

A **banner** appears for a few seconds when the picture type changes, saying what came in and what happens to it. When the change needs a resync it waits until the display shows a picture again. Turn banners off with [`osd.banners`](/settings/#osd-banners).

The **slate** is shown whenever there is no picture: **No signal**, **Signal lost** (and how long ago), **Receiver off**, **Identifying the signal...**, or **Can't show this format**. It moves to another corner every five minutes. Set [`slate.venue`](/settings/#slate-venue) to put your theater's name above it.

## The web page

Open `http://<the cinefit PC>:8080` on a phone or computer on the same network. It shows the same status as the info panel, every setting (including those not in the menu) with what changing it costs, Apple TV pairing, and the logs of recent runs.

::: callout warning "No password"
The web page has no login: anyone on your network can change settings. Change the port with [`web.port`](/settings/#web-port), or set it to 0 to turn the page off.
:::
