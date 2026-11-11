---
id: linbpq-gateway
title: What will bite you when LinBPQ drives coppa as a VARA-style TNC?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-daemon/src/event_loop.rs
  - crates/coppa-host/src/vara/protocol.rs
  - docs/BPQ.md
  - tools/bpq-loop/run.sh
verified:
  commit: 28d4c9a
  date: 2026-11-11
links:
  - overview
---
LinBPQ's Linux download is i386. An x86-64 host needs its loader and
libraries. Its libc can also fail on inode numbers above 2**32; the lab
uses a tmpfs working directory. [BPQ.md](../../docs/BPQ.md) has the recipe.

MYCALL lists matter because the BBS application call can differ from the
node call. coppa answers up to five callsigns and reports the dialled one
in CONNECTED. The first remains the identity used for outbound calls.
RETRIES accepts spaces or an equals sign, up to 20, per command client.
Pat's default remains one request per dial.

PENDING parks the scanner. Keep a dwell of at least 7 s to cover the
inbound timeout; LISTEN OFF ends an unfinished attempt if the scanner
moves first. Match the APPLICATION number to the BBS application or the
caller gets `Application not available`.

The cable test uses LinBPQ's local BBS, not an RMS or CMS forwarding queue.
The work is clean-room. LinBPQ ran as an unmodified black box, and no
LinBPQ source or HTML templates were read or copied. See the run record
linked from BPQ.md for program versions and results.
