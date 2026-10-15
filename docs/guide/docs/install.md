---
title: "Installing"
description: "Install cinefit on Ubuntu 26.04: the DeckLink driver, user groups, a release or a source build with tools/install.sh, and the one-time systemd service setup."
---

# Installing

Every step here is done once. After it, cinefit starts at boot and updates need no root.

## 1. The capture card's driver

Download **Desktop Video** (16.0 or newer) for Linux from Blackmagic's support site and install its `.deb` packages. Then check the card is seen:

```sh
ls /dev/blackmagic/
```

You should see `io0`. If not, the driver is not loaded (a reboot after installing is the usual fix).

## 2. Libraries and groups

```sh
sudo apt install mesa-vulkan-drivers libvulkan1 liblcms2-2 libdrm2 libdrm-amdgpu1 libgbm1 curl python3
sudo usermod -aG video,render,input "$USER"
```

The groups let cinefit open the display (`video`), the GPU's render node (`render`) and the remote or keyboard (`input`) without root. Log out and back in for them to take effect.

## 3. cinefit itself

Clone the repository and install the latest published release:

```sh
git clone https://github.com/jonhester/cinefit.git
cd cinefit
tools/install.sh --release
```

`install.sh` checks your CPU, downloads the release, verifies its checksum and puts it in `~/cinefit-app/`, with `~/cinefit-app/current` pointing at it. It runs `cinefit settings` on the new build before switching, so a broken download never becomes current.

::: callout info "Building from source instead"
On a CPU without x86-64-v3, or to run unreleased code, install Rust, build libplacebo once with `scripts/build-libplacebo.sh`, then run `tools/install.sh` with no arguments. It builds the tree and installs that build the same way.
:::

## 4. The service (root, once)

The first time, `install.sh` prints the steps that need root, with your user filled in:

```sh
sudo cp ~/cinefit-app/cinefit.service /etc/systemd/system/
sudo cp ~/cinefit-app/50-cinefit.rules /etc/polkit-1/rules.d/
sudo systemctl daemon-reload
sudo systemctl enable --now cinefit
```

The service runs `~/cinefit-app/current play` as your user, waits up to 30 seconds at boot for the capture card, and restarts itself 5 seconds after any exit. The polkit rule lets your user start, stop and restart **this unit only** without a password, so updates and tests need no `sudo`.

Check it is running and watch its log:

```sh
systemctl status cinefit
journalctl -u cinefit -f
```

## 5. Keep automatic updates out of the film {#updates}

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
