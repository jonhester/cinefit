---
title: "The web page"
description: "cinefit's web page on the home network: what is playing, every setting with what a change costs, Apple TV pairing and the logs of recent runs."
---

# The web page

Open `http://<the cinefit PC>:8080` on a phone or computer on the same network. It shows the same status as the info panel, every setting (including those not in the menu) with what changing it costs, Apple TV pairing, and the logs of recent runs.

<figure class="shot phones"><div><img src="/assets/images/web-status.png" alt="The web page on a phone: Playing; IN Dolby Vision low latency; OUT SDR BT.2020 unchanged from the player; CROP 2.40:1; LAST 10 S 240 shown, 0 repeated; recent run logs; the settings pages" width="1170" height="2532" loading="lazy"><img src="/assets/images/web-settings.png" alt="The web page's settings, the Picture page open: Crop black bars On, Record for detector tuning On, each with what changing it costs and its help" width="1170" height="2316" loading="lazy"></div><figcaption>The web page on a phone: what is playing, then every setting, each saying what a change costs.</figcaption></figure>

::: callout warning "No password"
The web page has no login: anyone on your network can change settings. Change the port with [`web.port`](/settings/#web-port), or set it to 0 to turn the page off.
:::
