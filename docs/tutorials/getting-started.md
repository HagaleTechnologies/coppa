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
cargo run --bin coppa --features file-backend -- loopback "Hello from Coppa"
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
cargo run --bin coppa --features file-backend -- tx "CQ CQ CQ DE VK2ABC K" -o cq.wav
```

Expected output:
```
Encoding: "CQ CQ CQ DE VK2ABC K"
Written 46080 samples to cq.wav
```

## Decoding a WAV File

```bash
cargo run --bin coppa --features file-backend -- rx -i cq.wav
```

Expected output:
```
Reading 65520 samples from cq.wav
Decoded: "CQ CQ CQ DE VK2ABC K"  (SNR: 25.7 dB)
```

Payloads that aren't valid UTF-8 print as `Decoded (hex): …`; `--hex` forces hex, and `--raw` prints only the hex for scripts.

## Listing Audio Devices

Requires the `cpal-backend` feature for real hardware access:

```bash
cargo run --bin coppa --features cpal-backend -- devices
```

## Viewing Operating Profiles

```bash
# List all profiles
cargo run --bin coppa -- config

# Show details for a specific profile
cargo run --bin coppa -- config -p HF_STANDARD
```

## Running the Daemon

The daemon (`coppad`) runs as a long-lived process with an optional VARA-style TCP control interface:

```bash
# Show usage, or the version, and exit
cargo run --bin coppad -- --help
cargo run --bin coppad -- --version

# Run with default settings
cargo run --bin coppad

# Run with a custom config file
cargo run --bin coppad -- --config my_config.toml
```

Without a config argument, `coppad` loads `./coppad.toml` if it exists and
otherwise runs on built-in defaults. A path you name with `--config` (or with
the older positional form, `coppad my_config.toml`, which still works) must
exist: a missing or unreadable file is an error that names the path, not a
silent fallback to defaults. An existing `./coppad.toml` that fails to parse
is an error too.

To start from the built-in defaults, print them as TOML, edit the copy, and
check it before running:

```bash
cargo run --bin coppad -- --print-default-config > my_config.toml
cargo run --bin coppad -- --config my_config.toml --check
```

`--check` prints `Configuration syntax and field types OK.` only after it has
read the file and parsed it into the daemon's configuration. That is all it
proves: the TOML is well formed and every known field has the right type.
Unknown keys are still accepted. It does not check that the PTT method or
profile name is valid, that the sample rate suits the profile, that the audio
devices exist, that rigctld answers, that the host ports are free, or that the
setup suits your radio. Those surface when the daemon starts. `--check` opens no
hardware and starts no listeners.

Logging goes to stdout. `--log-level` takes one of `off`, `error`, `warn`,
`info`, `debug` or `trace` and overrides `RUST_LOG`. Without it, `RUST_LOG`
applies (it accepts full tracing filter expressions), and an absent or invalid
`RUST_LOG` means `info`:

```bash
cargo run --bin coppad -- --config my_config.toml --log-level debug
RUST_LOG=coppa_daemon=debug cargo run --bin coppad
```

`--tnc` runs the AFSK 1200/AX.25 KISS TNC instead of the OFDM daemon, with its
own built-in settings (KISS on `127.0.0.1:8001`, 48 kHz audio); it reads no
config file and cannot be combined with `--config`, `--check` or
`--print-default-config`. It needs the `kiss-tnc` feature, and the
`cpal-backend` feature for real audio:

```bash
cargo run --bin coppad --features kiss-tnc,cpal-backend -- --tnc
```

A build without `kiss-tnc` exits with `Error: TNC mode requires the kiss-tnc
feature`.

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

[host]
vara_enabled = true
vara_command_port = 8300
vara_data_port = 8301

[engine]
profile = "HF_STANDARD"
callsign = "VK2ABC"
```

## Using the C FFI

Coppa can be used from C, Python, Swift, or any language with C FFI support:

```c
#include <stdio.h>

// Opaque handle
typedef void* CoppaHandle;

extern CoppaHandle coppa_engine_create();
extern void coppa_engine_destroy(CoppaHandle handle);
extern int coppa_encode(CoppaHandle handle, const char* message,
                        float** out_samples, size_t* out_len);
extern void coppa_free_samples(float* samples, size_t len);

int main() {
    CoppaHandle engine = coppa_engine_create();

    float* samples;
    size_t len;
    int result = coppa_encode(engine, "Hello", &samples, &len);

    if (result == 0) {
        printf("Encoded %zu samples\n", len);
        coppa_free_samples(samples, len);
    }

    coppa_engine_destroy(engine);
    return 0;
}
```

## Next Steps

- Read [ARCHITECTURE.md](../../ARCHITECTURE.md) for system design details
- Browse the crate documentation: `cargo doc --workspace --no-deps --open`
