---
title: "Remote and menu"
description: "The remote's buttons and the keys they send, and the on-screen menu: its pages, its settings and what each change costs."
---

# Remote and menu

## Buttons

cinefit reads any USB keyboard-like device with arrow keys: a keyboard, or an IR receiver such as a flirc that types keys for a remote. These keys are taken, so they do not also reach the console behind the picture.

<!-- fig:remote -->
<figure class="fig"><div class="scroll"><svg viewBox="0 0 760 310" role="img" aria-label="The remote: arrows, OK, Back, Menu and Info, and the keyboard keys each one sends" font-size="13"><defs><marker id="ah-remote" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" class="ah"/></marker></defs><text x="20" y="18" text-anchor="start" class="t b" font-size="15">Five kinds of button run everything</text><rect x="60" y="40" width="140" height="250" rx="40" class="card"/><path d="M121 95 L130 84 L139 95z" class="ah" transform="rotate(0 130 90)"/><path d="M159 133 L168 122 L177 133z" class="ah" transform="rotate(90 168 128)"/><path d="M121 171 L130 160 L139 171z" class="ah" transform="rotate(180 130 166)"/><path d="M83 133 L92 122 L101 133z" class="ah" transform="rotate(270 92 128)"/><circle cx="130" cy="128" r="56" class="box"/><circle cx="130" cy="128" r="20" class="hot"/><text x="130" y="132.5" text-anchor="middle" class="ht b">OK</text><rect x="72" y="208" width="52" height="28" rx="14" class="card"/><text x="98" y="226.5" text-anchor="middle" class="t" font-size="12">Back</text><rect x="136" y="208" width="52" height="28" rx="14" class="card"/><text x="162" y="226.5" text-anchor="middle" class="t" font-size="12">Menu</text><rect x="104" y="248" width="52" height="28" rx="14" class="card"/><text x="130" y="266.5" text-anchor="middle" class="t" font-size="12">Info</text><text x="300" y="56" text-anchor="start" class="t b">Arrows</text><text x="300" y="73" text-anchor="start" class="d">move around the menu; left and right change a value</text><text x="740" y="56" text-anchor="end" class="d" font-size="12">arrow keys</text><text x="300" y="108" text-anchor="start" class="t b">OK</text><text x="300" y="125" text-anchor="start" class="d">enter a page of the menu</text><text x="740" y="108" text-anchor="end" class="d" font-size="12">Enter or E</text><text x="300" y="160" text-anchor="start" class="t b">Back</text><text x="300" y="177" text-anchor="start" class="d">up a level; closes the menu or the info panel</text><text x="740" y="160" text-anchor="end" class="d" font-size="12">Backspace, Esc or B</text><text x="300" y="212" text-anchor="start" class="t b">Menu</text><text x="300" y="229" text-anchor="start" class="d">opens and closes the menu</text><text x="740" y="212" text-anchor="end" class="d" font-size="12">C or M</text><text x="300" y="264" text-anchor="start" class="t b">Info</text><text x="300" y="281" text-anchor="start" class="d">what is coming in, going out, and the crop</text><text x="740" y="264" text-anchor="end" class="d" font-size="12">I or D</text></svg></div><figcaption>Any remote that sends these keys works, through a USB receiver such as a flirc; so does a keyboard. cinefit keys shows what each button sends.</figcaption></figure>
<!-- /fig -->

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

## The menu {#menu}

**Menu** opens it.

<figure class="shot"><img src="/assets/images/osd-menu.png" alt="The menu, on its Picture page: Crop black bars On, Fill the screen up to 2.40:1, Banners On; at the foot, Applies now, no resync" width="2884" height="1146" loading="lazy"><figcaption>The menu as cinefit draws it, on its Picture page. The foot says what the selected change costs. Picture: <em>Charade</em> (1963), public domain in the US, with cinefit's display drawn over it.</figcaption></figure>

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

Settings not in the menu are on the [web page](/web-page/) and in the [settings file](/settings/).
