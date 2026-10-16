---
title: "Installing"
description: "Install cinefit on Ubuntu 26.04 LTS: the DeckLink driver, then the cinefit package, which sets up the system service, its user and its folders."
---

# Installing

Two packages: the capture card's driver, then cinefit. After that, cinefit starts whenever the PC does.

## 1. The capture card's driver

Download **Desktop Video** (16.0 or newer) for Linux from Blackmagic's support site and install it. Then check the card is seen:

```sh
ls /dev/blackmagic/
```

You should see `io0`. If not, the driver is not loaded (a reboot after installing is the usual fix).

## 2. cinefit

```sh
curl -fsSLO https://github.com/jonhester/cinefit/releases/latest/download/cinefit_amd64.deb
sudo apt install ./cinefit_amd64.deb
```

apt installs what cinefit needs alongside it (Vulkan and the graphics libraries), and the package then:

- creates a `cinefit` system user, allowed to use the display, the GPU and the remote,
- installs and starts the `cinefit` service, which starts again at every boot.

Check it is running, and watch what it does:

```sh
systemctl status cinefit
journalctl -u cinefit -f
```

### Where things are

| What | Where |
|---|---|
| The program | `/usr/bin/cinefit` |
| Settings, and the Apple TV pairing | `/etc/cinefit/` |
| Run logs | `/var/log/cinefit/`, and `journalctl -u cinefit` |
| The bar detector's recordings | `/var/lib/cinefit/` |

The web page and the on-screen menu change settings for you. To edit the settings file by hand, edit it as the `cinefit` user, so the service can still save to it:

```sh
sudo -u cinefit editor /etc/cinefit/cinefit.toml
```

## 3. Running cinefit's commands yourself (optional)

The checks in [First picture](/first-picture/) are cinefit commands you type. To run them as yourself, join the same groups as the service, then log out and back in:

```sh
sudo usermod -aG video,render,input "$USER"
```

## 4. Keep automatic updates out of the film {#updates}

Ubuntu installs security updates daily, and afterwards restarts services that use an updated library. The graphics libraries load into cinefit, so a Mesa or Vulkan update restarts it: the picture goes black for a few seconds and the display resyncs, in the middle of whatever is playing.

Move the daily upgrade to a time nobody watches (the time is in the machine's clock, often UTC):

```sh
sudo systemctl edit apt-daily-upgrade.timer
```

```ini
[Timer]
OnCalendar=
OnCalendar=*-*-* 16:00
RandomizedDelaySec=30m
```

And tell `needrestart` never to restart cinefit; it picks up new libraries at its next start instead:

```sh
echo '$nrconf{override_rc}{qr(^cinefit\.service$)} = 0;' \
  | sudo tee /etc/needrestart/conf.d/cinefit.conf
```

Next: [The signal chain](/signal-chain/).

::: callout info "Other ways to install"
To run unreleased code, on a CPU too old for the published builds, or without root, see [Building from source](/build-from-source/).
:::
