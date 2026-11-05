---
id: mdns-discovery
title: What will bite you when testing coppad's mDNS discovery?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-daemon/src/mdns.rs
  - crates/coppa-daemon/tests/cli.rs
  - docs/OPERATING.md
verified:
  commit: 2526b50
  date: 2026-11-03
links:
  - overview
---
COP-140's `mdns` feature advertises `coppad.local.` and one `_coppa._tcp`
service with the in-process `mdns-sd` crate. With `coppad --instance NAME`
(COP-85) both names become `coppad-NAME` (`mdns::Names::for_instance`). How to build, enable and probe it
is in [OPERATING.md](../../docs/OPERATING.md#finding-coppad-on-the-network-mdns).
Two traps make a same-host test pass or fail for the wrong reason.

## mdns-sd answers on loopback unless told not to

By default `mdns-sd` binds every interface, loopback included. A browser in a
second process on the same host then resolves `coppad.local.` to `::1` or
`127.0.0.1`, and a round-trip test passes although no other host could reach
the daemon. The responder disables `IfKind::LoopbackV4` and
`IfKind::LoopbackV6`; with them disabled, the same-host lookup returns the LAN
address. The round-trip test (`the_daemon_can_be_found_as_coppad_local`)
asserts a non-loopback address, so a loopback-only answer fails it. Keep both.

Related, seen while probing: `mdns-sd` announces an explicit address only on
the interface whose subnet holds it, so an address on no local interface is
never announced. Two responders on one host with the same address raise no
`NameChange`: identical records are not a conflict under RFC 6762, so the
rename to `coppad-2.local.` cannot be staged on one host. An instance named
`2` advertises that same `coppad-2.local.` on purpose, so a renamed
`coppad.local` and `coppad --instance 2` on one LAN look alike; pick instance
names that are not bare digits.

## getent and glibc do not resolve .local without nss-mdns

`getent hosts coppad.local` (and so `curl http://coppad.local:8400/`) exits 2
on a Linux host whose `/etc/nsswitch.conf` lacks `mdns4_minimal`, even while
the responder answers. Do not assume a CI runner has nss-mdns. A test must
resolve with an mDNS client (`mdns_sd::ServiceDaemon::browse` and
`resolve_hostname`), then connect to the address it returns. Phones, macOS,
Windows and browsers resolve `.local` themselves; a Linux client needs Avahi's
nss-mdns.
