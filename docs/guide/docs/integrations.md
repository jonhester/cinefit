---
title: "Receiver and Apple TV"
description: "Optional integrations: reading a Denon receiver's input and power, and an Apple TV's now-playing information, for the info panel, banners and slate."
---

# Receiver and Apple TV

Both integrations only read. cinefit never switches inputs or controls playback.

## Denon receiver

Set [`avr.denon`](/settings/#avr-denon) to the receiver's hostname or IP address (on the web page under Receiver, or in the settings file). cinefit then reads, over the network:

- **Which input is on**, by the name you gave it on the receiver: shown on banners, the info panel and the slate (**Signal lost · waiting for Apple TV**).
- **Whether the receiver is on**: the slate shows **Receiver off** instead of **No signal**.

The receiver's network control must be on (it usually is). Only Denon receivers are supported; other brands are untested and likely do not answer.

## Apple TV

Set [`atv.host`](/settings/#atv-host) to the Apple TV's hostname or IP address, then pair once on the web page:

1. Under **Receiver**, find **Apple TV pairing** and press **Pair**. The Apple TV shows a PIN.
2. Type the PIN in **PIN on the TV** and press **Pair with this PIN**.

cinefit keeps the pairing in `~/.config/cinefit-rs/atv-credentials` (readable only by your user; it holds a secret key). Pair again only if the Apple TV forgets cinefit.

Once paired, the info panel's **Playing** row shows the title, app and position. Some apps report nothing about what they play.

With a Denon receiver too, set [`atv.input`](/settings/#atv-input) to the receiver input the Apple TV is on (its name, such as `Apple TV`, or its code, such as `MPLAY`). The Apple TV's information is then shown only while that input is selected, so a paused show does not linger over a disc.
