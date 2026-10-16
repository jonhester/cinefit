---
title: "Updating"
description: "Update cinefit to a new release with its package, choose when the service switches to it, and go back to an earlier release."
---

# Updating

Download the latest package and install it over the old one:

```sh
curl -fsSLO https://github.com/jonhester/cinefit/releases/latest/download/cinefit_amd64.deb
sudo apt install ./cinefit_amd64.deb
```

Settings, the Apple TV pairing, logs and recordings stay where they are. If a release retires a setting, cinefit says at start what replaced it rather than ignoring the line.

## When it takes effect

<!-- fig:update -->
<figure class="fig"><div class="scroll"><svg viewBox="0 0 760 188" role="img" aria-label="Installing an update does not interrupt playback; the new build runs from the next start" font-size="13"><defs><marker id="ah-upd" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" class="ah"/></marker></defs><text x="20" y="18" text-anchor="start" class="t b" font-size="15">An update waits for the next start</text><rect x="20" y="84" width="460" height="36" rx="6" class="old"/><text x="250" y="107" text-anchor="middle" class="t">the film plays on the build that started it</text><rect x="490" y="84" width="250" height="36" rx="6" class="hot"/><text x="615" y="107" text-anchor="middle" class="ht b">the new build</text><path d="M250 58 L250 80" class="ln" marker-end="url(#ah-upd)"/><text x="250" y="52" text-anchor="middle" class="t" font-size="12">sudo apt install ./cinefit_amd64.deb</text><path d="M485 74 V130" class="ln" stroke-dasharray="3 3"/><text x="485" y="150" text-anchor="middle" class="d">next start: a boot, or sudo systemctl restart cinefit</text><text x="485" y="168" text-anchor="middle" class="d">the display resyncs once, here</text></svg></div><figcaption>Installing never restarts a running service, so an update can go in during a film.</figcaption></figure>
<!-- /fig -->

**A running service keeps its build** until it next starts: at the next boot, or when you restart it. A restart is a resync on the display, so an update never does it behind your back. To switch now:

```sh
sudo systemctl restart cinefit
```

## Going back

Every release's package is on the [releases page](https://github.com/jonhester/cinefit/releases), named with its version. Install the one you want:

```sh
curl -fsSLO https://github.com/jonhester/cinefit/releases/download/v0.1.0/cinefit_0.1.0_amd64.deb
sudo apt install --allow-downgrades ./cinefit_0.1.0_amd64.deb
sudo systemctl restart cinefit
```

An older release may refuse a setting that a newer one added; the log then names it, and removing the line fixes it.

## Removing cinefit

```sh
sudo apt remove cinefit    # stops the service; keeps settings, logs and recordings
sudo apt purge cinefit     # also deletes /etc/cinefit, /var/lib/cinefit and /var/log/cinefit
```
