---
title: "Picture types"
description: "What cinefit does with HDR10, Dolby Vision (tunnel and low latency), SDR, HLG and RGB input, the two output modes, resyncs, and constant image height cropping."
---

# Picture types

## Output modes

[`output.mode`](/settings/#output-mode) chooses what HDR10 becomes:

- **`passthrough`** (default): HDR10 goes out as HDR10 with the source's metadata, and the display does its own tone mapping (a JVC's Frame Adapt HDR, for example).
- **`sdr2020`**: cinefit tone maps HDR10 down to SDR in a BT.2020 container, white at [`output.white_nits`](/settings/#output-white-nits). Set the display to its SDR BT.2020 picture mode. The curve is [`tonemap.curve`](/settings/#tonemap-curve).

Dolby Vision always goes out as SDR BT.2020, whatever the mode.

## Each input

| Input | Output |
|---|---|
| **HDR10** | As the output mode says. |
| **HLG** | Passed through as HLG. |
| **Dolby Vision, tunneled** (arrives as 10-bit RGB) | cinefit decodes the Dolby Vision metadata from the pixels and maps it to SDR BT.2020 itself, scene by scene. While it checks whether a new 10-bit RGB signal is a tunnel (up to two seconds), the slate says **Identifying the signal...** over black. |
| **Dolby Vision, low latency** (LLDV) | The player has already mapped it to the peak the EDID declared. cinefit passes it one to one, with [`output.white_nits`](/settings/#output-white-nits) set to that peak. |
| **SDR** | Placed in BT.2020 and scaled to the panel. |
| **RGB** (not a tunnel) | Rendered; its range comes from [`input.rgb_range`](/settings/#input-rgb-range) because the card does not report it. The crop is not detected. |

::: callout warning "Real SDR BT.2020 sources"
The capture card cannot tell low-latency Dolby Vision from SDR labelled BT.2020: both arrive the same way. With [`input.lldv`](/settings/#input-lldv) on (the default), cinefit treats such a signal as LLDV. Discs' SDR is BT.709 and unaffected. If a source really sends SDR BT.2020, turn `input.lldv` off.
:::

## Resyncs {#resyncs}

A change of frame rate, or between SDR, HDR10 and HLG, means the output is set up again, and the display blanks for a few seconds while it locks on (2-6 s on the projector it was measured on). cinefit waits for the new signal to hold for half a second before doing it. Changes of HDR metadata alone (a new title's MaxCLL) go out with the next frame, with no resync.

While waiting with no source, cinefit holds the output at [`output.wait_hz`](/settings/#output-wait-hz). A source that starts at that rate, in SDR, comes up without a resync: 23.976 suits an Apple TV set to match frame rate, 59.94 a disc player whose menus run at 59.94.

## Cropping and constant image height

With the lens set so the picture's width fills the screen, only a band of the panel lands on a scope screen: [`geometry.screen_ar`](/settings/#geometry-screen-ar) says which. cinefit finds the black bars in each picture and scales the picture so it fills that band's height:

- **16:9 and 1.85 content** is shown at the screen's full height, with black at the sides.
- **Content as wide as the screen** fills it.
- **Content slightly wider** than the screen, up to [`geometry.fill_max_ar`](/settings/#geometry-fill-max-ar) (2.40 by default), also fills the height, losing about 1% at each side, so no thin bars remain.
- **Much wider content** (2.55, 2.76) keeps its whole width, with thin bars inside the screen.

Player menus and on-screen controls are never cropped. Subtitles placed in the black bars are cropped with them. Turn cropping off with [`detect.crop`](/settings/#detect-crop): every frame then fills the panel as it came.
