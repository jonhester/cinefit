---
title: "What you need"
description: "The parts for a cinefit system: a PC, an AMD graphics card (tested on a deliberately modest RX 6400), a Blackmagic DeckLink capture card, a source it accepts, your projector and a remote; what is tested and what should work."
---

# What you need

A cinefit system is a PC in your rack, between your sources and your projector. These are the parts, with what has been tested and what should work.

<p class="disclosure">Some links below go to Amazon. Buying through them supports cinefit, at no cost to you: as an Amazon Associate, cinefit's author earns from qualifying purchases.</p>

### A PC {#pc}

<div class="setting part"><div class="tags"><span class="tag tag-start">Any recent desktop</span></div><p>Runs Ubuntu and cinefit, with no desktop: cinefit starts by itself when the PC powers on, and after setup it needs no monitor or keyboard of its own.</p><dl><div><dt>Needs</dt><dd>Two PCIe slots, one for each card below; Ubuntu 26.04</dd></div><div><dt>CPU</dt><dd>From about 2015 on (Intel Haswell, AMD Excavator or Zen) for the published builds; older ones can <a href="/build-from-source/">build from source</a></dd></div><div><dt>Check</dt><dd>Room and airflow for two cards</dd></div></dl></div>

### A graphics card {#gpu}

<div class="setting part"><div class="tags"><span class="tag tag-now">Tested: AMD Radeon RX 6400</span><span class="tag tag-resync">Should work: newer AMD cards</span><span class="tag tag-untried">Not tried: NVIDIA, Intel</span></div><p>Scales, crops and colors every frame, and drives the projector. Developed and tested on an <strong>AMD Radeon RX 6400</strong>, chosen on purpose as a modest card: if cinefit runs well on it, most AMD cards will do.</p><dl><div><dt>Tested with</dt><dd>AMD Radeon RX 6400: RDNA 2, 2022, about 53 W, no power connector, low-profile versions made</dd></div><div><dt>Should work</dt><dd>Newer AMD cards (RX 7000 and RX 9000 series), with more headroom; not yet tested</dd></div><div><dt>Needs</dt><dd>Vulkan, HDMI out</dd></div></dl><div class="links"><a href="https://www.amd.com/en/products/graphics/desktops/radeon/6000-series/amd-radeon-rx-6400.html">AMD's page</a> · <a href="https://www.amazon.com/s?k=Radeon+RX+6400" rel="sponsored noopener">Find it on Amazon</a></div></div>

### A capture card {#capture}

<div class="setting part"><div class="tags"><span class="tag tag-now">Tested: DeckLink Mini Recorder 4K</span><span class="tag tag-resync">Should work: other DeckLinks with HDMI in</span></div><p>How the picture gets into the PC: a <strong>Blackmagic DeckLink</strong> card with an HDMI input.</p><dl><div><dt>Tested with</dt><dd>DeckLink Mini Recorder 4K</dd></div><div><dt>Needs</dt><dd>Blackmagic's Desktop Video driver, 16.0 or newer</dd></div><div><dt>Check</dt><dd>The Mini Recorder 4K captures 4K up to 30 frames per second (60 at 1080p). Films at 24 are fine; set the player to match the content's frame rate. A 4K60 source needs a card that captures it</dd></div></dl><div class="links"><a href="https://www.blackmagicdesign.com/products/decklink/models">Blackmagic's DeckLink range</a> · <a href="https://www.amazon.com/s?k=DeckLink+Mini+Recorder+4K" rel="sponsored noopener">Find it on Amazon</a></div></div>

### A source the capture card accepts {#source}

<div class="setting part"><div class="tags"><span class="tag tag-start">Your player or streamer</span></div><p>The capture card takes only signals without HDCP copy protection, which most commercial players apply; cinefit does not remove or bypass it. An HDMI splitter in front of the card also decides which kind of HDR the player sends.</p><dl><div><dt>Needs</dt><dd>A signal without HDCP</dd></div><div><dt>Then</dt><dd>An HDMI splitter whose EDID you can choose</dd></div><div><dt>More</dt><dd><a href="/signal-chain/">The signal chain</a> explains both</dd></div></dl></div>

### A projector or TV {#display}

<div class="setting part"><div class="tags"><span class="tag tag-now">Tested: a JVC 4096x2160 projector</span><span class="tag tag-resync">Should work: any HDMI display</span></div><p>Anything with an HDMI input. cinefit starts out set for a 4096x2160 projector on a 2.35:1 scope screen, and two settings adapt it to yours.</p><dl><div><dt>Needs</dt><dd>HDMI in; its native resolution at 24 Hz (nearly every 4K display has it)</dd></div><div><dt>Then</dt><dd>Set <a href="/settings/#output-width">the panel size</a> and <a href="/settings/#geometry-screen-ar">the screen ratio</a></dd></div><div><dt>Check</dt><dd>For constant image height, a scope screen and a lens that zooms</dd></div></dl></div>

### A remote {#remote}

<div class="setting part"><div class="tags"><span class="tag tag-now">Tested: flirc USB receiver</span><span class="tag tag-start">Optional</span></div><p>Any remote, through a small USB receiver that turns its buttons into keyboard keys. A plain USB keyboard works too, and the <a href="/web-page/">web page</a> works from a phone.</p><dl><div><dt>Tested with</dt><dd>flirc USB receiver, with a universal remote</dd></div><div><dt>Needs</dt><dd>A free USB port</dd></div><div><dt>More</dt><dd><a href="/using/">Remote and menu</a> lists the keys</dd></div></dl><div class="links"><a href="https://flirc.tv/products/flirc-usb-receiver">flirc's page</a> · <a href="https://www.amazon.com/s?k=flirc+USB" rel="sponsored noopener">Find it on Amazon</a></div></div>

## The finer print

For anyone matching parts or troubleshooting:

- **GPU**: needs Vulkan (Mesa's RADV driver on AMD), a display output the Linux kernel drives directly, and an overlay plane for the on-screen menu. cinefit holds the GPU's clocks steady on AMD only; elsewhere it logs `gpu clocks: not held` and carries on.
- **Capture driver**: Blackmagic Desktop Video 16.0 or newer.
- **CPU**: published builds target x86-64-v3 (AVX2).
- **Nothing else on the display**: no desktop environment on the GPU's output. cinefit drives the display itself.
- **Optional extras**: a Denon receiver and an Apple TV on your network add input names and what's playing to the on-screen info. See [Receiver and Apple TV](/integrations/).
