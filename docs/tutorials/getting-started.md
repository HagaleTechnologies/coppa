# Getting Started with Coppa

## Prerequisites

- Rust 1.85.0 or later
- ALSA development headers (Linux): `sudo apt-get install libasound2-dev`
- No additional dependencies on macOS or Windows

## Building

```bash
git clone https://github.com/HagaleTechnologies/coppa.git
cd coppa
cargo build --workspace
```

## Running Tests

```bash
cargo test --workspace
```

## Loopback Test

The simplest way to verify everything works is the loopback test, which encodes a message and immediately decodes it.

**Note:** WAV file operations require the `file-backend` feature:

```bash
cargo run -p coppa-cli --bin coppa --features file-backend -- loopback "Hello from Coppa"
```

Expected output:
```
Coppa Loopback Test
===================
Input: "Hello from Coppa"
Encoded: 46080 samples
Decoded: "Hello from Coppa"
PASS: Loopback test successful!
```

## Encoding to a WAV File

```bash
cargo run -p coppa-cli --bin coppa --features file-backend -- tx "CQ CQ CQ DE VK2ABC K" -o cq.wav
```

Expected output:
```
Encoding: "CQ CQ CQ DE VK2ABC K"
Written 46080 samples to cq.wav
```

## Decoding a WAV File

```bash
cargo run -p coppa-cli --bin coppa --features file-backend -- rx -i cq.wav
```

Expected output:
```
Reading 65520 samples from cq.wav
Decoded: "CQ CQ CQ DE VK2ABC K"  (SNR: 25.7 dB)
```

Payloads that aren't valid UTF-8 print as `Decoded (hex): …`; `--hex` forces hex, and `--raw` prints only the hex for scripts.

For a stream that scripts can parse, use `--json`:

```bash
cargo run -p coppa-cli --bin coppa -- rx cq.wav --json | jq -c .
```

Each decoded frame is one newline-delimited JSON object. `payload_hex` always
contains the exact bytes; `payload_text` is `null` for binary frames. SNR and
CFO are numbers when finite and `null` otherwise. Status text is absent from
JSON stdout, and diagnostics use stderr. `--json` conflicts with `--raw` and
`--hex`.

## Listing Audio Devices

Requires the `cpal-backend` feature for real hardware access:

```bash
cargo run -p coppa-cli --bin coppa --features cpal-backend -- devices
cargo run -p coppa-cli --bin coppa --features cpal-backend -- devices --json
```

`devices --json` writes a single array, including `[]` when no devices are
listed.

## Receiving Live Audio

Requires the `cpal-backend` feature:

```bash
cargo run -p coppa-cli --bin coppa --features cpal-backend -- rx --duration 60
```

With no file, `coppa rx` captures from the default input device (`--device <name>` picks
another by substring) and prints each frame as it decodes, in the same format as a WAV
decode. `--duration` stops after that many seconds; without it, `coppa rx` runs until
Ctrl+C. `coppa listen` is a deprecated alias for `coppa rx` and will be removed in the
next release. It accepts `--json` too and prints its deprecation warning on
stderr. Use `-q` to suppress status output, or `-v` for diagnostics. Repeat
`-v` as `-vv` for recommended speed level and delay spread per decoded frame;
quiet and verbose flags conflict.

## Persisting your CLI settings

`coppa tx`, `tune`, `rx`, and `loopback` read the same `coppad.toml` as the daemon.
They search the current directory, your Coppa user config directory, then the
system config path in the order listed in [Running the Daemon](#running-the-daemon).
Use `coppa --config PATH` or `COPPA_CONFIG=PATH` to select one file explicitly.
A flag wins over its `COPPA_*` environment variable, which wins over the file;
`--verbose` shows each selected value and where it came from. See
`coppa tx --help` for the variable and TOML key names.

A copied `coppad.toml.example` sets `profile = "HF_STANDARD"`. That changes the
CLI's waveform from its built-in no-file default. Set the same profile on the
peer, including when decoding a WAV file, or the payload may not decode.

## Viewing Operating Profiles

```bash
# List all profiles, as text or one JSON array
cargo run -p coppa-cli --bin coppa -- profiles
cargo run -p coppa-cli --bin coppa -- profiles --json

# Show one profile, as text or one JSON object
cargo run -p coppa-cli --bin coppa -- profiles -p HF_STANDARD
cargo run -p coppa-cli --bin coppa -- profiles -p HF_STANDARD --json
```

`config` remains available with its earlier text output. `profiles` reports
the encoder's maximum bytes per frame for each named profile.

## Running the Daemon

The daemon (`coppad`) runs as a long-lived process with an optional VARA-style TCP control interface:

```bash
# Show usage, or the version, and exit
cargo run -p coppa-daemon --bin coppad -- --help
cargo run -p coppa-daemon --bin coppad -- --version

# Run with default settings
cargo run -p coppa-daemon --bin coppad

# Run with a custom config file
cargo run -p coppa-daemon --bin coppad -- --config my_config.toml
```

For live audio (a radio, or Winlink with Pat), build `coppad` with `--release`
and `--features cpal-backend`: a debug build decodes too slowly to keep up
with real-time audio. [docs/PAT.md](../PAT.md) walks through a Winlink
session with Pat.

Without a config argument, `coppad` uses the first of these files that
exists, and otherwise runs on built-in defaults:

1. `coppad.toml` in the current directory;
2. `$XDG_CONFIG_HOME/coppa/coppad.toml`, or `~/.config/coppa/coppad.toml` when
   `XDG_CONFIG_HOME` is unset or not an absolute path (an empty or relative
   `HOME` skips this entry; `%APPDATA%\coppa\coppad.toml` on Windows);
3. `/etc/coppa/coppad.toml` (not on Windows).

It never merges files. At startup it logs `Using configuration file` with the
path it chose, or a warning listing every path it searched when it found none;
without a config argument, `coppad --check` prints the same information as its
first line. A path you
name with `--config` (or with the older positional form,
`coppad my_config.toml`, which still works) must exist and is the only file
read: a missing or unreadable file is an error that names the path, not a
silent fallback to defaults. A file the search finds that fails to parse is an
error too, naming that file.

To start from the built-in defaults, print them as TOML, edit the copy, and
check it before running:

```bash
cargo run -p coppa-daemon --bin coppad -- --print-default-config > my_config.toml
cargo run -p coppa-daemon --bin coppad -- --config my_config.toml --check
```

`--check` prints `Configuration syntax and field types OK.` only after it has
read the file and parsed it into the daemon's configuration. That is all it
proves: the TOML is well formed, every field has the right type, and there is
no unknown or misspelled key or table (such as `calsign` for `callsign`); an
unknown key is an error that names it, in `--check` and when the daemon starts.
It does not check that the PTT method or profile name is valid, that
`buffer_size` is above 0, that the sample rate suits the profile, that the audio
devices exist, that rigctld answers, that the host ports are free, or that the
setup suits your radio. Those surface when the daemon starts. `--check` opens no
hardware and starts no listeners.

When the daemon starts, a condition that stops it doing its job is logged at
`ERROR`, never as `Daemon ready`. An `[engine] profile` that is not one of the
four built-in names, `[audio] buffer_size = 0`, a sample rate other than
48000, a named `[audio]` device that does not exist (or any named device in a
build without `cpal-backend`), an audio device that will not open, a host port
that is already taken, an unknown PTT method, `[host] websocket_enabled = true`
in a build without the `websocket` feature, or a wideband profile with no
frequency source stops the daemon: it logs the reason at `ERROR`, prints `Error: ...` on
stderr and exits with status 1. An unreachable rigctld does not stop it. The
daemon keeps running without PTT, logs the failed connection at `ERROR`, and
logs `Daemon running degraded` with the reason in place of `Daemon ready`. A
PTT backend that refuses the startup release (for example a rigctld that
answers `RPRT -1`) is reported the same way. Just before `Daemon ready`, the
log states the host listeners, the audio devices that were opened, and the PTT
backend in control; see
[What coppad reports at startup](../OPERATING.md#what-coppad-reports-at-startup).

Logging goes to stdout. Colour codes are written only when stdout is a
terminal and `NO_COLOR` is unset or empty, so a log file or the systemd
journal gets plain text. `--log-level` takes one of `off`, `error`, `warn`,
`info`, `debug` or `trace` and overrides `RUST_LOG`. Without it, `RUST_LOG`
applies (it accepts full tracing filter expressions), and an absent or invalid
`RUST_LOG` means `info`:

```bash
cargo run -p coppa-daemon --bin coppad -- --config my_config.toml --log-level debug
# info everywhere, debug for the daemon's own log target (coppad)
RUST_LOG=info,coppad=debug cargo run -p coppa-daemon --bin coppad
```

`--tnc` runs the AFSK 1200/AX.25 KISS TNC instead of the OFDM daemon, with its
own built-in settings (KISS on `127.0.0.1:8001`, 48 kHz audio); it reads no
config file and cannot be combined with `--config`, `--check` or
`--print-default-config`. It needs the `kiss-tnc` feature, and the
`cpal-backend` feature for real audio:

```bash
cargo run -p coppa-daemon --bin coppad --features kiss-tnc,cpal-backend -- --tnc
```

A build without `kiss-tnc` exits with `Error: TNC mode requires the kiss-tnc
feature`.

[docs/TNC.md](../TNC.md) says what the TNC mode is for (KISS bring-up and
testing), covers `coppa tnc`, which takes its own `--port` and `--device` so
several TNCs can run on one machine, and walks through a demo with two of them.

Example `coppad.toml`:
```toml
[audio]
sample_rate = 48000
buffer_size = 8192

[radio]
# "none" (default), "vox", "rigctld", or "serial:<port>:<dtr|rts>" /
# "gpio:<pin>" (Linux only; requires coppad built with --features
# serial-ptt / gpio-ptt respectively). An unrecognized value is a hard
# startup error, not a silent fallback to no PTT.
ptt_method = "none"
# Wait this long after decoding a frame before keying (default 500 ms);
# must exceed the other station's PTT tail / VOX hang time.
# turnaround_ms = 500

[host]
vara_enabled = true
vara_command_port = 8300
vara_data_port = 8301
# Off by default: data-port and WebSocket `send` bytes are transmitted only
# inside a connected session. `true` restores sessionless TX (raw frames, or
# ARQ segments with [engine] arq_enabled) for two-station bench rigs; any
# host that can reach the data port can then key the transmitter.
sessionless_tx_enabled = false

[engine]
profile = "HF_STANDARD"
callsign = "VK2ABC"
```

## Using the C FFI

`coppa-ffi` builds Coppa as a C library. Its header, `crates/coppa-ffi/coppa.h`, is generated by cbindgen from the Rust source and checked in. C, C++, Python, Swift, or any language with a C FFI can call it.

Build the library from the repository root:

```bash
cargo build -p coppa-ffi --release
```

On Linux this writes `target/release/libcoppa_ffi.so`; on macOS, `target/release/libcoppa_ffi.dylib`. Save this program as `hello.c`:

```c
#include <stdio.h>
#include "coppa.h"

int main(void) {
    coppa_engine_t *engine = coppa_engine_create();
    if (engine == NULL) {
        fprintf(stderr, "coppa_engine_create failed\n");
        return 1;
    }

    float *samples = NULL;
    uintptr_t len = 0;
    int32_t result = coppa_encode(engine, "Hello", &samples, &len);

    if (result == 0) {
        printf("Encoded %zu samples\n", (size_t)len);
        coppa_free_samples(samples, len);
    }

    /* Pass the handle's address: destroy frees the engine and sets
     * engine to NULL, so calling it again is a safe no-op. */
    coppa_engine_destroy(&engine);
    return result == 0 ? 0 : 1;
}
```

Compile and run it from the repository root (on macOS, use `DYLD_LIBRARY_PATH` in place of `LD_LIBRARY_PATH`):

```bash
cc -std=c11 -Wall -Wextra -Icrates/coppa-ffi hello.c -Ltarget/release -lcoppa_ffi -o hello
LD_LIBRARY_PATH=target/release ./hello
```

It prints `Encoded 65520 samples`.

`coppa_engine_destroy` takes the address of your handle (`coppa_engine_t **`), not the handle. It frees the engine and sets your variable to `NULL`, so a second call on the same variable does nothing. It is the only function in the API shaped this way: `coppa_free_samples` and the other `coppa_free_*` functions take the pointer itself. Include `coppa.h` rather than writing your own prototypes; with the header, a compiler flags `coppa_engine_destroy(engine)` as an incompatible pointer type.

`coppa.h` wraps its declarations in `extern "C"`, so the same program builds as C++:

```bash
c++ -x c++ -std=c++17 -Wall -Wextra -Icrates/coppa-ffi hello.c -Ltarget/release -lcoppa_ffi -o hello
```

## Next Steps

- Read [ARCHITECTURE.md](../../ARCHITECTURE.md) for system design details
- Browse the crate documentation: `cargo doc --workspace --no-deps --open`
