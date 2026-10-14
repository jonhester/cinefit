# Running cinefit as the service

cinefit runs as `cinefit.service`, a system unit that starts at boot and plays
whatever the capture card receives. Everything after the one-time setup below
is done without root: `tools/install.sh` installs builds and switches between
them, and the polkit rule lets your user start, stop and restart the unit.

## One-time setup (root)

`tools/install.sh` prints these the first time, with your user filled in:

```sh
tools/install.sh                       # builds and installs; writes the files below
sudo cp ~/cinefit-app/cinefit.service /etc/systemd/system/
sudo cp ~/cinefit-app/50-cinefit.rules /etc/polkit-1/rules.d/
sudo systemctl daemon-reload
sudo systemctl enable --now cinefit
```

The unit runs `~/cinefit-app/current play`, a symlink `tools/install.sh`
points at a build; settings come from `~/.config/cinefit-rs/cinefit.toml`.

## Keep Ubuntu's automatic updates out of the film

Ubuntu installs security updates by itself (unattended-upgrades) once a day,
and afterwards `needrestart` restarts every service that uses a library the
update replaced. The graphics stack loads into cinefit, so an update to Mesa,
Vulkan or even X11 libraries restarts it: the picture goes black for a few
seconds and the projector resyncs, mid-film if the timer fires then. It did,
once at 02:18 local time, after a `libx11` update.

Two settings stop that. Both are one-time root steps; either alone helps, the
two together leave nothing to chance.

**Move the daily upgrade to a time nobody watches.** Ubuntu's default runs it
between 06:00 and 07:00 by the machine's clock, which on a machine kept in UTC
is the small hours in the Americas. For example, noon US Eastern:

```sh
sudo systemctl edit apt-daily-upgrade.timer
```
```ini
[Timer]
OnCalendar=
OnCalendar=*-*-* 16:00
RandomizedDelaySec=30m
```

The empty `OnCalendar=` clears the default before setting the new time, which
is in the machine's clock (16:00 UTC is noon Eastern in summer, 11:00 in
winter). Check with `systemctl list-timers apt-daily-upgrade.timer`.

**Tell needrestart never to restart cinefit.** Updates still install; cinefit
picks the new libraries up at its next start (after a film, at boot, or
`RESTART=1 tools/install.sh`):

```sh
echo '$nrconf{override_rc}{qr(^cinefit\.service$)} = 0;' \
  | sudo tee /etc/needrestart/conf.d/cinefit.conf
```

To see whether an update restarted cinefit, look for `systemctl restart
cinefit.service` in `/var/log/unattended-upgrades/unattended-upgrades.log`, or
a `Stopping cinefit.service` a few seconds after `apt-daily-upgrade` starts in
`journalctl`.
