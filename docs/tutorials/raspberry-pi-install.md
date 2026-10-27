# Running coppad as a service on a Raspberry Pi 4

This guide takes a Raspberry Pi 4 from a fresh Raspberry Pi OS Lite image to
`coppad` running as a non-root systemd service that starts at boot and comes
back by itself after a failure. It installs the unit shipped in
[`contrib/coppad.service`](../../contrib/coppad.service).

For the CLI basics (loopback, WAV files, the FFI), see
[getting-started.md](getting-started.md). For field operation (TX levels, ALC,
PTT safety), see [OPERATING.md](../OPERATING.md).

The commands below were checked against the shipped files and the real
`coppad --check`, but this guide has **not yet been run end to end on a Pi**.
Steps that depend on the Pi's hardware are marked as not verified.

## 1. What you need

- A Raspberry Pi 4. A native build is easiest with 4 GB of RAM or more.
- Raspberry Pi OS Lite (64-bit). Trixie is current; Bookworm also works.
- A USB sound-card interface to the radio (a SignaLink, a rig's built-in USB
  codec, or similar).
- A way to key the radio: CAT through rigctld, a GPIO pin driving a transistor
  or relay, a serial port's DTR or RTS line, or the radio's VOX.

Use the Lite image. On the desktop image, a logged-in user's PipeWire session
can hold the USB sound card and keep coppad from opening it. This is reasoned
from how PipeWire claims ALSA devices, not tested.

## 2. Build coppa on the Pi

There are no release binaries yet, so build from source on the Pi:

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config git curl libasound2-dev libudev-dev libhamlib-utils
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
git clone https://github.com/HagaleTechnologies/coppa.git && cd coppa
cargo build --release -p coppa-daemon --features cpal-backend,websocket,gpio-ptt,serial-ptt
cargo build --release -p coppa-cli --features cpal-backend
```

- `libasound2-dev` is needed for live audio. `libudev-dev` is needed only for
  the `serial-ptt` feature, and `libhamlib-utils` (which provides `rigctld`)
  only for rigctld PTT.
- The repository's `rust-toolchain.toml` pins Rust 1.98.0 with clippy and
  rustfmt. rustup installs that toolchain on the first `cargo` run inside the
  checkout.

Pick the `coppad` features for the way you will run it:

| Feature | When you need it |
|---|---|
| `cpal-backend` | Always. Without it coppad runs but moves no audio. |
| `gpio-ptt` | `ptt_method = "gpio:<pin>"` |
| `serial-ptt` | `ptt_method = "serial:<port>:<dtr\|rts>"` (needs `libudev-dev`) |
| `websocket` | The WebSocket JSON API (`[host] websocket_enabled`) |

rigctld and VOX PTT need no extra feature.

The build time on a Pi has not been measured; expect a long build. On a 1 or
2 GB Pi, if the compiler is killed for lack of memory, run the `cargo build`
lines again with `CARGO_BUILD_JOBS=1` in front of them. This advice is
reasoned, not measured. Cross-compiling from a faster machine also works, but
is outside the scope of this guide.

## 3. Install the binaries

From the checkout:

```bash
sudo install -m 0755 target/release/coppad target/release/coppa /usr/local/bin/
```

The unit runs `/usr/local/bin/coppad`. `coppa` is optional; this guide uses it
to list sound devices.

## 4. Create the service user

coppad runs as a dedicated system user, `coppa`, in the groups that own sound
cards (`audio`), serial ports (`dialout`) and sysfs GPIO (`gpio`):

```bash
sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin \
    --groups audio,dialout,gpio coppa
id coppa
```

`id coppa` should list `audio`, `dialout` and `gpio`.

On a casual single-purpose Pi you can run coppad as your own login user
instead. Add that user to the same three groups and change `User=` and
`Group=` in the unit to match.

## 5. Configure

Put the config where the unit reads it, readable by the `coppa` group and
writable only by root:

```bash
sudo install -d -o root -g coppa -m 0750 /etc/coppa
sudo install -o root -g coppa -m 0640 coppad.toml.example /etc/coppa/coppad.toml
sudoedit /etc/coppa/coppad.toml
```

Set at least `[engine] callsign`, the sound devices (step 6) and the PTT
method (step 7). Then check the file as the service user:

```bash
sudo -u coppa coppad --config /etc/coppa/coppad.toml --check
```

A good file prints:

```text
Configuration syntax and field types OK.
Hardware and runtime settings were not checked.
```

`--check` reads only the file. It does not open the sound card or the PTT
device; the service start in step 8 does that.

## 6. Pick the sound card

List the devices the `coppa` user can see:

```bash
sudo -u coppa coppa devices
```

- The names are ALSA descriptions, and one USB card usually appears several
  times (as its `sysdefault`, `front`, `hw` and `plughw` variants).
- coppad uses the first device whose name contains your string, ignoring
  case. A distinctive part of the card's name, such as `USB Audio CODEC`, is
  usually enough.
- Set both `[audio] input_device` and `output_device`. Do not leave
  `input_device` empty: a Pi 4 has no built-in audio input, so the default
  input device does not exist and coppad stops at startup.

A name that matches nothing stops coppad with a line that lists the real
names:

```text
audio input device "USB Audio" not found ([audio] input_device); available input devices: ...
```

## 7. Choose a PTT method

Set `[radio] ptt_method` to one of the following.

### rigctld (recommended)

CAT PTT through hamlib's `rigctld`. If coppad dies while transmitting, the
PTT guardian releases the radio (see
[OPERATING.md](../OPERATING.md#ptt-safety-when-coppad-stops)).

Find your rig's model number with `rigctl -l`, then create
`/etc/systemd/system/rigctld.service`, replacing `<model>` and `<baud>`:

```ini
[Unit]
Description=Hamlib rigctld
After=network.target

[Service]
ExecStart=/usr/bin/rigctld -m <model> -r /dev/ttyUSB0 -s <baud> -T 127.0.0.1 -t 4532
User=coppa
Group=coppa
SupplementaryGroups=dialout
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now rigctld
```

In `/etc/coppa/coppad.toml`:

```toml
[radio]
ptt_method = "rigctld"
rigctld_address = "127.0.0.1:4532"
```

coppad connects to rigctld once, at startup. If rigctld is not listening yet,
coppad runs the whole session without PTT. `After=rigctld.service` alone is
not enough, because systemd considers rigctld started before it opens its
port. Once you have installed the coppad unit (step 8), add a drop-in that
waits for the port:

```bash
sudo systemctl edit coppad
```

```ini
[Unit]
Wants=rigctld.service
After=rigctld.service

[Service]
# coppad connects to rigctld once, at startup. If rigctld is not
# listening yet, coppad runs the whole session without PTT and logs
# "Daemon running degraded". Wait up to 30 s for rigctld's port.
ExecStartPre=/usr/bin/timeout 30 /bin/bash -c 'until (: </dev/tcp/127.0.0.1/4532) 2>/dev/null; do sleep 1; done'
```

The command contains no `$` or `%`, so systemd passes it to bash unchanged.

### GPIO

A GPIO pin drives a transistor or relay that keys the radio. Never connect a
GPIO pin straight to the rig's PTT line.

The number in `ptt_method` is the kernel's sysfs GPIO number, not the BCM pin
number: it is the GPIO chip's base plus the BCM pin. On Raspberry Pi OS with
kernel 6.6 or later, the Pi 4's chip is registered at base 512, so BCM GPIO 17
is sysfs GPIO 529. Read the base on your Pi rather than assuming it:

```bash
grep -H . /sys/class/gpio/gpiochip*/label /sys/class/gpio/gpiochip*/base
```

Take the `base` of the chip whose `label` is `pinctrl-bcm2711` and add the BCM
pin. For BCM 17 on a base of 512:

```toml
[radio]
ptt_method = "gpio:529"
```

Raspberry Pi OS's udev rule (`99-com.rules`) gives the `gpio` group access to
each pin as it is exported. It does so a moment after the export, so coppad
waits up to 2 s for that access after exporting a pin itself.

The sysfs GPIO interface is deprecated upstream, though still built into
Raspberry Pi kernels. Test PTT on your image before relying on it, and use
rigctld if a future kernel drops it. coppad must be built with the
`gpio-ptt` feature.

### Serial (DTR or RTS)

```toml
[radio]
ptt_method = "serial:/dev/serial/by-id/usb-...-if00-port0:rts"
```

Use the port's `/dev/serial/by-id/...` path (`ls /dev/serial/by-id/`) rather
than `/dev/ttyUSB0`: the by-id path stays the same when USB devices are
re-enumerated. The `coppa` user needs the `dialout` group, and coppad must be
built with `serial-ptt`.

There is no PTT guardian for serial PTT: if coppad crashes while keyed,
release depends on the operating system dropping DTR/RTS when it closes the
port, which has not been checked on real hardware (see
[OPERATING.md](../OPERATING.md#what-is-not-covered)).

### VOX

```toml
[radio]
ptt_method = "vox"
```

The radio keys itself on the audio coppad sends. Nothing in coppad can release
a VOX-keyed radio; set the radio's VOX hang time short.

## 8. Install and start the service

```bash
sudo install -m 0644 contrib/coppad.service /etc/systemd/system/coppad.service
sudo systemctl daemon-reload
sudo systemctl enable --now coppad
systemctl status coppad
journalctl -u coppad -f
```

Startup is finished when the journal shows `Daemon ready`. If it shows
`Daemon running degraded` instead, the same line names each problem (for
example, rigctld unreachable). See the troubleshooting table below.

To connect a VARA-style host client (for example, Pat), set
`[host] vara_enabled = true` and point the client at `127.0.0.1`, command
port 8300 and data port 8301. Keep `bind_address = "127.0.0.1"` unless the
network is trusted. As the example config warns, any other address "exposes
an UNAUTHENTICATED control plane to the network. Anyone who can reach this
host could key your transmitter."

## 9. Change the configuration

coppad has no reload. Edit the file, check it, then restart:

```bash
sudoedit /etc/coppa/coppad.toml
sudo -u coppa coppad --config /etc/coppa/coppad.toml --check
sudo systemctl restart coppad
```

## 10. What the unit does

- **Failure:** `Restart=on-failure` with `RestartSec=10` and
  `StartLimitIntervalSec=0`. If coppad exits with an error (for example, the
  USB sound card is unplugged), systemd starts it again every 10 s and never
  gives up, so it comes back by itself when the card returns. The cost is
  that a config error logs one failure every 10 s until you fix it.
- **Stop:** `systemctl stop` sends SIGTERM to coppad, which releases PTT, logs
  `Releasing PTT before shutdown`, and exits.
- **A stop that hangs:** after `TimeoutStopSec=20`, systemd SIGKILLs coppad.
  The unit sets `KillMode=process`, so that SIGKILL reaches coppad alone and
  not its PTT guardian (`coppad --ptt-guardian`), which then releases PTT
  itself. Under systemd's default `KillMode=control-group`, the guardian
  would be killed too. See
  [OPERATING.md](../OPERATING.md#ptt-safety-when-coppad-stops) for every case
  the guardian does and does not cover.
- **A quick restart:** a guardian from the previous run can still be exiting
  when the new coppad starts. systemd then logs `Found left-over process ...
  Ignoring.`, which is expected and harmless.
- **Hardening:** the unit sets `NoNewPrivileges`, `ProtectSystem=strict`,
  `ProtectHome`, `PrivateTmp`, `ProtectControlGroups`, `ProtectKernelModules`,
  `ProtectKernelLogs`, `ProtectHostname` and `RestrictSUIDSGID`. It
  deliberately omits `ProtectKernelTunables` (it makes `/sys` read-only, so
  GPIO PTT fails), `PrivateDevices` (it hides the sound card and serial
  ports), `ProtectClock`, `DeviceAllow` and `DevicePolicy` (below systemd 252
  `ProtectClock` alone shuts off every device except the RTC), and
  `DynamicUser` (there would be no fixed user to put in the device groups).
- **Groups:** every group in `SupplementaryGroups=audio dialout gpio` must
  exist. On a host other than Raspberry Pi OS there is usually no `gpio`
  group; delete it from that line, or systemd refuses to start coppad with
  `status=216/GROUP`.
- **Logs:** `Environment=NO_COLOR=1` keeps colour escape codes out of the
  journal.

## 11. Troubleshooting

| Log line | Cause | Fix |
|---|---|---|
| `failed to open the default input device ...` | `[audio] input_device` is empty, and a Pi 4 has no built-in input | Set it to your USB card's name (step 6) |
| `audio input device "..." not found ([audio] input_device); available input devices: ...` | The name matches no device | Use part of a name from the list |
| `failed to open GPIO PTT pin N: failed to export GPIO pin N: Permission denied (os error 13)` | The user is not in `gpio` (or `gpio` is missing from `SupplementaryGroups`) | `sudo usermod -aG gpio coppa`, then restart |
| `failed to open GPIO PTT pin N: failed to export GPIO pin N: Invalid argument (os error 22)` | `N` is a BCM pin number, not the sysfs number | Add the chip base (step 7, GPIO) |
| `failed to open serial PTT port ...: failed to open serial port ...: Permission denied` | The user is not in `dialout` | `sudo usermod -aG dialout coppa`, then restart |
| `Failed to connect to rigctld; PTT is disabled ...`, then `Daemon running degraded` | rigctld was not listening when coppad started | Start rigctld, add the drop-in from step 7, restart coppad |
| `status=216/GROUP` in `systemctl status` | A group in `SupplementaryGroups` does not exist | Remove it from the unit (step 10) |
| `RX audio input is silent: ...` | No audio is reaching the card | Check the cable, the ALSA mixer (`alsamixer`) and any mute switch |

The GPIO, serial, `216/GROUP` and silent-input rows describe what the code
and systemd print, but have not been reproduced on a Pi.

Whatever the PTT method, turn on your rig's TX time-out timer, set a little
longer than `max_tx_duration_s`, as [OPERATING.md](../OPERATING.md) explains.
It is the only protection that works when nothing else can.
