"""Python binding for coppa's C library (COP-139): drive a connected,
reliable coppa link from Python.

Standard library only (``ctypes``). Not published; see ``README.md`` next to
this package.

Rules that carry over from ``coppa.h``:

- Time: every ``now_ms`` you pass is a monotonic millisecond count from any
  epoch you choose. It must never decrease for one link.
- Threads: one ``Link`` may be used from several threads (the library locks
  inside the handle), but feed one audio stream from one thread only, and
  call ``close()`` only after every other thread is done with the link.
- Errors: every failed call raises ``CoppaError`` carrying the library's own
  message (``coppa_last_error``), for example
  ``[-7] coppa_link_send: link is not connected (state IDLE)``.

Example::

    import coppa
    link = coppa.Link("N0AAA")
    link.connect("N0BBB", now_ms=0)
    tx, active = link.process(20, rx_samples, tx_len=960)
"""

import ctypes
from array import array
from dataclasses import dataclass
from enum import IntEnum

from . import _ffi
from ._ffi import (
    ABI_VERSION_MAJOR,
    ABI_VERSION_MINOR,
    ERR_CODEC,
    ERR_INVALID_PARAM,
    ERR_INVALID_STATE,
    ERR_INVALID_UTF8,
    ERR_NULL_ARG,
    ERR_PANIC,
    ERR_POISONED,
    NONE_PENDING,
    OK,
    CoppaError,
)

__all__ = [
    "ABI_VERSION_MAJOR",
    "ABI_VERSION_MINOR",
    "CoppaError",
    "DisconnectReason",
    "ERR_CODEC",
    "ERR_INVALID_PARAM",
    "ERR_INVALID_STATE",
    "ERR_INVALID_UTF8",
    "ERR_NULL_ARG",
    "ERR_PANIC",
    "ERR_POISONED",
    "EventKind",
    "Link",
    "LinkEvent",
    "LinkState",
    "NONE_PENDING",
    "OK",
    "abi_version",
    "last_error",
    "strerror",
    "version",
]

#: Samples per second of every coppa audio stream.
SAMPLE_RATE = 48000


def version():
    """The loaded library's crate version, for example ``'0.5.0'``."""
    return _ffi.lib.coppa_version().decode("utf-8")


def abi_version():
    """The loaded library's C ABI version as ``(major, minor)``."""
    packed = int(_ffi.lib.coppa_abi_version())
    return (packed >> 16, packed & 0xFFFF)


def strerror(code):
    """The library's fixed description of an error code."""
    return _ffi.lib.coppa_strerror(int(code)).decode("utf-8", "replace")


def last_error():
    """The calling thread's message for its most recent failed coppa call,
    or None when that call succeeded."""
    raw = _ffi.lib.coppa_last_error()
    return None if raw is None else raw.decode("utf-8", "replace")


def _check(rc):
    """Raise CoppaError for a negative return code; return rc otherwise."""
    if rc < 0:
        raise CoppaError(rc, last_error() or strerror(rc))
    return rc


class LinkState(IntEnum):
    """``COPPA_LINK_STATE_*``."""

    IDLE = _ffi.LINK_STATE_IDLE
    CONNECTING = _ffi.LINK_STATE_CONNECTING
    CONNECTED = _ffi.LINK_STATE_CONNECTED
    DISCONNECTING = _ffi.LINK_STATE_DISCONNECTING


class EventKind(IntEnum):
    """``COPPA_LINK_EVENT_*``."""

    CONNECTED = _ffi.LINK_EVENT_CONNECTED
    DISCONNECTED = _ffi.LINK_EVENT_DISCONNECTED


class DisconnectReason(IntEnum):
    """``COPPA_LINK_REASON_*``."""

    NONE = _ffi.LINK_REASON_NONE
    LOCAL = _ffi.LINK_REASON_LOCAL
    REMOTE = _ffi.LINK_REASON_REMOTE
    TIMEOUT = _ffi.LINK_REASON_TIMEOUT
    RETRIES_EXHAUSTED = _ffi.LINK_REASON_RETRIES_EXHAUSTED
    HANDSHAKE_FAILED = _ffi.LINK_REASON_HANDSHAKE_FAILED


@dataclass(frozen=True)
class LinkEvent:
    """A link event: ``kind`` is an EventKind, ``remote`` the peer callsign,
    ``reason`` a DisconnectReason (NONE for CONNECTED)."""

    kind: EventKind
    remote: str
    reason: DisconnectReason


class Link:
    """One connected, reliable coppa session over 48 kHz mono float audio.

    ``process`` is the audio callback: pass the samples captured since the
    last call and get back the samples to play plus the signal length.
    Key the transmitter while that length is above zero.
    """

    def __init__(self, callsign, profile=None, speed_level=0, turnaround_ms=0):
        self._h = ctypes.c_void_p()
        self._tx_buf = None
        self._rx_buf = None
        cfg = _ffi.LinkConfigStruct()
        cfg.struct_size = ctypes.sizeof(_ffi.LinkConfigStruct)
        cfg.callsign = callsign.encode("utf-8")
        cfg.profile = None if profile is None else profile.encode("utf-8")
        cfg.speed_level = int(speed_level)
        cfg.turnaround_ms = int(turnaround_ms)
        handle = _ffi.lib.coppa_link_new(ctypes.byref(cfg))
        if not handle:
            raise CoppaError(ERR_INVALID_PARAM, last_error() or strerror(ERR_INVALID_PARAM))
        self._h = ctypes.c_void_p(handle)

    def _handle(self):
        if not self._h:
            raise CoppaError(ERR_NULL_ARG, "link is closed")
        return self._h

    def listen(self, on=True):
        """Answer incoming CONNECT requests addressed to this callsign."""
        _check(_ffi.lib.coppa_link_listen(self._handle(), bool(on)))

    def connect(self, remote, now_ms):
        """Start a connection to ``remote``."""
        _check(_ffi.lib.coppa_link_connect(self._handle(), remote.encode("utf-8"), int(now_ms)))

    def send(self, data):
        """Queue bytes for reliable delivery. Needs state CONNECTED."""
        if isinstance(data, (int, str)):
            # bytes(5) would queue five NUL bytes; refuse it instead.
            raise TypeError("send() needs a bytes-like object, not %s" % type(data).__name__)
        buf = bytes(memoryview(data).cast("B"))
        arr = (ctypes.c_uint8 * len(buf)).from_buffer_copy(buf) if buf else None
        _check(_ffi.lib.coppa_link_send(self._handle(), arr, len(buf)))

    def disconnect(self, now_ms):
        """Close the link once queued data is delivered."""
        _check(_ffi.lib.coppa_link_disconnect(self._handle(), int(now_ms)))

    def process(self, now_ms, rx, tx_len=None):
        """Run the link for one audio block.

        ``rx`` is any float32 buffer (``array('f')``, ``bytes`` read from a
        socket, a ``memoryview``), possibly empty. ``tx_len`` defaults to the
        number of rx samples. Returns ``(array('f') of tx_len samples, active)``
        where the first ``active`` samples are signal and the rest are zero.
        """
        view = memoryview(rx)
        if view.itemsize != 1 and view.format not in ("f", "<f", "=f"):
            # A float64 buffer read as float32 would decode as noise.
            raise TypeError("rx must be float32 samples or raw bytes, not format %r" % view.format)
        raw = view.cast("B")
        n_rx = len(raw) // 4
        if tx_len is None:
            tx_len = n_rx
        rx_arr = None
        if n_rx:
            rx_arr = (ctypes.c_float * n_rx).from_buffer_copy(raw[: n_rx * 4])
        if self._tx_buf is None or len(self._tx_buf) != tx_len:
            self._tx_buf = (ctypes.c_float * tx_len)()
        active = ctypes.c_size_t(0)
        out_ptr = self._tx_buf if tx_len else None
        _check(
            _ffi.lib.coppa_link_process(
                self._handle(), int(now_ms), rx_arr, n_rx, out_ptr, tx_len, ctypes.byref(active)
            )
        )
        return array("f", bytes(self._tx_buf)), active.value

    def recv(self, max_bytes=65536):
        """Return delivered bytes (up to ``max_bytes``), or b'' when none."""
        if self._rx_buf is None or len(self._rx_buf) != max_bytes:
            self._rx_buf = (ctypes.c_uint8 * max_bytes)()
        buf = self._rx_buf
        out_len = ctypes.c_size_t(0)
        _check(_ffi.lib.coppa_link_recv(self._handle(), buf, max_bytes, ctypes.byref(out_len)))
        return bytes(buf[: out_len.value])

    def poll_event(self):
        """Return the next LinkEvent, or None when none is pending."""
        ev = _ffi.LinkEventStruct()
        rc = _check(_ffi.lib.coppa_link_next_event(self._handle(), ctypes.byref(ev)))
        if rc == NONE_PENDING:
            return None
        return LinkEvent(
            EventKind(ev.kind),
            ev.remote.decode("utf-8", "replace"),
            DisconnectReason(ev.reason),
        )

    @property
    def state(self):
        """The current LinkState."""
        return LinkState(_check(_ffi.lib.coppa_link_state(self._handle())))

    @property
    def pending_bytes(self):
        """Bytes queued or in flight and not yet acknowledged."""
        out = ctypes.c_size_t(0)
        _check(_ffi.lib.coppa_link_pending(self._handle(), ctypes.byref(out)))
        return out.value

    def close(self):
        """Destroy the link. Call only after other threads are done with it."""
        if self._h:
            _ffi.lib.coppa_link_destroy(ctypes.byref(self._h))
            self._h = ctypes.c_void_p()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
        return False

    def __del__(self):
        try:
            self.close()
        except Exception:  # noqa: BLE001 - never raise from a finalizer
            pass
