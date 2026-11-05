"""The Python constant mirror matches coppa.h, and the struct layouts match."""

import ctypes
import re
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from coppa import _ffi  # noqa: E402

HEADER = Path(__file__).resolve().parents[3] / "crates" / "coppa-ffi" / "coppa.h"


def header_defines():
    text = HEADER.read_text(encoding="utf-8")
    return {m.group(1): int(m.group(2)) for m in re.finditer(r"#define (COPPA_\w+) \(?(-?\d+)\)?", text)}


class HeaderConstantTests(unittest.TestCase):
    def test_constants_match_header(self):
        defines = header_defines()
        wanted = [
            name
            for name in defines
            if name.startswith(("COPPA_ERR_", "COPPA_LINK_", "COPPA_ABI_VERSION_"))
            or name in ("COPPA_OK", "COPPA_NONE_PENDING")
        ]
        self.assertTrue(wanted, f"no COPPA_ constants found in {HEADER}")
        for name in ("COPPA_OK", "COPPA_NONE_PENDING", "COPPA_ERR_INVALID_STATE",
                     "COPPA_ABI_VERSION_MAJOR", "COPPA_LINK_REASON_HANDSHAKE_FAILED"):
            self.assertIn(name, wanted)
        for name in wanted:
            py_name = name[len("COPPA_"):]
            self.assertTrue(hasattr(_ffi, py_name), f"_ffi has no mirror of {name}")
            self.assertEqual(getattr(_ffi, py_name), defines[name], name)

    @unittest.skipUnless(ctypes.sizeof(ctypes.c_void_p) == 8, "64-bit layout")
    def test_link_config_layout(self):
        s = _ffi.LinkConfigStruct
        self.assertEqual(ctypes.sizeof(s), 32)
        self.assertEqual(s.struct_size.offset, 0)
        self.assertEqual(s.callsign.offset, 8)
        self.assertEqual(s.profile.offset, 16)
        self.assertEqual(s.speed_level.offset, 24)
        self.assertEqual(s.turnaround_ms.offset, 28)

    def test_link_event_layout(self):
        s = _ffi.LinkEventStruct
        self.assertEqual(ctypes.sizeof(s), 24)
        self.assertEqual(s.remote.offset, 8)


if __name__ == "__main__":
    unittest.main()
