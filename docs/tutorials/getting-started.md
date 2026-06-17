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
Reading from cq.wav
Decoded: "CQ CQ CQ DE VK2ABC K"
```

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
# Run with default settings
cargo run --bin coppad

# Run with a custom config file
cargo run --bin coppad -- my_config.toml
```

Example `coppad.toml`:
```toml
[audio]
sample_rate = 48000
buffer_size = 8192

[radio]
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
