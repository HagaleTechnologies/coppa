# COP-172 LinBPQ session record

Run date: 2026-11-11. Host: Ubuntu 24.04.5 LTS, x86-64, no radio,
sound card, root installation, RMS account or CMS connection. Two release
coppad processes use HF_STANDARD at 48 kHz over the half-duplex userspace
ALSA cable. The B2F peer is LinBPQ's BBS application 1, N0GW-1.

## Programs and downloads

- coppa 0.13.0, built with `cargo build --release -p coppa-daemon --bin coppad --features cpal-backend`.
- LinBPQ's unmodified Linux i386 binary printed `G8BPQ AX25 Packet Switch System Version 6.0.26.1 October 2026`.
  Download: <https://www.cantab.net/users/john.wiseman/Downloads/linbpq>.
  SHA256: `6b5be6ffc5c0c1304d3e4175958ce2c78967ac4e8463474ce29744dc8aee748d`.
- Pat v1.0.0, downloaded at run time by `tools/pat-loop/fetch-pat.sh`.
  Archive SHA256: `331e4700c9e8b44098d7173e448da53204f59720c6c19ebc903685666aa377f7`.
- No-root i386 runtime, downloaded from Ubuntu's noble release pocket and
  extracted with `dpkg-deb -x`. Each download passed SHA256 verification:

```text
libc6_2.39-0ubuntu8_i386.deb 8928db539155135cc6943677d6ee158fe6380653188716e16daf9de7094e6476
libgcc-s1_14-20240412-0ubuntu1_i386.deb 43cb16a9a1cbfcb90d86aeb445a1ab7ba60bd6d0acd44679ffad1c0dee11b10e
zlib1g_1.3.dfsg-3.1ubuntu2_i386.deb 96bfd120cff8a4811154b80a177723d914dda190f295b857a931e924c978edc4
```

The root multiarch apt route, BPQ32 on Windows, a physical rig, RF and
Raspberry Pi were not run. No external program binary is in the repository.

## LinBPQ's wire behaviour

These are black-box observations of LinBPQ 6.0.26.1, from the proxy and
stand-in rigctld logs in the experiments below. `tests/bpq_vara_session.rs`
replays the startup, session events, reconnect and scanner commands. It
manually advances scan decisions; the real rigctld log proves actual scan
behaviour. The tests use virtual time and cannot prove decode performance.

| Event | Observed behaviour |
|---|---|
| Connect command/data ports | Startup block about 1 s later: MYCALL N0GW N0GW-1, BW2300, RETRIES 20, LISTEN ON, all in one write. |
| Scan hop | LISTEN OFF, optional BW2300, LISTEN ON, with an F command to rigctld. |
| PENDING | Scanner parks. The pending hold lasts six dwell intervals. |
| CONNECTED | Scanner stays parked through DISCONNECTED. |
| DISCONNECTED | LISTEN ON and BW2300 immediately; scanning resumes about 15 s later. |
| CANCELPENDING | In this run the next hop was about 10 s later, not within the plan's predicted 5 s. The experiment allows 20 s. |
| Gateway restart | LinBPQ reconnects and resends the startup block. |

Earlier phase black-box probes on 2026-11-09 and 2026-11-10, using this
same LinBPQ binary, observed no command-reply timeout within 146 s and a
node dial wait of at least 416 s. Those are lower bounds, not a documented
LinBPQ timeout. The current experiment independently enforces a 1 s
command-reply bar. CALLTIMEOUT remains unsupported and gets WRONG.

## Recipe corrections found by running it

LinBPQ's i386 libc failed on XFS and overlay inode numbers above 2**32
with `Value too large for defined data type`. The experiment keeps LinBPQ's
working files on tmpfs under `/dev/shm`. Downloaded executables stay outside
that no-exec mount.

The default BBS holds new users' mail. A message can be stored while the
normal `L` listing hides it. The experiment seeds a minimal `linmail.cfg`
with `DontHoldNewUsers = 1` and explicit 99999-byte RX/TX limits. Omitting
the limits caused `Message Rejected by Size Limit`, even though Pat exited
0. That failed experiment is why PASS also requires the stored subject and
body, the BBS log and the destination's `L` listing. This is a lab policy,
not advice to turn off review of unknown users on a public gateway.

The public [mail configuration guide](https://www.cantab.net/users/john.wiseman/Documents/MailServerConfiguration.html)
describes the user hold flag. The public [BBS commands](https://www.cantab.net/users/john.wiseman/Documents/BBSUserCommands.html)
document listing and user administration. A first telnet login asks for a
name; the experiment answers before listing. No HTML templates were used.

The initial TX log's 2730 ms includes a station ID. The encoded request
itself is 1365 ms, so `2 * request + 2 * 500 ms turnaround` reaches the
5 s floor, not the plan's predicted 6.5 s. A 500 ms poll rounds actual
repeats up. The helper's 2.730 s input still correctly returns 6.460 s.

The legacy Pat experiment initially transferred its message successfully
but failed its strict zero-overflow bar. The lab receive ring is now 16384
samples to absorb scheduling and synchronous decode work. No production
default or zero-overflow assertion changed.

## Clean-room provenance

LinBPQ was run only as an unmodified black box. Inputs were authored
configuration and TCP lines. Outputs inspected were its banner, generated
configuration, BBS help, logs and stored messages. No LinBPQ source, HTML
templates, or copyleft/proprietary modem code was read, copied or ported.
The implementation uses the ticket, this repository, published VARA command
conventions and the public LinBPQ documentation linked above.

## PR checklist for the runner

- [x] Clean-room: public documentation and unmodified black-box execution only; no copyleft or proprietary modem code read, ported or copied.
- [x] Copied code: no code copied from an external source.
- [x] No new dependency was introduced.
- [ ] Final workspace gate results are reported by the implementation phase.

## Live command and retry probe

The live pair answered the startup probes and keyed four requests for
RETRIES 3. The last outcome arrived 30.45 s after the final keying.

```text
MYCALL two=OK six=WRONG
RETRIES 20 OK 0.1 ms
RETRIES=20 OK 0.1 ms
CALLTIMEOUT 600 WRONG 0.1 ms
PENDING WRONG 0.1 ms
CANCELPENDING WRONG 0.1 ms
BUSY BUSY OFF 0.1 ms
RETRIES 3 OK 0.0 ms
RETRIES 3 TX starts= [0.06, 5.59, 11.09, 16.61] DISCONNECTED= [47.06]
```

## Restart trigger correction

Killing the gateway immediately on CONNECTED can happen before the BBS
banner reaches Pat. That leaves no outstanding caller data. In both
initial restart probes the caller sent KEEPALIVE every 30-35 s and stayed
connected through the 400 s experiment limit. The experiment now waits
for a nonzero BUFFER on the caller after CONNECTED, then kills the gateway.
This proves recovery of an interrupted B2F transfer. Idle-peer liveness
remains a limitation; the earlier 263 s observation is not a general bound.

## Full real-program run

Command: `tools/bpq-loop/run.sh --keep`. Exit 0. This run used the same
production DSP/session code as the fresh-clone run below; the later review
fixes bound unknown-command log memory and normalize a relative cache path.
Both have regression tests. No waveform or B2F behaviour changed.

```text
bpq-loop: LinBPQ sha256 6b5be6ffc5c0c1304d3e4175958ce2c78967ac4e8463474ce29744dc8aee748d
bpq-loop: G8BPQ AX25 Packet Switch System Version 6.0.26.1 October 2026
bpq-loop: scratch /tmp/bpq-loop.yffkthuk, ports A=46825/46826 B=55507/55508 rigctld=54433
session pat_exit=0 connected="CONNECTED N1CALL N0GW-1 2400" stored=yes listed=yes bbs_log=yes scan_hops_during_session=0 max_reply_ms=363.9
stall cancelpending=yes scan_resumed=yes max_reply_ms=363.9
restart pat_exit=0 connected="CONNECTED N1CALL N0GW-1 2400" stored=yes listed=yes bbs_log=yes scan_hops_during_session=0 max_reply_ms=363.9
restart first_exit=1 startup_block_resent=yes second_exit=0
PASS
bpq-loop: kept /tmp/bpq-loop.yffkthuk
```

## Stalled caller cleanup before restart

A later validation run of `tools/bpq-loop/run.sh --keep` exited 1 at the
restart scenario. The stall scenario had resumed the caller and closed its
command socket, but closing a host socket does not end a dial. Pat's
restart dial reached the caller about 0.5 s before the old dial's handshake
reaper ended it, so coppa refused it with `Connect request refused: another
session is active` and the CONNECTED wait timed out. The standalone
`--scenario restart` run passed.

The stall scenario now sends `ABORT` on the caller's command socket and
waits up to 40 s for `DISCONNECTED` before closing the socket. A dial the
reaper ended first has already pushed its `DISCONNECTED`, so either path
leaves the caller with no live dial. Offline tests in
`tools/bpq-loop/test_bpq_loop.py` cover both paths and the failure case.

The rerun below exited 0. In this run the caller's dial was 34 s old when
it resumed, so its reaper ended it at once and the wait consumed that
`DISCONNECTED`. The ABORT path was not exercised live. The daemon test
`abort_cuts_the_keyed_frame_and_pushes_disconnected_at_once` shows that
coppa answers ABORT during a dial with `OK`, `PTT OFF` and `DISCONNECTED`.

```text
bpq-loop: LinBPQ sha256 6b5be6ffc5c0c1304d3e4175958ce2c78967ac4e8463474ce29744dc8aee748d
bpq-loop: G8BPQ AX25 Packet Switch System Version 6.0.26.1 October 2026
bpq-loop: scratch /tmp/bpq-loop.ysirt7ue, ports A=59573/59574 B=45713/45714 rigctld=39117
session pat_exit=0 connected="CONNECTED N1CALL N0GW-1 2400" stored=yes listed=yes bbs_log=yes scan_hops_during_session=0 max_reply_ms=395.0 
stall cancelpending=yes scan_resumed=yes max_reply_ms=395.0
restart pat_exit=0 connected="CONNECTED N1CALL N0GW-1 2400" stored=yes listed=yes bbs_log=yes scan_hops_during_session=0 max_reply_ms=439.9 first_exit=1 startup_block_resent=yes second_exit=0
PASS
bpq-loop: kept /tmp/bpq-loop.ysirt7ue
```

## Fresh clone, fresh HOME and empty cache

The runner owns the final commit and push, so this test cloned HEAD and
overlaid only the final git-listed working files, excluding platform output
and the thoughts tree. It did not test a published commit. All scratch
paths are per-run. The outer HOME, XDG config and cache directories started
empty. The first release build used the same source overlay in the first
reproduction clone; the final review fixes were compiled from a fourth
fresh clone, whose final-session output is recorded below.

Commands, with COP172_CHECKOUT set to the writable checkout:

```sh
git clone --shared --no-hardlinks "$COP172_CHECKOUT" /tmp/cop172-repro3/repo
# Overlay the changed tracked and untracked source files from the checkout.
# Paths came from git diff --name-only HEAD and git ls-files --others,
# excluding .catalyst-output and thoughts. No .git or credential file was copied.
mkdir /tmp/cop172-repro3/home /tmp/cop172-repro3/cache /tmp/cop172-repro3/config
HOME=/tmp/cop172-repro3/home XDG_CACHE_HOME=/tmp/cop172-repro3/cache XDG_CONFIG_HOME=/tmp/cop172-repro3/config /tmp/cop172-repro3/repo/tools/bpq-loop/run.sh   --coppad "$COP172_CHECKOUT/target/release/coppad" --keep
```

Exit 0. Verbatim output:

```text
bpq-loop: LinBPQ sha256 6b5be6ffc5c0c1304d3e4175958ce2c78967ac4e8463474ce29744dc8aee748d
bpq-loop: G8BPQ AX25 Packet Switch System Version 6.0.26.1 October 2026
bpq-loop: scratch /tmp/bpq-loop.1jxzhg49, ports A=43823/43824 B=52111/52112 rigctld=35841
session pat_exit=0 connected="CONNECTED N1CALL N0GW-1 2400" stored=yes listed=yes bbs_log=yes scan_hops_during_session=0 max_reply_ms=404.3 
stall cancelpending=yes scan_resumed=yes max_reply_ms=404.3
restart pat_exit=0 connected="CONNECTED N1CALL N0GW-1 2400" stored=yes listed=yes bbs_log=yes scan_hops_during_session=0 max_reply_ms=429.7 first_exit=1 startup_block_resent=yes second_exit=0
PASS
bpq-loop: kept /tmp/bpq-loop.1jxzhg49
```

## Pat regression after extracting the downloader

`tools/pat-loop/run.sh --direction a` exited 0 with the larger lab ring:

```text
pat-loop: Pat v1.0.0 (sha256 OK), coppad /workspace/tmp/catalyst-runner-workspace-L2ktCe/target/release/coppad
pat-loop: scratch /tmp/pat-loop.ru4uFh, ports A=58201/58202 B=59603/59604
N1CALL->N0CALL  exit=0  elapsed=46s  body_match=yes  overflow_lines=0  keepalives=0
PASS
```

## Automated checks before the final gate

- Eleven `cop172` daemon tests passed, including 0/2/20 retry counts,
  no early repeat, per-client reset, the 300 s cap, stale-session reuse,
  two IAMALIVE ticks during a long dial and immediate command replies.
- Four `bpq_vara_session` tests, the unchanged Pat VARA socket test and
  three `bpq_doc` tests passed.
- Five offline Python checks passed, including relative-cache launch paths.
- Bash syntax and shellcheck passed for both lab entries and the downloader.
- The feature-enabled workspace library suite passed 2198 tests.
- Cargo semver-checks 0.51.0 passed against main 28d4c9a after the minor bump.
  Its first attempt lacked libudev.pc. Ubuntu libudev-dev/libudev1 packages
  were unpacked outside the repository and PKG_CONFIG_PATH pointed there.
- The wiki body hash matches its manifest entry.
- The final all-targets build, full workspace tests, clippy and fmt are
  reported by the implementation phase's terminal gate receipt.

## Final source rebuild and fresh-session check

After both review fixes, a fourth fresh clone received the final working
source overlay. HOME, XDG config and cache were empty. These commands
compiled that clone, then followed the recipe through a working session:

```sh
HOME=/tmp/cop172-repro4/home XDG_CACHE_HOME=/tmp/cop172-repro4/cache XDG_CONFIG_HOME=/tmp/cop172-repro4/config CARGO_TARGET_DIR="$COP172_CHECKOUT/target" cargo build --manifest-path /tmp/cop172-repro4/repo/Cargo.toml   --release -p coppa-daemon --bin coppad --features cpal-backend
HOME=/tmp/cop172-repro4/home XDG_CACHE_HOME=/tmp/cop172-repro4/cache XDG_CONFIG_HOME=/tmp/cop172-repro4/config /tmp/cop172-repro4/repo/tools/bpq-loop/run.sh --scenario session   --coppad "$COP172_CHECKOUT/target/release/coppad" --keep
```

Both commands exited 0. Verbatim experiment output:

```text
bpq-loop: LinBPQ sha256 6b5be6ffc5c0c1304d3e4175958ce2c78967ac4e8463474ce29744dc8aee748d
bpq-loop: G8BPQ AX25 Packet Switch System Version 6.0.26.1 October 2026
bpq-loop: scratch /tmp/bpq-loop.u_0aulrw, ports A=49835/49836 B=58805/58806 rigctld=49927
session pat_exit=0 connected="CONNECTED N1CALL N0GW-1 2400" stored=yes listed=yes bbs_log=yes scan_hops_during_session=0 max_reply_ms=400.2 
PASS
bpq-loop: kept /tmp/bpq-loop.u_0aulrw
```

The Pat ARDOP socket integration test also passed after the shared
callsign-setter change. Review found and fixed two defects: unbounded
unknown-word retention and a relative-cache path that broke after chdir.
Their regression tests pass. No second review pass was run.
