"""Low-level ctypes declarations for libcoppa_ffi (COP-139).

This module finds and loads the shared library, checks its ABI version, and
declares the argument and return types of every C function the binding uses.
The constants mirror ``crates/coppa-ffi/coppa.h``; ``tests/test_header_constants.py``
fails if they drift.
"""

import ctypes
import ctypes.util
import os
import sys
from pathlib import Path

# ABI this binding was written against: major must match, minor must be >= this.
REQUIRED_ABI = (1, 0)

# Error codes (coppa.h COPPA_ERR_*).
OK = 0
ERR_NULL_ARG = -1
ERR_INVALID_UTF8 = -2
ERR_CODEC = -3
ERR_POISONED = -4
ERR_PANIC = -5
ERR_INVALID_PARAM = -6
ERR_INVALID_STATE = -7
NONE_PENDING = 1

# ABI version (coppa.h COPPA_ABI_VERSION_*).
ABI_VERSION_MAJOR = 1
ABI_VERSION_MINOR = 0

# Link states, events and disconnect reasons (coppa.h COPPA_LINK_*).
LINK_STATE_IDLE = 0
LINK_STATE_CONNECTING = 1
LINK_STATE_CONNECTED = 2
LINK_STATE_DISCONNECTING = 3
LINK_EVENT_CONNECTED = 1
LINK_EVENT_DISCONNECTED = 2
LINK_REASON_NONE = 0
LINK_REASON_LOCAL = 1
LINK_REASON_REMOTE = 2
LINK_REASON_TIMEOUT = 3
LINK_REASON_RETRIES_EXHAUSTED = 4
LINK_REASON_HANDSHAKE_FAILED = 5


class CoppaError(Exception):
    """A failed coppa call. ``code`` is the C return code (None when the
    library could not be loaded) and ``message`` is the library's text."""

    def __init__(self, code, message):
        super().__init__(code, message)
        self.code = code
        self.message = message

    def __str__(self):
        if self.code is None:
            return self.message
        return f"[{self.code}] {self.message}"


class ConfigStruct(ctypes.Structure):
    """Mirror of ``coppa_config_t``."""

    _fields_ = [
        ("sample_rate", ctypes.c_uint32),
        ("profile", ctypes.c_char_p),
        ("speed_level", ctypes.c_uint8),
        ("callsign", ctypes.c_char_p),
    ]


class LinkConfigStruct(ctypes.Structure):
    """Mirror of ``coppa_link_config_t`` (32 bytes on a 64-bit build)."""

    _fields_ = [
        ("struct_size", ctypes.c_uint32),
        ("callsign", ctypes.c_char_p),
        ("profile", ctypes.c_char_p),
        ("speed_level", ctypes.c_uint8),
        ("turnaround_ms", ctypes.c_uint32),
    ]


class LinkEventStruct(ctypes.Structure):
    """Mirror of ``coppa_link_event_t``."""

    _fields_ = [
        ("kind", ctypes.c_int32),
        ("reason", ctypes.c_int32),
        ("remote", ctypes.c_char * 16),
    ]


_c_float_p = ctypes.POINTER(ctypes.c_float)
_size_p = ctypes.POINTER(ctypes.c_size_t)
_u8_p = ctypes.POINTER(ctypes.c_uint8)
_vp = ctypes.c_void_p
_i32 = ctypes.c_int32

# One line per function: name -> (restype, argtypes).
_DECLS = {
    "coppa_version": (ctypes.c_char_p, []),
    "coppa_abi_version": (ctypes.c_uint32, []),
    "coppa_last_error": (ctypes.c_char_p, []),
    "coppa_strerror": (ctypes.c_char_p, [_i32]),
    "coppa_engine_new_with": (_vp, [ctypes.POINTER(ConfigStruct)]),
    "coppa_engine_destroy": (None, [ctypes.POINTER(_vp)]),
    "coppa_link_new": (_vp, [ctypes.POINTER(LinkConfigStruct)]),
    "coppa_link_destroy": (None, [ctypes.POINTER(_vp)]),
    "coppa_link_listen": (_i32, [_vp, ctypes.c_bool]),
    "coppa_link_connect": (_i32, [_vp, ctypes.c_char_p, ctypes.c_uint64]),
    "coppa_link_send": (_i32, [_vp, _u8_p, ctypes.c_size_t]),
    "coppa_link_disconnect": (_i32, [_vp, ctypes.c_uint64]),
    "coppa_link_process": (
        _i32,
        [_vp, ctypes.c_uint64, _c_float_p, ctypes.c_size_t, _c_float_p, ctypes.c_size_t, _size_p],
    ),
    "coppa_link_recv": (_i32, [_vp, _u8_p, ctypes.c_size_t, _size_p]),
    "coppa_link_next_event": (_i32, [_vp, ctypes.POINTER(LinkEventStruct)]),
    "coppa_link_state": (_i32, [_vp]),
    "coppa_link_pending": (_i32, [_vp, _size_p]),
}


def _lib_name():
    if sys.platform == "darwin":
        return "libcoppa_ffi.dylib"
    if sys.platform.startswith("win"):
        return "coppa_ffi.dll"
    return "libcoppa_ffi.so"


def candidate_paths():
    """Library paths in search order: $COPPA_LIB, target/release, target/debug."""
    paths = []
    env = os.environ.get("COPPA_LIB")
    if env:
        paths.append(Path(env))
    repo = Path(__file__).resolve().parents[3]
    name = _lib_name()
    paths.append(repo / "target" / "release" / name)
    paths.append(repo / "target" / "debug" / name)
    return paths


def check_abi(lib, path):
    """Raise CoppaError unless ``lib`` reports an ABI this binding supports."""
    func = getattr(lib, "coppa_abi_version", None)
    if func is None:
        raise CoppaError(
            None,
            f"libcoppa_ffi at {path} is too old: it has no coppa_abi_version "
            f"(this binding needs ABI {REQUIRED_ABI[0]}.{REQUIRED_ABI[1]} or newer)",
        )
    func.restype = ctypes.c_uint32
    func.argtypes = []
    packed = int(func())
    major, minor = packed >> 16, packed & 0xFFFF
    if major != REQUIRED_ABI[0] or minor < REQUIRED_ABI[1]:
        raise CoppaError(
            None,
            f"libcoppa_ffi at {path} has ABI {major}.{minor}; this binding needs "
            f"ABI {REQUIRED_ABI[0]}.x with x >= {REQUIRED_ABI[1]}",
        )
    return major, minor


def load():
    """Find, load, ABI-check and declare libcoppa_ffi. Returns (lib, path)."""
    tried = []
    for path in candidate_paths():
        tried.append(str(path))
        if path.is_file():
            return _open(str(path))
    found = ctypes.util.find_library("coppa_ffi")
    if found:
        return _open(found)
    raise CoppaError(
        None,
        "could not find libcoppa_ffi; build it with `cargo build -p coppa-ffi --release` "
        "or set COPPA_LIB to its path (tried: " + ", ".join(tried) + ")",
    )


def _open(path):
    lib = ctypes.CDLL(path)
    check_abi(lib, path)
    for name, (restype, argtypes) in _DECLS.items():
        func = getattr(lib, name)
        func.restype = restype
        func.argtypes = argtypes
    return lib, path


class _LazyLib:
    """Loads the library on first attribute access, so importing the package
    for its pure-Python parts (tests, ABI checks) does not need the library."""

    def __init__(self):
        self._lib = None
        self.path = None

    def __getattr__(self, name):
        if self._lib is None:
            self._lib, self.path = load()
        return getattr(self._lib, name)


lib = _LazyLib()
