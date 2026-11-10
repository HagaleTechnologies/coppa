---
id: daemon-test-ports
title: What will bite you when a test hands coppad a port?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-daemon/tests/support/mod.rs
  - crates/coppa-daemon/tests/runtime_smoke.rs
  - crates/coppa-daemon/tests/cli.rs
  - crates/coppa-daemon/src/vara_wiring.rs
verified:
  commit: 2662e93
  date: 2026-11-09
links:
  - overview
---
`support::free_ports` binds two loopback listeners on port 0, reads their
ports and drops the listeners before the test spawns `coppad`. Between that
drop and coppad's own bind, any other process on the host can take the port.
coppad then exits 1 during start-up with `failed to bind the VARA command
port` (or `data port`) and `Address already in use`, and
`Daemon::wait_for_count` panics with `coppad exited with ... before logging
"Daemon ready"`. COP-163 saw this
twice on the macOS Platform Check (CI runs 42127728326 and 42194766433), in
tests that did not touch start-up. A rerun passed both times.

Picking another free port does not help: every pick-then-release has the same
gap. Let coppad bind instead. Set `vara_command_port = 0` and
`vara_data_port = 0`, and read the ports it bound from its
`VARA TCP server listening command_addr=... data_addr=...` line
(`vara_wiring::serve_vara` logs the listeners' real addresses).
`Daemon::vara_ports` in the test support does this after `Daemon ready`, and
panics with the captured output if the line or an address is missing. The
runtime smoke tests use it, and
`two_coppads_on_port_zero_bind_and_report_distinct_vara_ports` checks two
daemons on port 0 at once. `cli.rs` reads `command_addr=` the same way
(`a_library_message_is_a_tracing_line_not_a_bare_print`).

The reload and PTT runners (`reload.rs`, `ptt_release.rs`) still use
`free_ports` and carry the same, rarer, race. Move a test to port 0 when it
next flakes this way. A start-up failure prints coppad's stdout and stderr,
so check its stderr for `Address already in use` before suspecting the code
under test.
