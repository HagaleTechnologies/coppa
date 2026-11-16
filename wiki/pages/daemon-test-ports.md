---
id: daemon-test-ports
title: What will bite you when a test hands coppad a port?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-daemon/tests/support/mod.rs
  - crates/coppa-daemon/tests/runtime_smoke.rs
  - crates/coppa-daemon/tests/ptt_release.rs
  - crates/coppa-daemon/tests/reload.rs
  - crates/coppa-daemon/tests/cli.rs
  - crates/coppa-daemon/src/vara_wiring.rs
verified:
  commit: 06a2987
  date: 2026-11-16
links:
  - overview
---
A test that binds a loopback listener on port 0, reads its port and drops
the listener before spawning `coppad` (what the removed `support::free_ports`
did) leaves a gap. Between that drop and coppad's own bind, any other process
on the host can take the port.
coppad then exits 1 during start-up with `failed to bind the VARA command
port` (or `data port`) and `Address already in use`, and the test's
`wait_for_log` panics with `coppad exited with ... before logging
"Daemon ready"`. COP-163 saw this
twice on the macOS Platform Check (CI runs 42127728326 and 42194766433), in
tests that did not touch start-up. A rerun passed both times.

Picking another free port does not help: every pick-then-release has the same
gap. Let coppad bind instead. Set `vara_command_port = 0` and
`vara_data_port = 0`, and read the ports it bound from its
`VARA TCP server listening command_addr=... data_addr=...` line
(`vara_wiring::serve_vara` logs the listeners' real addresses).
`support::vara_ports_from` parses that line, and panics with the captured
output if the line or an address is missing, malformed, not loopback or
port 0. `Daemon::vara_ports` calls it after `Daemon ready`, and
`ptt_release.rs`, which has its own `Coppad` runner, calls it directly. The
runtime smoke tests use `Daemon::vara_ports`, and
`two_coppads_on_port_zero_bind_and_report_distinct_vara_ports` checks two
daemons on port 0 at once. `cli.rs` reads `command_addr=` the same way
(`a_library_message_is_a_tracing_line_not_a_bare_print`).

COP-208 moved `reload.rs` and `ptt_release.rs` to port 0 after the same race
failed `clean_sigterm_leaves_the_guardian_quiet` on Linux in merge-queue run
43645980361, and deleted `free_ports`. A start-up failure prints coppad's
stdout and stderr, so check its stderr for `Address already in use` before
suspecting the code under test.
