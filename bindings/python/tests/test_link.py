"""The Python Link: version, ABI check, an in-process pair, and scenario 1
(two OS processes hold a link over a socketpair)."""

import json
import os
import re
import socket
import subprocess
import sys
import threading
import unittest
from array import array
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import coppa  # noqa: E402
from coppa import _ffi  # noqa: E402

REPO = Path(__file__).resolve().parents[3]
EXAMPLE = REPO / "bindings" / "python" / "examples" / "two_process_link.py"
PAYLOAD = b"hello from process A\x00\xff\xfe\n"
BLOCK = 960


def workspace_version():
    text = (REPO / "Cargo.toml").read_text(encoding="utf-8")
    section = text.split("[workspace.package]", 1)[1]
    return re.search(r'^version\s*=\s*"([^"]+)"', section, re.M).group(1)


class _FakeFunc:
    def __init__(self, value):
        self.value = value

    def __call__(self):
        return self.value


class _FakeLib:
    def __init__(self, packed):
        self.coppa_abi_version = _FakeFunc(packed)


class LinkTests(unittest.TestCase):
    def test_version_and_abi(self):
        self.assertEqual(coppa.version(), workspace_version())
        self.assertEqual(coppa.abi_version(), (1, 1))

    def test_air_protocol_and_host_api_versions(self):
        # COP-178: separate fields, the same values TCP and WebSocket report.
        self.assertEqual(coppa.air_protocol_version(), 1, "air protocol v1")
        self.assertEqual(coppa.host_api_version(), 1, "host API v1")
        self.assertIsInstance(coppa.version(), str)

    def test_abi_check_rejects_abi_1_0_before_resolving_new_symbols(self):
        # An ABI 1.0 library lacks coppa_air_protocol_version: the check must
        # refuse it by its ABI number, before any symbol is resolved.
        self.assertEqual(_ffi.REQUIRED_ABI, (1, 1))
        with self.assertRaises(coppa.CoppaError) as cm:
            _ffi.check_abi(_FakeLib(1 << 16), "/old/libcoppa_ffi.so")
        self.assertIn("has ABI 1.0", str(cm.exception))
        self.assertIn("x >= 1", str(cm.exception))

    def test_abi_check_rejects_old_library(self):
        with self.assertRaises(coppa.CoppaError) as cm:
            _ffi.check_abi(object(), "/old/libcoppa_ffi.so")
        self.assertIn("is too old", str(cm.exception))
        with self.assertRaises(coppa.CoppaError) as cm:
            _ffi.check_abi(_FakeLib(2 << 16), "/new/libcoppa_ffi.so")
        self.assertIn("has ABI 2.0", str(cm.exception))
        self.assertEqual(_ffi.check_abi(_FakeLib((1 << 16) | 3), "x"), (1, 3))

    def test_in_process_pair_connects_and_delivers(self):
        a = coppa.Link("N0AAA")
        b = coppa.Link("N0BBB")
        b.listen(True)
        a.connect("N0BBB", 0)
        a_rx = array("f", bytes(4 * BLOCK))
        b_rx = array("f", bytes(4 * BLOCK))
        events = {"a": [], "b": []}
        got = bytearray()
        sent = False
        for tick in range(1, 6001):
            now = tick * 20
            a_tx, _ = a.process(now, a_rx, BLOCK)
            b_tx, _ = b.process(now, b_rx, BLOCK)
            a_rx, b_rx = b_tx, a_tx
            for name, link in (("a", a), ("b", b)):
                while (ev := link.poll_event()) is not None:
                    events[name].append(ev)
            if not sent and a.state == coppa.LinkState.CONNECTED:
                a.send(PAYLOAD)
                sent = True
            got.extend(b.recv())
            if sent and len(got) >= len(PAYLOAD) and a.pending_bytes == 0 and a.state == coppa.LinkState.CONNECTED:
                a.disconnect(now)
            if len(events["a"]) >= 2 and len(events["b"]) >= 2:
                break
        a.close()
        b.close()
        self.assertEqual(bytes(got), PAYLOAD)
        self.assertEqual([e.kind for e in events["a"]], [coppa.EventKind.CONNECTED, coppa.EventKind.DISCONNECTED])
        self.assertEqual([e.kind for e in events["b"]], [coppa.EventKind.CONNECTED, coppa.EventKind.DISCONNECTED])
        self.assertEqual(events["a"][0].remote, "N0BBB")
        self.assertEqual(events["b"][0].remote, "N0AAA")
        self.assertEqual(events["a"][1].reason, coppa.DisconnectReason.LOCAL)
        self.assertEqual(events["b"][1].reason, coppa.DisconnectReason.REMOTE)

    def test_two_threads_recv_get_each_byte_once(self):
        """The thread rule: two threads calling recv() on one Link get every
        delivered byte exactly once between them."""
        payload = bytes(i % 251 for i in range(680))
        # Speed level 8 delivers the payload in a few frames; the race is in
        # recv(), not on the air.
        a = coppa.Link("N0AAA", speed_level=8)
        b = coppa.Link("N0BBB", speed_level=8)
        got = [bytearray(), bytearray()]
        try:
            b.listen(True)
            a.connect("N0BBB", 0)
            a_rx = array("f", bytes(4 * BLOCK))
            b_rx = array("f", bytes(4 * BLOCK))
            sent = False
            for tick in range(1, 30001):
                now = tick * 20
                a_tx, _ = a.process(now, a_rx, BLOCK)
                b_tx, _ = b.process(now, b_rx, BLOCK)
                a_rx, b_rx = b_tx, a_tx
                if not sent and a.state == coppa.LinkState.CONNECTED:
                    a.send(payload)
                    sent = True
                if sent and a.pending_bytes == 0:
                    break
            self.assertTrue(sent and a.pending_bytes == 0, "payload was not delivered")

            errors = []

            def drain(out):
                try:
                    while chunk := b.recv(1):
                        out.extend(chunk)
                except Exception as e:  # noqa: BLE001 - asserted after join
                    errors.append(e)

            threads = [threading.Thread(target=drain, args=(out,)) for out in got]
            for t in threads:
                t.start()
            for t in threads:
                t.join()
        finally:
            a.close()
            b.close()
        self.assertEqual(errors, [])
        self.assertEqual(len(got[0]) + len(got[1]), len(payload))
        self.assertEqual(Counter(got[0] + got[1]), Counter(payload))

    def test_two_processes_connect_and_send(self):
        """Scenario 1: two processes using the binding on a loopback channel;
        one connects and sends, the other receives the bytes."""
        a, b = socket.socketpair()
        env = os.environ.copy()
        procs = {}
        try:
            for role, sock in (("listen", b), ("send", a)):
                fd = sock.fileno()
                procs[role] = subprocess.Popen(
                    [sys.executable, str(EXAMPLE), "--role", role, "--fd", str(fd)],
                    pass_fds=[fd],
                    env=env,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                )
        finally:
            a.close()
            b.close()
        outs = {}
        for role, p in procs.items():
            out, err = p.communicate(timeout=300)
            outs[role] = out
            self.assertEqual(p.returncode, 0, f"{role} failed:\n{out}\n{err}")
        listener_lines = [l for l in outs["listen"].splitlines() if l.startswith("{")]
        self.assertTrue(listener_lines, outs["listen"])
        result = json.loads(listener_lines[-1])
        self.assertEqual(bytes.fromhex(result["received_hex"]), PAYLOAD)
        pids = {procs["listen"].pid, procs["send"].pid, os.getpid()}
        self.assertEqual(len(pids), 3)
        self.assertEqual(result["pid"], procs["listen"].pid)
        self.assertIn("connected to N0BBB", outs["send"])
        self.assertIn("connected to N0AAA", outs["listen"])


if __name__ == "__main__":
    unittest.main()
