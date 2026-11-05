# Broadcast files over OFDM KISS

Build with `cargo build -p coppa-cli --features kiss-tnc`. Configure an
OFDM coppad with KISS enabled using [RETICULUM.md](RETICULUM.md).
The AFSK `coppa tnc` service is a different transport.

For a five-byte file containing `hello` without a newline:

```text
coppa broadcast send hello.txt --kiss 127.0.0.1:8100 --repeat 3 --interval-ms 30000
Submitting 5 bytes in 1 chunk per copy to KISS.
Submitted 3 copies to KISS; delivery is unconfirmed.
```

Each receiving station runs this independently, even after sending starts:

```text
coppa broadcast receive --kiss 127.0.0.1:8100 --output received.txt --idle-timeout 86400
Waiting for broadcast file chunks.
Received 5 bytes into received.txt; checksum verified.
```

Status is on stderr; stdout is empty. Global `--quiet` suppresses status,
but errors remain. Success exits 0, errors exit 1, and Ctrl-C exits 130.
Stopping a sender cannot retract datagrams already accepted by coppad.
Normal daemon PTT cleanup still applies.

The shown KISS address, repeats, interval and idle timeout are the defaults.
The receive timeout defaults to 86400 seconds (24 hours), longer than a
complete 2731-chunk copy of the maximum-size file at the default 30-second
pacing. This allows a missing chunk to arrive in the next copy even when
only duplicates arrive in between. With slower pacing or longer channel
outages, set `--idle-timeout` above the expected gap between new chunks.
Repeats, interval and timeout must be positive. Output is required.
The receiver accepts the first valid transfer and sends no commands or data.
Only a new chunk resets its idle timer. Duplicates and unrelated traffic
cannot keep an incomplete receive alive. On timeout it reports, for example,
`Error: broadcast incomplete: no new chunks for 86400 seconds`.

Files may be empty or binary and must be at most 1048576 bytes.
Oversize inputs fail before connecting with
`Error: broadcast file exceeds 1048576 bytes`.
Existing outputs fail before connecting with
`Error: broadcast output already exists: received.txt`.
A verified file is created exclusively, never overwritten. Incomplete
transfers create no output; a failed file write removes the new file.

Chunks carry their own metadata and accumulate across repeated copies.
A late receiver needs enough remaining transmissions to collect every
chunk; reception after the final copy cannot recover missing data.
There is no return channel, ARQ or delivery confirmation. CRC64 checks
accidental damage, not sender authenticity. Do not treat a received file
as trusted merely because it passed CRC.

Use small files on a quiet channel. The 30-second interval spaces individual
datagrams, including across copy boundaries. This is conservative pacing,
not daemon backpressure. A 1 MiB copy has 2731 chunks and takes about
22.8 hours at this interval. Busy channels, collisions, active sessions
and the daemon's eight-packet queue can still lose chunks.
See [LIMITATIONS.md](LIMITATIONS.md) and the normative
[packet format](SPEC.md#broadcast-file-application).

Automated acceptance uses a one-way virtual audio cable with two independent
OFDM daemon receivers, one joining mid-frame, and a separate dropped-chunk
run. Physical RF, fading and real-time audio performance are unmeasured.
