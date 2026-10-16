---
title: "The signal chain"
description: "How sources, an AV receiver, an HDMI splitter and the DeckLink capture card connect for cinefit, what signal the card accepts, and which EDID choices decide HDR10, Dolby Vision tunnel or LLDV."
---

# The signal chain

<!-- fig:chain -->
<figure class="fig"><div class="scroll"><svg viewBox="0 0 760 326" role="img" aria-label="The signal chain: player, optional receiver, HDMI splitter, then the capture card and graphics card in the cinefit PC, then the projector; the splitter presents the EDID the player reads" font-size="13"><defs><marker id="ah-chain" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" class="ah"/></marker></defs><text x="20" y="18" text-anchor="start" class="t b" font-size="15">The picture goes one way; the EDID goes the other</text><path d="M435 72 V46 H90 V70" class="ln" stroke-dasharray="4 3" marker-end="url(#ah-chain)"/><text x="262" y="40" text-anchor="middle" class="ht">the EDID: what the player may send</text><rect x="20" y="72" width="140" height="52" rx="8" class="card"/><text x="90.0" y="94.5" text-anchor="middle" class="t b">Player</text><text x="90.0" y="110.5" text-anchor="middle" class="d">or streamer</text><rect x="190" y="72" width="140" height="52" rx="8" class="card"/><text x="260.0" y="94.5" text-anchor="middle" class="t b">AV receiver</text><text x="260.0" y="110.5" text-anchor="middle" class="d">optional</text><rect x="360" y="72" width="150" height="52" rx="8" class="hot"/><text x="435.0" y="94.5" text-anchor="middle" class="ht b">HDMI splitter</text><text x="435.0" y="110.5" text-anchor="middle" class="ht">chooses the EDID</text><path d="M160 98 L188 98" class="ln" marker-end="url(#ah-chain)"/><path d="M330 98 L358 98" class="ln" marker-end="url(#ah-chain)"/><rect x="540" y="56" width="200" height="190" rx="10" class="box"/><text x="728" y="76" text-anchor="end" class="t b">The cinefit PC</text><rect x="556" y="86" width="168" height="48" rx="8" class="card"/><text x="640.0" y="106.5" text-anchor="middle" class="t b">Capture card</text><text x="640.0" y="122.5" text-anchor="middle" class="d">HDMI in</text><rect x="556" y="176" width="168" height="48" rx="8" class="card"/><text x="640.0" y="196.5" text-anchor="middle" class="t b">Graphics card</text><text x="640.0" y="212.5" text-anchor="middle" class="d">HDMI out</text><path d="M510 110 L554 110" class="ln" marker-end="url(#ah-chain)"/><path d="M640 134 L640 174" class="ln" marker-end="url(#ah-chain)"/><text x="652" y="158" text-anchor="start" class="d">cinefit</text><rect x="360" y="176" width="150" height="48" rx="8" class="card"/><text x="435.0" y="196.5" text-anchor="middle" class="t b">Another display</text><text x="435.0" y="212.5" text-anchor="middle" class="d">optional</text><path d="M435 124 L435 174" class="ln" marker-end="url(#ah-chain)"/><path d="M640 224 L640 270" class="ln" marker-end="url(#ah-chain)"/><rect x="560" y="272" width="160" height="44" rx="8" class="card"/><text x="640.0" y="298.5" text-anchor="middle" class="t b">Projector or TV</text></svg></div><figcaption>The player reads the EDID at the end of its cable, which with a splitter is the splitter's: that choice decides whether HDR10, tunneled Dolby Vision or low-latency Dolby Vision arrives.</figcaption></figure>
<!-- /fig -->

The receiver is optional; it is there for audio, and cinefit can read its input names ([Receiver and Apple TV](/integrations/)).

## What the capture card accepts {#accepts}

**The capture card accepts only signals without HDCP.** Most commercial players protect their output, and a protected signal gives no picture: the slate says **No signal**. cinefit does not remove or bypass copy protection; what reaches the card is up to the equipment upstream.

## Why a splitter

A splitter between the receiver and the card decides which EDID the player sees, and so which kind of signal it sends, independently of the projector and the receiver. Its second output can feed another display.

## The EDID decides what the player sends

A player chooses its output from the **EDID** it sees: the description of the display at the end of the cable. With a splitter in the chain, the player sees the splitter's EDID, not the card's or the projector's. That choice decides which kind of signal reaches cinefit:

| What the splitter presents | What a player sends | What cinefit does |
|---|---|---|
| A copy of the capture card's EDID | HDR10 and SDR, YCbCr 4:2:2 | Passes HDR10 through, or tone maps it ([`output.mode`](/settings/#output-mode)) |
| A Dolby Vision display (TV-led) | Dolby Vision tunneled in 10-bit RGB | Decodes it and maps it to SDR BT.2020 |
| A low-latency Dolby Vision display (LLDV) | Dolby Vision already mapped by the player | Shows it as is; set [`output.white_nits`](/settings/#output-white-nits) to the EDID's declared peak (often 96) |

::: callout warning "Watch for RGB and 4:4:4"
Some EDID presets make players send RGB or YCbCr 4:4:4. cinefit's bar detector reads only YCbCr, so with RGB the crop stays where it was. Formats cinefit cannot render show a slate saying **Can't show this format · set the player to YCbCr**. The most reliable choice is YCbCr 4:2:2, 10-bit.
:::

## Player settings

- **Color mode**: YCbCr (Auto usually ends up there with the EDIDs above).
- **Deep color**: Auto or 10-bit.
- **Frame rate**: match the content (for example "Match Frame Rate" on an Apple TV), so films arrive at 23.976 or 24.

## Be gentle with the input EDID

Every change to an EDID anywhere in the chain is a hotplug: the player renegotiates and picks its format again. Some receivers then send a black picture until their input is switched away and back. cinefit never writes the card's EDID, and only `cinefit probe --edid` reads it, as an explicit choice, since even a read may be a hotplug.

Next: [First picture](/first-picture/).
