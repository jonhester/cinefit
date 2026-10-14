# On-screen UI mockups

Design mockups for the OSD and web page, from the design discussion in #72.
They are drawings of intent, not screenshots of cinefit: sizes and spacing are
approximate, and "The Screening Room" stands in for a theater's name. Each
screen mockup shows the whole 4096 x 2160 panel; the dashed lines mark the
2.35 screen, and nothing is drawn on the rows outside it.

| Image | What | Issue |
| --- | --- | --- |
| [palette.png](palette.png) | Colours: lavender accent, three text tiers, apricot for attention and lighter red for errors, on words only | #72 |
| [logo.png](logo.png) | The logo from `docs/logo` in the original slate blue and in purple | #72 |
| [slate.png](slate.png) | The slate, the quiet corner: no signal, identifying, an error, attention | #76 |
| [slate-no-name.png](slate-no-name.png) | The slate with and without a theater name set | #76 |
| [banner.png](banner.png) | The banner on a change of picture type, and when it shows | #77 |
| [info.png](info.png) | The info overlay | #72 |
| [menu.png](menu.png) | The OSD menu: pages on the left, settings on the right | #75 |
| [menu-output.png](menu-output.png), [menu-picture.png](menu-picture.png), [menu-input.png](menu-input.png), [menu-system.png](menu-system.png) | The menu as cinefit's own OSD code draws it (2026-10-14), at the banner's 60% scale over a stand-in picture: Output is HDR10 (to the projector, or tone-mapped to SDR BT.2020) and the rate while waiting; Picture is crop, fill limit and banners; Input is RGB range, Dolby Vision low latency and the capture device; System is the version, run logs and restart. The version is also at the foot of the left column on every page; read-only rows have no arrows. The brightness and tone-curve settings live on the web page | #75 |
| [web.png](web.png) | The web page on a phone | #74 |
