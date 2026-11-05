"""Scenario 2 seen from Python: a failed call carries the library's text."""

import ctypes
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import coppa  # noqa: E402
from coppa import _ffi  # noqa: E402


class ErrorTests(unittest.TestCase):
    def test_invalid_callsign_raises_with_library_text(self):
        with self.assertRaises(coppa.CoppaError) as cm:
            coppa.Link("N0 CALL")
        e = cm.exception
        self.assertEqual(e.code, -6)
        self.assertIn("Invalid callsign character: ' '", str(e))
        self.assertTrue(str(e).startswith("[-6] coppa_link_new:"), str(e))

    def test_send_while_idle_raises_invalid_state(self):
        with coppa.Link("N0AAA") as link:
            self.assertEqual(link.state, coppa.LinkState.IDLE)
            with self.assertRaises(coppa.CoppaError) as cm:
                link.send(b"hi")
        self.assertEqual(cm.exception.code, coppa.ERR_INVALID_STATE)
        self.assertIn("link is not connected (state IDLE)", str(cm.exception))

    def test_now_ms_backwards_raises(self):
        with coppa.Link("N0AAA") as link:
            link.process(1000, b"", 960)
            with self.assertRaises(coppa.CoppaError) as cm:
                link.process(900, b"", 960)
        self.assertEqual(cm.exception.code, -6)
        self.assertIn("earlier than the previous call's", str(cm.exception))

    def test_raw_ffi_failure_has_last_error(self):
        cfg = _ffi.ConfigStruct()
        cfg.profile = b"NOPE"
        handle = _ffi.lib.coppa_engine_new_with(ctypes.byref(cfg))
        self.assertIsNone(handle)
        self.assertIn('unknown profile "NOPE"', coppa.last_error())

    def test_strerror(self):
        self.assertIn("destroy it and create a new one", coppa.strerror(-4))
        self.assertEqual(coppa.strerror(-42), "unknown coppa error code")


if __name__ == "__main__":
    unittest.main()
