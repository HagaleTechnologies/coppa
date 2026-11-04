# Real Reticulum over the HF OFDM modem

Build `cargo build --release -p coppa-daemon --bin coppad --features cpal-backend,kiss-tnc`,
then run `tools/reticulum-loop/run.sh --keep`. Linux, Python 3 with venv, pip access,
and ALSA are required. No sound card, root, or radio is used.

The script installs RNS 1.5.7 in a scratch venv and starts two coppads and two real
Reticulum nodes. It reuses `tools/pat-loop/relay.py` and `asoundrc.in` unchanged.
The harness sets `[audio] buffer_size = 131072` to absorb synchronous codec work
without dropping live input. Each node announces every 20 seconds and sends a 300-byte packet on hearing its peer.
It checks both received payloads and refuses a pass if either decoder overflowed.
The run ends within 300 seconds after client startup. Failed runs retain all logs.
Use `--timeout SECONDS` to change this bound and `--coppad PATH` for another build.
Debug builds require `--allow-debug` because they may not decode in real time.

See [the operator guide](../../docs/RETICULUM.md) for configuration and limits.
