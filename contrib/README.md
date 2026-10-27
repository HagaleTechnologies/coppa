# Service files for coppad

| File | What it is |
|---|---|
| [`coppad.service`](coppad.service) | systemd unit: runs coppad as the non-root user `coppa`, restarts it on failure, and leaves the PTT guardian alone on stop |
| [`io.github.hagaletechnologies.coppad.plist`](io.github.hagaletechnologies.coppad.plist) | macOS launchd agent: runs coppad in your login session and restarts it on failure |

Neither file has been verified on real hardware yet. For a complete walkthrough
on a Raspberry Pi 4, see
[docs/tutorials/raspberry-pi-install.md](../docs/tutorials/raspberry-pi-install.md).

## Any systemd host

The Pi guide explains each step; in brief, from a checkout with coppad built
(`cargo build --release -p coppa-daemon --features cpal-backend` plus any PTT
features you need):

```bash
sudo install -m 0755 target/release/coppad /usr/local/bin/
sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin \
    --groups audio,dialout,gpio coppa
sudo install -d -o root -g coppa -m 0750 /etc/coppa
sudo install -o root -g coppa -m 0640 coppad.toml.example /etc/coppa/coppad.toml
sudoedit /etc/coppa/coppad.toml
sudo -u coppa coppad --config /etc/coppa/coppad.toml --check
sudo install -m 0644 contrib/coppad.service /etc/systemd/system/coppad.service
sudo systemctl daemon-reload
sudo systemctl enable --now coppad
journalctl -u coppad -f
```

Most distributions other than Raspberry Pi OS have no `gpio` group. Drop
`gpio` from the `useradd --groups` list, and delete it from the unit's
`SupplementaryGroups=` line, or systemd refuses to start coppad with
`status=216/GROUP`.

## macOS (LaunchAgent)

The plist is a per-user LaunchAgent, not a system LaunchDaemon, because
CoreAudio devices belong to the logged-in session. launchd does not expand
`~`, so every path in it is absolute.

```bash
sudo install -d /usr/local/bin /usr/local/etc/coppa
sudo install -m 0755 target/release/coppad /usr/local/bin/
sudo install -m 0644 coppad.toml.example /usr/local/etc/coppa/coppad.toml
sudo install -d -o "$USER" /usr/local/var/log/coppa
mkdir -p ~/Library/LaunchAgents
cp contrib/io.github.hagaletechnologies.coppad.plist ~/Library/LaunchAgents/
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.github.hagaletechnologies.coppad.plist
tail -f /usr/local/var/log/coppa/coppad.log
```

Edit `/usr/local/etc/coppa/coppad.toml` (with `sudo`), check it with
`coppad --config /usr/local/etc/coppa/coppad.toml --check`, then restart the
agent:

```bash
launchctl kickstart -k gui/$(id -u)/io.github.hagaletechnologies.coppad
```

To stop it and keep it stopped until the next `bootstrap`:

```bash
launchctl bootout gui/$(id -u)/io.github.hagaletechnologies.coppad
```

Caveats:

- macOS microphone privacy (TCC) may refuse audio input to a binary started by
  launchd. That can show up as silence (coppad logs `RX audio input is
  silent: ...`), as a permission prompt, or as coppad being terminated. This
  has not been checked on a Mac.
- There is no GPIO PTT on macOS; use rigctld, serial or VOX.
- launchd restarts coppad after a non-zero exit, no more often than every
  10 s (`ThrottleInterval`), and never gives up. A clean exit is not
  restarted.
- Nothing rotates `/usr/local/var/log/coppa/coppad.log`.
