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

- A Raspberry Pi 4. The release binary (option A in step 2) needs no compiler;
  building from source (option B) is easiest with 4 GB of RAM or more.
- Raspberry Pi OS Lite (64-bit). Trixie is current; Bookworm also works.
- A USB sound-card interface to the radio (a SignaLink, a rig's built-in USB
  codec, or similar).
- A way to key the radio: CAT through rigctld, a GPIO pin driving a transistor
  or relay, a serial port's DTR or RTS line, or the radio's VOX.

Use the Lite image. On the desktop image, a logged-in user's PipeWire session
can hold the USB sound card and keep coppad from opening it. This is reasoned
from how PipeWire claims ALSA devices, not tested.

## 2. Install coppa

There are two ways to get `coppa` and `coppad` onto the Pi. Option A, the
release binary, needs no compiler and takes a minute. Option B builds from
source on the Pi.

Both options need the ALSA runtime for live audio, and rigctld PTT needs
`libhamlib-utils` (which provides `rigctld`):

```bash
sudo apt-get update
sudo apt-get install -y libasound2 libhamlib-utils
```

On Raspberry Pi OS Trixie and other newer distributions the ALSA runtime
package is called `libasound2t64`; install that name if apt cannot find
`libasound2`.

### Option A: release binary

Pick the archive by the architecture of the operating system, not of the CPU:

```bash
dpkg --print-architecture
```

| `dpkg --print-architecture` prints | Use `t=` |
|---|---|
| `arm64` (64-bit Raspberry Pi OS) | `aarch64-unknown-linux-gnu` |
| `armhf` (32-bit Raspberry Pi OS) | `armv7-unknown-linux-gnueabihf` |

Do not go by `uname -m`. A 32-bit Raspberry Pi OS on a Pi 4 runs a 64-bit
kernel, so `uname -m` prints `aarch64` even though the programs it can run are
32-bit `armhf` ones.

The release binaries do not run on ARMv6 boards (Raspberry Pi 1, Zero and
Zero W). On those, use option B.

Download, check and unpack (shown for a 64-bit OS):

```bash
v=0.13.0; t=aarch64-unknown-linux-gnu
curl -fLO https://github.com/HagaleTechnologies/coppa/releases/download/v$v/coppa-$v-$t.tar.gz
curl -fLO https://github.com/HagaleTechnologies/coppa/releases/download/v$v/SHA256SUMS
sha256sum --ignore-missing -c SHA256SUMS
tar -xzf coppa-$v-$t.tar.gz
cd coppa-$v-$t
```

`sha256sum` should print `coppa-0.13.0-aarch64-unknown-linux-gnu.tar.gz: OK`.
The unpacked directory holds `coppa`, `coppad`, `coppad.toml.example` and the
`contrib/` service files, so the rest of this guide runs from inside it.

The release `coppad` is built with `cpal-backend`, `websocket`, `serial-ptt`,
`gpio-ptt`, `kiss-tnc`, `ardop` and `mdns`, so every PTT method and host
interface in this guide is available. Each one stays off until you enable it
in `coppad.toml`.

### Option B: build from source

```bash
sudo apt-get install -y build-essential pkg-config git curl libasound2-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
git clone https://github.com/HagaleTechnologies/coppa.git && cd coppa
cargo build --release -p coppa-daemon --features cpal-backend,websocket,gpio-ptt,serial-ptt,mdns
cargo build --release -p coppa-cli --features cpal-backend
```

- `libasound2-dev` is needed for live audio. The `serial-ptt` feature needs no
  extra system package.
- The repository's `rust-toolchain.toml` pins Rust 1.98.0 with clippy and
  rustfmt. rustup installs that toolchain on the first `cargo` run inside the
  checkout.

Pick the `coppad` features for the way you will run it:

| Feature | When you need it |
|---|---|
| `cpal-backend` | Always. Without it coppad runs but moves no audio. |
| `gpio-ptt` | `ptt_method = "gpio:<chip>:<line>"` |
| `serial-ptt` | `ptt_method = "serial:<port>:<dtr\|rts>"` |
| `websocket` | The WebSocket JSON API (`[host] websocket_enabled`) |
| `mdns` | Finding the Pi as `coppad.local` (`[host] mdns_enabled`); turns on `websocket` |

rigctld, VOX and CM108 PTT need no extra feature.

The build time on a Pi has not been measured; expect a long build. On a 1 or
2 GB Pi, if the compiler is killed for lack of memory, run the `cargo build`
lines again with `CARGO_BUILD_JOBS=1` in front of them. This advice is
reasoned, not measured. Cross-compiling from a faster machine also works, but
is outside the scope of this guide.

## 3. Install the binaries

From the unpacked release directory (option A):

```bash
sudo install -m 0755 coppad coppa /usr/local/bin/
```

Or from the checkout (option B):

```bash
sudo install -m 0755 target/release/coppad target/release/coppa /usr/local/bin/
```

The unit runs `/usr/local/bin/coppad`. `coppa` is optional; this guide uses it
to list sound devices. `coppa --version` prints the version, the commit it was
built from and its features.

The rest of this guide uses `coppad.toml.example` and `contrib/` by relative
path. Run those commands from the unpacked release directory or from the
checkout.

## 4. Create the service user

coppad runs as a dedicated system user, `coppa`, in the groups that own sound
cards (`audio`), serial ports (`dialout`) and GPIO (`gpio`):

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

coppad drives GPIO PTT through the GPIO character device (`/dev/gpiochipN`).
`ptt_method` names a chip and a line on it: `gpio:<chip>:<line>`. The chip is
a label, a name such as `gpiochip0`, or a path such as `/dev/gpiochip0`. On a
Pi's header chip the line is the BCM GPIO number. Find the chip and line with
the `gpiod` tools:

```bash
sudo apt install gpiod
gpiodetect
gpioinfo
```

On a Pi 4, `gpiodetect` lists `pinctrl-bcm2711` with 58 lines, so BCM GPIO 17
is:

```toml
[radio]
ptt_method = "gpio:pinctrl-bcm2711:17"
```

On a Pi 5, use the label `gpiodetect` shows for the header chip. This guide has
not been run on a Pi 5.

coppad requests the line at its RX level, as consumer `coppad`; its PTT
guardian uses `coppad-guardian`, so `gpioinfo` shows who holds the line. A
line another process holds is retried for 2 s, after which coppad names the
holder. After coppad lets go of the line, its level is up to the kernel's GPIO
driver: coppad drives it to RX first. coppad must be built with the
`gpio-ptt` feature.

**The older sysfs form.** `ptt_method = "gpio:529"` (the kernel's sysfs GPIO
number: the chip's base, 512 on a Pi 4 with kernel 6.6 or later, plus the BCM
pin) still works where the kernel has sysfs GPIO, and logs a warning naming the
new form. The sysfs interface is deprecated upstream; where the kernel lacks
it, coppad refuses to start and names the `gpio:<chip>:<line>` form to use.

### Serial (DTR or RTS)

```toml
[radio]
ptt_method = "serial:/dev/ttyUSB0:rts"
```

Use `dtr` instead of `rts` if your interface keys on DTR. Prefer the port's
`/dev/serial/by-id/...` path (`ls /dev/serial/by-id/`) to `/dev/ttyUSB0`, for
example `serial:/dev/serial/by-id/usb-FTDI_FT232R_USB_UART_A1B2C3-if00-port0:rts`:
the by-id path stays the same when USB devices are re-enumerated. The `coppa` user needs the `dialout` group, and coppad must be
built with `serial-ptt`.

There is no PTT guardian for serial PTT: if coppad crashes while keyed,
release depends on the operating system dropping DTR/RTS when it closes the
port, which has not been checked on real hardware (see
[OPERATING.md](../OPERATING.md#what-is-not-covered)).

### CM108 USB interface (AIOC, Digirig Lite, DRA)

Many USB sound-card interfaces key the rig from a GPIO pin on their C-Media
CM108 or CM119 chip (or a chip that copies it, like the AIOC). coppad drives
that pin by writing to the interface's Linux hidraw node:

```toml
[radio]
ptt_method = "cm108:/dev/cm108ptt"
```

You can also give the node directly, `cm108:/dev/hidraw0`, and another pin
after a second colon, `cm108:/dev/hidraw0:4`. The pin defaults to GPIO 3,
which the AIOC, Digirig Lite and DRA-style interfaces use for PTT. No cargo
feature is needed. CM108 PTT is Linux only.

hidraw numbers can change between boots and when USB devices are plugged in,
and the nodes are readable and writable by root only by default. A udev rule
fixes both: it gives the `audio` group access and adds a stable
`/dev/cm108ptt` symlink. Find your interface's vendor ID with `lsusb` (the
four hex digits before the colon in `ID 0d8c:013c`), then install this rule
as `/etc/udev/rules.d/60-coppa-cm108.rules`:

```
# C-Media CM108/CM119 family. Replace 0d8c with the vendor ID `lsusb` shows for
# your interface (an AIOC shows 1209), and add ATTRS{idProduct}=="xxxx" if more
# than one matching device is plugged in.
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="0d8c", GROUP="audio", MODE="0660", SYMLINK+="cm108ptt"
```

Apply it and check the symlink:

```bash
sudo udevadm control --reload
sudo udevadm trigger
ls -l /dev/cm108ptt
```

The shipped unit already gives the `coppa` user the `audio` group.

- An AIOC needs firmware 1.2.0 or later for CM108-style PTT.
- A Digirig Mobile keys through serial RTS, not a CM108 pin. Use
  `serial:<port>:rts` for it (above).
- coppad starts a PTT guardian for CM108, as for rigctld and GPIO: the chip
  holds its pin until something writes it again, so the guardian writes the
  RX report if coppad dies while keyed.

### Inverted (active-low) PTT

If your interface keys the rig when its PTT line is low, add:

```toml
[radio]
ptt_invert = true
```

TX then drives the line low and RX drives it high. It applies to gpio and
cm108 PTT, and the PTT guardian is told about it. With serial PTT, coppad
refuses to start, because closing the port would leave an active-low rig
keyed. With `none`, `vox` or `rigctld`, coppad logs a warning and ignores
it. See
[OPERATING.md](../OPERATING.md#active-low-ptt-lines-ptt_invert).

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

Optional, on a trusted, firewalled LAN: to open the operator page from a phone
or laptop as `coppad.local`, set under `[host]`:

```toml
bind_address = "0.0.0.0"
websocket_enabled = true
websocket_token = "paste the output of openssl rand -hex 32 here"
websocket_allowed_origins = ["http://coppad.local:8400"]
mdns_enabled = true
```

Restart coppad, then open `http://coppad.local:8400/` (or probe
`http://coppad.local:8400/health`). `bind_address` applies to every listener,
so with `vara_enabled = true` the VARA-style ports become reachable too. Allow
UDP port 5353 through any firewall. A Linux client needs nss-mdns to resolve
`.local` names. See
[OPERATING.md](../OPERATING.md#monitoring-and-discovery) for the details.

## 9. Change the configuration

Edit the file, check it, then reload. coppad re-reads the file on SIGHUP
and applies the settings it can change while running:

```bash
sudoedit /etc/coppa/coppad.toml
sudo -u coppa coppad --config /etc/coppa/coppad.toml --check
sudo systemctl reload coppad
journalctl -u coppad -n 5   # a restart_required= line means: sudo systemctl restart coppad
```

[OPERATING.md](../OPERATING.md#reloading-the-configuration) lists which
settings a reload applies. Audio devices, PTT method and listener ports need
`sudo systemctl restart coppad`.

To run a second coppad on the same Pi (for example, one radio on HF and one
on VHF), use the template unit `contrib/coppad@.service`; see
"More than one coppad" in `contrib/README.md`.

## 10. What the unit does

- **Failure:** `Restart=on-failure` with `RestartSec=10` and
  `StartLimitIntervalSec=0`. If coppad exits with an error (for example, the
  USB sound card is missing when it starts), systemd starts it again every
  10 s and never gives up, so it comes up by itself once the card appears.
  The cost is that a config error logs one failure every 10 s until you fix
  it. Unplugging the card while coppad is running does **not** make it exit:
  it keeps running without audio (and may log `RX audio input is silent`), so
  run `sudo systemctl restart coppad` after plugging the card back in.
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
  Ignoring.`, which is expected. With the character-device GPIO form, a line
  the old guardian still holds is retried for 2 s. With the older sysfs form,
  a guardian that is releasing the pin also unexports it, which could pull the
  pin away from a coppad that has just started. So with that form, if a stop
  had to end in a SIGKILL while transmitting, use `sudo systemctl stop coppad`,
  wait a few seconds, then `sudo systemctl start coppad`, rather than
  `systemctl restart`.
- **Hardening:** the unit sets `NoNewPrivileges`, `ProtectSystem=strict`,
  `ProtectHome`, `PrivateTmp`, `ProtectControlGroups`, `ProtectKernelModules`,
  `ProtectKernelLogs`, `ProtectHostname` and `RestrictSUIDSGID`. It
  deliberately omits `ProtectKernelTunables` (it makes `/sys` read-only, so
  the older sysfs GPIO form fails), `PrivateDevices` (it hides the sound card,
  serial ports and GPIO chips), `ProtectClock`, `DeviceAllow` and `DevicePolicy` (below systemd 252
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
| `failed to open GPIO PTT line CHIP:N: failed to read GPIO chip /dev/gpiochip0: ... Permission denied (os error 13)` | The user is not in `gpio` (or `gpio` is missing from `SupplementaryGroups`) | `sudo usermod -aG gpio coppa`, then restart |
| `failed to open GPIO PTT line CHIP:N: no GPIO chip matches "CHIP"; this system has ...` | No chip has that label, name or path | Use a chip from the list (step 7, GPIO) |
| `failed to open GPIO PTT line CHIP:N: the line is in use by "..."` | Another program holds the line | Stop it (`gpioinfo` names it), then restart |
| `ptt_method = "gpio:N" needs the sysfs GPIO interface ...` | The kernel has no sysfs GPIO | Use the `gpio:<chip>:<line>` form the message names |
| `failed to open GPIO PTT pin N: failed to export GPIO pin N: Permission denied (os error 13)` | Older sysfs form: the user is not in `gpio` | `sudo usermod -aG gpio coppa`, then restart |
| `failed to open GPIO PTT pin N: failed to export GPIO pin N: Invalid argument (os error 22)` | Older sysfs form: `N` is a BCM pin number, not the sysfs number | Use `gpio:<chip>:<line>` instead (step 7, GPIO) |
| `failed to open serial PTT port ...: failed to open serial port ...: Permission denied` | The user is not in `dialout` | `sudo usermod -aG dialout coppa`, then restart |
| `failed to open CM108 PTT device ...: cannot open ... for writing: Permission denied (os error 13)` | No udev rule gives the `audio` group the hidraw node, or the user is not in `audio` | Install the rule from step 7 (CM108), or `sudo usermod -aG audio coppa`, then restart |
| `failed to open CM108 PTT device ...: cannot open ... for writing: No such file or directory (os error 2)` | The path is wrong, or the interface is unplugged | Check `ls -l /dev/hidraw* /dev/cm108ptt` and the cable |
| `Failed to connect to rigctld; PTT is disabled ...`, then `Daemon running degraded` | rigctld was not listening when coppad started | Start rigctld, add the drop-in from step 7, restart coppad |
| `status=216/GROUP` in `systemctl status` | A group in `SupplementaryGroups` does not exist | Remove it from the unit (step 10) |
| `RX audio input is silent: ...` | No audio is reaching the card | Check the cable, the ALSA mixer (`alsamixer`) and any mute switch |

The GPIO, serial, CM108, `216/GROUP` and silent-input rows describe what the
code and systemd print, but have not been reproduced on a Pi.

Whatever the PTT method, turn on your rig's TX time-out timer, set a little
longer than `max_tx_duration_s`, as [OPERATING.md](../OPERATING.md) explains.
It is the only protection that works when nothing else can.
