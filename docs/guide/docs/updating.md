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
