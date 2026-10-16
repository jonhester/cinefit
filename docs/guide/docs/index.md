---
title: "cinefit"
description: "cinefit is an open-source video processor for home theaters: it captures HDMI on a Linux PC, crops away black bars for constant image height, handles HDR10 and Dolby Vision, and drives your projector directly."
noStyle: true
titleAppend: false
components:
  meta: true
  favicon: true
  css: false
  highlight: false
  theme: false
  themeMode: false
  spa: false
customHead: '<link rel="stylesheet" href="/assets/css/landing.css"><link rel="apple-touch-icon" href="/assets/images/apple-touch-icon.png">'
---
<div class="landing">
<header class="top">
  <a class="brand" href="/"><img src="/assets/images/cinefit-wordmark-dark.svg" alt="cinefit" width="120" height="31"></a>
  <nav>
    <a href="/overview/">Guide</a>
    <a href="/settings/">Settings</a>
    <a href="https://github.com/jonhester/cinefit">GitHub</a>
  </nav>
</header>
<section class="hero">
  <h1>A video processor for your home theater, on a PC you build.</h1>
  <p class="lede">cinefit captures what your player sends, crops away the black bars, scales the picture to fill your screen, and drives your projector directly, with HDR10 and Dolby Vision handled the way you choose. Open source, no desktop, no subscription.</p>
  <div class="cta">
    <a class="button primary" href="/requirements/">Get started</a>
    <a class="button" href="/overview/">Read the guide</a>
  </div>
</section>
<section class="cih">
  <div class="cih-text">
    <h2>Same height, every film</h2>
    <p>On a scope screen, a 16:9 show and a 2.39:1 film should stand the same height. cinefit finds the bars in every frame and places the picture so it does: no lens memories, no remote juggling between titles.</p>
  </div>
  <div class="cih-art" role="img" aria-label="Two scope screens: a 16:9 show with black at the sides, and a 2.39:1 film filling the screen, both the same height">
    <figure>
      <svg viewBox="0 0 240 104"><defs><linearGradient id="sky-a" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2b2350"/><stop offset="0.62" stop-color="#9a6b8f"/><stop offset="1" stop-color="#f0b27a"/></linearGradient><clipPath id="pic-a"><rect x="31" y="2" width="178" height="100"/></clipPath></defs><rect x="2" y="2" width="236" height="100" rx="2" class="screen"/><g clip-path="url(#pic-a)"><rect x="0" y="0" width="240" height="104" fill="url(#sky-a)"/><circle cx="150" cy="70" r="13" fill="#ffd9a0"/><path d="M0 78 L40 60 L78 74 L118 56 L160 76 L200 62 L240 72 V104 H0Z" fill="#3a2d4f"/><path d="M0 90 L52 76 L96 88 L140 74 L190 90 L240 80 V104 H0Z" fill="#1d1729"/></g><rect x="2" y="2" width="236" height="100" rx="2" class="frame"/></svg>
      <figcaption>A 16:9 show</figcaption>
    </figure>
    <figure>
      <svg viewBox="0 0 240 104"><defs><linearGradient id="sky-b" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2b2350"/><stop offset="0.62" stop-color="#9a6b8f"/><stop offset="1" stop-color="#f0b27a"/></linearGradient><clipPath id="pic-b"><rect x="2" y="2" width="236" height="100"/></clipPath></defs><rect x="2" y="2" width="236" height="100" rx="2" class="screen"/><g clip-path="url(#pic-b)"><rect x="0" y="0" width="240" height="104" fill="url(#sky-b)"/><circle cx="150" cy="70" r="13" fill="#ffd9a0"/><path d="M0 78 L40 60 L78 74 L118 56 L160 76 L200 62 L240 72 V104 H0Z" fill="#3a2d4f"/><path d="M0 90 L52 76 L96 88 L140 74 L190 90 L240 80 V104 H0Z" fill="#1d1729"/></g><rect x="2" y="2" width="236" height="100" rx="2" class="frame"/></svg>
      <figcaption>A 2.39:1 film</figcaption>
    </figure>
  </div>
</section>
<section class="features">
  <article>
    <h3>Dolby Vision, decoded</h3>
    <p>A tunneled Dolby Vision signal is decoded from the pixels and mapped by cinefit itself. A player's low-latency Dolby Vision passes through untouched.</p>
  </article>
  <article>
    <h3>HDR10 your way</h3>
    <p>Pass HDR10 through for your projector's own tone mapping, or let cinefit map it to SDR BT.2020 with libplacebo's curves.</p>
  </article>
  <article>
    <h3>Made for a dark room</h3>
    <p>An on-screen menu and info panel driven by your remote, short banners when the picture type changes, and a quiet slate while it waits.</p>
  </article>
  <article>
    <h3>Runs like an appliance</h3>
    <p>Installs as an Ubuntu package and starts with the PC. Updates never interrupt a film, and an earlier release is one install away.</p>
  </article>
  <article>
    <h3>On your phone too</h3>
    <p>A web page on your home network shows what is coming in and going out, and holds every setting.</p>
  </article>
  <article>
    <h3>Fewer resyncs</h3>
    <p>cinefit follows the source's frame rate, and changes HDR metadata without blanking the display. A resync happens only when the signal really changes.</p>
  </article>
</section>
<section class="shot">
  <img src="/assets/images/osd-menu.png" alt="cinefit's on-screen menu: the Picture page, with Crop black bars on, Fill the screen up to 2.40:1, and Banners on" width="1974" height="1146" loading="lazy">
  <p>The on-screen menu, as cinefit draws it. Every change says what it costs before you make it.</p>
</section>
<section class="parts">
  <h2>What it takes</h2>
  <ol>
    <li><strong>A PC</strong> with Ubuntu 26.04 and a free PCIe slot</li>
    <li><strong>An AMD graphics card</strong>, tested on the Radeon RX 6400</li>
    <li><strong>A Blackmagic DeckLink</strong> capture card with HDMI in</li>
    <li><strong>A source</strong> whose signal the capture card accepts</li>
  </ol>
  <a class="button primary" href="/requirements/">See the full list</a>
</section>
<section class="ai">
  <h2>Set it up with an AI assistant</h2>
  <p>The whole guide is published as <a href="/llms-full.txt">one text file</a> that ChatGPT, Claude and other assistants can read. Hand it over with your parts and your logs, and ask. <a href="/ai-help/">How to ask well</a>.</p>
</section>
<footer class="bottom">
  <p>cinefit is open source, under Apache-2.0 or MIT. Developed on an AMD RX 6400, a DeckLink Mini Recorder 4K and a JVC projector; other hardware is untested.</p>
  <p><a href="/overview/">Guide</a> · <a href="/install/">Install</a> · <a href="/troubleshooting/">Troubleshooting</a> · <a href="https://github.com/jonhester/cinefit">GitHub</a></p>
</footer>
</div>
