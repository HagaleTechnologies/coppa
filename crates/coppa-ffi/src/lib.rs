//! C FFI bindings for the Coppa engine.
//!
//! Provides a C-compatible API so that Coppa can be used from C, Python,
//! Swift, and other languages via shared library.
//!
//! # Safety
//!
//! All `extern "C"` functions in this module use raw pointers and are inherently
//! unsafe from the caller's perspective. They validate inputs before use.
//!
//! # Error codes
//!
//! | Code | Meaning |
//! |------|---------|
//! | `0`  | Success |
//! | `-1` | Null argument |
//! | `-2` | Invalid UTF-8 |
//! | `-3` | Encode/decode failed |
//! | `-4` | Lock poisoned — **handle is permanently broken** (see below) |
//! | `-5` | Internal panic — **handle is permanently broken** (see below) |
//! | `-6` | Invalid parameter value (an out-of-range or band-unavailable speed level passed to `coppa_engine_set_speed_level`; `coppa_engine_new_with` instead returns null for a rejected config, including an unrecognized profile name) — added in FFI v2, see below |
//! | `-7` | Call not valid in the link's current state (`coppa_link_*`, COP-139) |
//!
//! Each code has a named constant ([`COPPA_OK`], [`COPPA_ERR_NULL_ARG`] through
//! [`COPPA_ERR_INVALID_STATE`], and [`COPPA_NONE_PENDING`] for `1`).
//!
//! # Error messages (COP-139)
//!
//! Every fallible function clears the calling thread's error slot on entry
//! and, on each failure path (a negative code, or null from a constructor),
//! stores `"<function>: <reason>"` there. [`coppa_last_error`] returns it, or
//! null when the thread's last call succeeded, so a null return with no
//! message means "nothing" (for example [`coppa_get_decoded`] on an empty
//! stream). [`coppa_strerror`] gives a fixed description of any code.
//!
//! # Thread safety
//!
//! - Any function may be called from any thread. The library starts no
//!   threads and never calls back into your code.
//! - Calls on different handles are independent and may run at the same time.
//! - Calls on one `coppa_engine_t` or `coppa_link_t` from several threads are
//!   serialized by a lock inside the handle. They are memory-safe, but their
//!   order is whatever the threads produce. Feeding one audio stream to one
//!   handle from two threads interleaves blocks and corrupts the stream: use
//!   one receiving handle per audio input, fed by one thread.
//! - `coppa_engine_destroy`, `coppa_link_destroy` and `coppa_stop_stream` must
//!   not run while any other call on the same handle is in progress, and no
//!   call may use the handle after it. Join or stop the other threads first.
//! - `coppa_last_error` is per thread: it reports the calling thread's last
//!   failed call only.
//! - `coppa_version`, `coppa_abi_version` and `coppa_strerror` return static
//!   strings or values and are always safe to call.
//!
//! # ABI version
//!
//! [`coppa_abi_version`] returns `(MAJOR << 16) | MINOR` for the loaded
//! library ([`COPPA_ABI_VERSION_MAJOR`], [`COPPA_ABI_VERSION_MINOR`]). The
//! major changes on an incompatible change or removal, the minor on an
//! addition. It is separate from the crate version [`coppa_version`] returns.
//!
//! [`coppa_next_frame`] is a deliberate, documented exception to the `0` =
//! success convention above: it additionally uses `1` for the non-error
//! "no frame pending" case. See its own doc comment for why.
//!
//! # Handle lifetime and recovery
//!
//! After a `-4` (lock poisoned) or `-5` (internal panic) return, the affected
//! handle's internal `Mutex` is poisoned. **No further operations on that
//! handle will succeed.** The caller must destroy the handle and create a new
//! one. Attempting to reuse a poisoned handle will continue to return `-4`.
//!
//! # Buffer ownership
//!
//! `coppa_encode` / `coppa_encode_bytes` return a sample buffer via
//! `out_samples` / `out_len`. The caller **must** preserve the exact
//! `out_len` value and pass it back to [`coppa_free_samples`] — the
//! allocator needs the original length to reconstruct the `Box<[f32]>`.
//! Passing a different length is undefined behavior.
//!
//! `coppa_next_frame` returns an owned payload buffer via the `payload` /
//! `payload_len` fields of its `coppa_frame_t` out-parameter. The caller
//! **must** preserve the exact `payload_len` value and pass it back to
//! [`coppa_free_frame_payload`], for the same reason as above.
//!
//! # FFI v2 (Phase 4 Task 6): binary/config/event API
//!
//! [`coppa_engine_new_with`], [`coppa_engine_set_speed_level`],
//! [`coppa_encode_bytes`], [`coppa_engine_feed_samples`], and
//! [`coppa_next_frame`] are a newer, additive API surface built on top of
//! the same [`CoppaHandle`] (`coppa_engine_t` in the generated header) that
//! [`coppa_engine_create`]/[`coppa_encode`]/[`coppa_decode`] already use —
//! v1 and v2 functions can be freely mixed on the same handle. v2 adds:
//! raw-bytes TX/RX (no UTF-8 requirement, unlike `coppa_encode`/
//! `coppa_decode`/the old streaming quartet below), explicit construction
//! from a [`CoppaConfig`] (`coppa_config_t`) instead of only
//! `EngineConfig::default()`, and an event-style streaming-RX API
//! (`coppa_engine_feed_samples` pushes, `coppa_next_frame` pops one queued
//! frame at a time) instead of the old quartet's internal text-message
//! queue.
//!
//! The old text-only streaming quartet ([`coppa_start_stream`],
//! [`coppa_feed_samples`], [`coppa_get_decoded`], [`coppa_stop_stream`]) is
//! kept, unchanged, as a deprecated wrapper API for one release — see each
//! function's own doc comment. Since COP-139 each carries `#[deprecated]`,
//! which the header turns into a compiler-visible `COPPA_DEPRECATED(...)`.
//!
//! # FFI v3 (COP-139): links
//!
//! [`coppa_link_new`] and the other `coppa_link_*` functions expose
//! `coppa_engine::link::Link`: one connected, reliable session (coppad's
//! CONNECT handshake, stop-and-wait ARQ, keepalive and disconnect) over audio
//! samples. Rules:
//!
//! - **Time.** Every call that takes `now_ms` wants a monotonic millisecond
//!   count from any epoch. It must never decrease for one link; a smaller
//!   value returns `-6`. Pass a real monotonic clock, or a counter advanced by
//!   the block length for simulation.
//! - **Audio.** [`coppa_link_process`] is an audio callback: pass the captured
//!   48 kHz samples, get the samples to play. The signal is a prefix of `tx`
//!   of length `*tx_active`, the rest is zero; key the transmitter while
//!   `tx_active > 0`. While transmitting the link ignores `rx` (half duplex).
//! - **Threads.** Feed one link's audio from one thread. Other threads may
//!   call `coppa_link_send`, `coppa_link_recv` and the queries at any time.
//! - **Compatibility.** Both ends must use the same profile (OFDM band and
//!   compression).

#![warn(missing_docs, missing_debug_implementations)]

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

// --- Error codes (COP-139): the values in the module doc's table ---

/// Success.
pub const COPPA_OK: i32 = 0;
/// A required pointer argument was null.
pub const COPPA_ERR_NULL_ARG: i32 = -1;
/// A string argument was not valid UTF-8.
pub const COPPA_ERR_INVALID_UTF8: i32 = -2;
/// Encode or decode failed.
pub const COPPA_ERR_CODEC: i32 = -3;
/// The handle's lock is poisoned: destroy the handle and create a new one.
pub const COPPA_ERR_POISONED: i32 = -4;
/// An internal panic was caught: destroy the handle and create a new one.
pub const COPPA_ERR_PANIC: i32 = -5;
/// An argument's value is out of range or not supported.
pub const COPPA_ERR_INVALID_PARAM: i32 = -6;
/// The call is not valid in the link's current state (COP-139).
pub const COPPA_ERR_INVALID_STATE: i32 = -7;
/// Not an error: nothing is pending (`coppa_next_frame`, `coppa_link_next_event`).
pub const COPPA_NONE_PENDING: i32 = 1;

/// ABI major version: changes when an exported function, struct layout or
/// constant changes incompatibly or is removed (COP-139).
pub const COPPA_ABI_VERSION_MAJOR: u32 = 1;
/// ABI minor version: changes when something is added (COP-139).
pub const COPPA_ABI_VERSION_MINOR: u32 = 0;

const POISONED_REASON: &str =
    "handle is unusable: an earlier internal panic poisoned its lock; destroy it and create a new one";

thread_local! {
    /// The calling thread's message for its most recent failed `coppa_*` call.
    static LAST_ERROR: std::cell::RefCell<Option<CString>> = const { std::cell::RefCell::new(None) };
}

/// Clear the calling thread's last error (on entry to every fallible function).
fn clear_error() {
    LAST_ERROR.with(|e| *e.borrow_mut() = None);
}

/// Set the calling thread's last error to `"<func>: <reason>"`.
fn set_error(func: &str, reason: impl std::fmt::Display) {
    // Interior NULs cannot cross a C string; replace them.
    let text = format!("{func}: {reason}").replace('\0', "\u{FFFD}");
    LAST_ERROR.with(|e| *e.borrow_mut() = CString::new(text).ok());
}

/// Set `"<func>: <param> is null"` and return [`COPPA_ERR_NULL_ARG`].
fn null_arg(func: &str, param: &str) -> i32 {
    set_error(func, format_args!("{param} is null"));
    COPPA_ERR_NULL_ARG
}

/// The text of a caught panic's payload.
fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}

fn set_panic_error(func: &str, p: &(dyn std::any::Any + Send)) {
    set_error(
        func,
        format_args!(
            "internal panic: {}; the handle is unusable, destroy it and create a new one",
            panic_message(p)
        ),
    );
}

/// Map a `catch_unwind` result to a return code, recording a panic.
fn finish(func: &str, r: std::thread::Result<i32>) -> i32 {
    match r {
        Ok(code) => code,
        Err(p) => {
            set_panic_error(func, &*p);
            COPPA_ERR_PANIC
        }
    }
}

/// Map a `catch_unwind` result to a pointer, recording a panic as null.
fn finish_ptr<T>(func: &str, r: std::thread::Result<*mut T>) -> *mut T {
    match r {
        Ok(ptr) => ptr,
        Err(p) => {
            set_panic_error(func, &*p);
            std::ptr::null_mut()
        }
    }
}

/// Lock `m`, or record the poisoned-lock message and return
/// [`COPPA_ERR_POISONED`].
fn lock_or_poisoned<'a, T>(m: &'a Mutex<T>, func: &str) -> Result<MutexGuard<'a, T>, i32> {
    m.lock().map_err(|_| {
        set_error(func, POISONED_REASON);
        COPPA_ERR_POISONED
    })
}

/// The C ABI version of the loaded library, `(MAJOR << 16) | MINOR`.
///
/// Compare it with `COPPA_ABI_VERSION_MAJOR`/`COPPA_ABI_VERSION_MINOR` from
/// the header you compiled against: the majors must be equal and the
/// library's minor equal or higher. Separate from [`coppa_version`]'s crate
/// version. Always safe to call; never touches [`coppa_last_error`].
#[no_mangle]
pub extern "C" fn coppa_abi_version() -> u32 {
    (COPPA_ABI_VERSION_MAJOR << 16) | COPPA_ABI_VERSION_MINOR
}

/// The calling thread's message for its most recent failed `coppa_*` call,
/// as `"<function>: <reason>"`, or null when that call succeeded.
///
/// Every fallible function clears the slot on entry, so a null return value
/// with a null `coppa_last_error()` means "nothing", not a failure. The
/// string stays valid until this thread's next `coppa_*` call. Do not free
/// it. The slot is per thread.
#[no_mangle]
pub extern "C" fn coppa_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ref().map_or(std::ptr::null(), |s| s.as_ptr()))
}

/// A fixed description of an error code. Never null. Do not free it.
#[no_mangle]
pub extern "C" fn coppa_strerror(code: i32) -> *const c_char {
    let s: &'static str = match code {
        COPPA_OK => "success\0",
        COPPA_NONE_PENDING => "nothing pending (not an error)\0",
        COPPA_ERR_NULL_ARG => "a required pointer argument was null\0",
        COPPA_ERR_INVALID_UTF8 => "a string argument was not valid UTF-8\0",
        COPPA_ERR_CODEC => "encode or decode failed\0",
        COPPA_ERR_POISONED => "handle is unusable: an earlier internal panic poisoned its lock; destroy it and create a new one\0",
        COPPA_ERR_PANIC => "internal panic; the handle is unusable; destroy it and create a new one\0",
        COPPA_ERR_INVALID_PARAM => "invalid parameter value\0",
        COPPA_ERR_INVALID_STATE => "call not valid in the link's current state\0",
        _ => "unknown coppa error code\0",
    };
    s.as_ptr() as *const c_char
}

/// Opaque handle to a Coppa engine instance (`coppa_engine_t` in the
/// generated C header — see `cbindgen.toml`'s `[export.rename]`).
///
/// Shared by both the v1 text/batch API ([`coppa_engine_create`],
/// [`coppa_encode`], [`coppa_decode`]) and the v2 binary/config/event API
/// (Phase 4 Task 6: [`coppa_engine_new_with`], [`coppa_encode_bytes`],
/// [`coppa_engine_feed_samples`], [`coppa_next_frame`]). `frame_queue` and
/// `next_seq` exist only for the v2 event-style streaming API — v1
/// functions never touch them, so extending this struct changes no v1
/// signature or behavior.
///
/// # Thread safety
///
/// Calls on one handle from several threads are serialized by the mutexes
/// inside it and are memory-safe; their order is whatever the threads
/// produce. [`coppa_engine_destroy`] must not run while any other call on the
/// handle is in progress. See the crate-level "Thread safety" section.
#[derive(Debug)]
pub struct CoppaHandle {
    engine: Mutex<coppa_engine::CoppaCore>,
    /// Frames completed by [`coppa_engine_feed_samples`], queued (FIFO) for
    /// [`coppa_next_frame`] to pop.
    frame_queue: Mutex<VecDeque<QueuedFrame>>,
    /// Monotonic counter assigning each successfully-queued frame's `seq`
    /// (see [`QueuedFrame`]'s doc for why this handle maintains its own
    /// counter rather than reading one off `coppa_engine::StreamFrame`).
    next_seq: Mutex<u64>,
}

/// One frame queued by [`coppa_engine_feed_samples`] for [`coppa_next_frame`]
/// to pop, built from a `coppa_engine::StreamFrame` whose `payload` decoded
/// successfully (`Ok`).
///
/// `coppa_engine::StreamFrame` has no sequence-number field of its own (see
/// its doc comment in `coppa-engine`) — a per-`CoppaHandle` monotonic counter
/// is a reasonable, simple stand-in: `seq` here means "the Nth frame this
/// handle has successfully queued", assigned once per call, starting at `0`,
/// incremented only for frames that actually get queued (a frame whose
/// payload failed decompression is dropped without consuming a `seq` value —
/// see [`coppa_engine_feed_samples`]'s doc).
#[derive(Debug)]
struct QueuedFrame {
    payload: Vec<u8>,
    snr_db: f32,
    cfo_hz: f32,
    speed_level: u8,
    seq: u64,
}

/// Opaque handle to a streaming decode session.
///
/// `coppa_feed_samples` pushes samples directly into the engine's internal
/// `StreamingReceiver` (via `CoppaCore::push_samples`), which owns its own
/// ring/sync-detector/frame-boundary bookkeeping — no buffer growth cap or
/// rescanning is needed here, since the receiver never re-examines samples it's
/// already consumed and bounds its own memory (`2 * max_frame_samples`; see
/// `coppa_protocol::modem::streaming`).
///
/// **Deprecated since FFI v2 (Phase 4 Task 6):** kept, unchanged, as a
/// deprecated wrapper for one release — see [`coppa_start_stream`]'s doc.
/// New code should use [`CoppaHandle`] (`coppa_engine_t`) with
/// [`coppa_engine_feed_samples`]/[`coppa_next_frame`] instead.
#[derive(Debug)]
pub struct CoppaStreamHandle {
    engine: Mutex<coppa_engine::CoppaCore>,
    decoded_messages: Mutex<VecDeque<String>>,
}

/// Create a new Coppa engine instance.
///
/// Returns a pointer to the engine handle, or null on failure.
/// The caller must free the handle with [`coppa_engine_destroy`].
#[no_mangle]
pub extern "C" fn coppa_engine_create() -> *mut CoppaHandle {
    clear_error();
    let result = std::panic::catch_unwind(|| {
        let handle = Box::new(CoppaHandle {
            engine: Mutex::new(coppa_engine::CoppaCore::new()),
            frame_queue: Mutex::new(VecDeque::new()),
            next_seq: Mutex::new(0),
        });
        Box::into_raw(handle)
    });
    finish_ptr("coppa_engine_create", result)
}

/// Destroy a Coppa engine instance and free its resources.
///
/// After this call, `*handle_ptr` is set to null. Calling with a null
/// `handle_ptr` or a `*handle_ptr` that is already null is a safe no-op.
///
/// # Safety
///
/// `handle_ptr` must be either null or a valid pointer to a variable that
/// holds a pointer returned by [`coppa_engine_create`].
#[no_mangle]
pub unsafe extern "C" fn coppa_engine_destroy(handle_ptr: *mut *mut CoppaHandle) {
    if handle_ptr.is_null() {
        return;
    }
    let handle = *handle_ptr;
    if !handle.is_null() {
        drop(Box::from_raw(handle));
        *handle_ptr = std::ptr::null_mut();
    }
}

/// Get the library version string.
///
/// Returns a pointer to a static null-terminated string. The caller must
/// not free or modify the returned pointer.
#[no_mangle]
pub extern "C" fn coppa_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// Encode a message string to audio samples.
///
/// On success, `out_samples` and `out_len` are populated with the sample
/// buffer and its length. The sample buffer must be freed with
/// [`coppa_free_samples`] using the exact length returned in `out_len`.
///
/// Returns `-4` if the internal lock is poisoned, or `-5` if an internal
/// panic occurred. **After either of these errors the handle is permanently
/// broken** — destroy it with `coppa_engine_destroy` and create a new one.
///
/// # Safety
///
/// `handle` must be a valid engine handle. `message` must be a valid
/// null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn coppa_encode(
    handle: *mut CoppaHandle,
    message: *const c_char,
    out_samples: *mut *mut f32,
    out_len: *mut usize,
) -> i32 {
    const F: &str = "coppa_encode";
    clear_error();
    if handle.is_null() {
        return null_arg(F, "handle");
    }
    if message.is_null() {
        return null_arg(F, "message");
    }
    if out_samples.is_null() {
        return null_arg(F, "out_samples");
    }
    if out_len.is_null() {
        return null_arg(F, "out_len");
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let msg = match CStr::from_ptr(message).to_str() {
            Ok(s) => s,
            Err(_) => {
                set_error(F, "message is not valid UTF-8");
                return COPPA_ERR_INVALID_UTF8;
            }
        };

        let engine = match lock_or_poisoned(&(*handle).engine, F) {
            Ok(e) => e,
            Err(code) => return code,
        };

        match engine.encode(msg) {
            Ok(samples) => {
                let boxed = samples.into_boxed_slice();
                let len = boxed.len();
                let ptr = Box::into_raw(boxed) as *mut f32;
                *out_len = len;
                *out_samples = ptr;
                COPPA_OK
            }
            Err(e) => {
                set_error(F, e);
                COPPA_ERR_CODEC
            }
        }
    }));
    finish(F, result)
}

/// Decode audio samples back to a message string.
///
/// On success, `out_message` is populated with a pointer to a
/// null-terminated string that must be freed with [`coppa_free_string`].
///
/// Returns `-4` if the internal lock is poisoned, or `-5` if an internal
/// panic occurred. **After either of these errors the handle is permanently
/// broken** — destroy it with `coppa_engine_destroy` and create a new one.
///
/// # Safety
///
/// `handle` must be a valid engine handle. `samples` must point to
/// `num_samples` valid f32 values.
#[no_mangle]
pub unsafe extern "C" fn coppa_decode(
    handle: *mut CoppaHandle,
    samples: *const f32,
    num_samples: usize,
    out_message: *mut *mut c_char,
) -> i32 {
    const F: &str = "coppa_decode";
    clear_error();
    if handle.is_null() {
        return null_arg(F, "handle");
    }
    if samples.is_null() {
        return null_arg(F, "samples");
    }
    if out_message.is_null() {
        return null_arg(F, "out_message");
    }
    if num_samples == 0 {
        set_error(F, "num_samples is 0");
        return COPPA_ERR_NULL_ARG;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sample_slice = std::slice::from_raw_parts(samples, num_samples);
        let engine = match lock_or_poisoned(&(*handle).engine, F) {
            Ok(e) => e,
            Err(code) => return code,
        };

        match engine.decode(sample_slice) {
            Ok(message) => {
                // Use CString so coppa_free_string can use CString::from_raw.
                match CString::new(message) {
                    Ok(cstr) => {
                        *out_message = cstr.into_raw();
                        COPPA_OK
                    }
                    // Message contained an interior NUL — treat as encode error.
                    Err(_) => {
                        set_error(F, "decoded message contains a NUL byte; use coppa_engine_feed_samples and coppa_next_frame for binary payloads");
                        COPPA_ERR_CODEC
                    }
                }
            }
            Err(e) => {
                set_error(F, e);
                COPPA_ERR_CODEC
            }
        }
    }));
    finish(F, result)
}

/// Free a sample buffer returned by `coppa_encode`.
///
/// `len` **must** be the exact value that was written to `out_len` by
/// `coppa_encode`. Passing a different length is undefined behavior.
/// Zero-length buffers are valid and will be freed correctly.
///
/// # Safety
///
/// `samples` must be a pointer returned by `coppa_encode`, and `len` must
/// be the exact length returned alongside it in `out_len`.
#[no_mangle]
pub unsafe extern "C" fn coppa_free_samples(samples: *mut f32, len: usize) {
    // C1: removed `len > 0` guard — zero-length boxed slices are valid allocations.
    if !samples.is_null() {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            samples, len,
        )));
    }
}

/// Free a string returned by `coppa_decode` or `coppa_get_decoded`.
///
/// Uses `CString::from_raw` internally — no `strlen` rescan needed because
/// the `CString` already knows its own length from the original allocation.
///
/// # Safety
///
/// `s` must be a pointer returned by `coppa_decode` or `coppa_get_decoded`,
/// and must not have been freed already.
#[no_mangle]
pub unsafe extern "C" fn coppa_free_string(s: *mut c_char) {
    // C2: switched from CStr rescan + Box::from_raw to CString::from_raw.
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

// --- v2 API (Phase 4 Task 6): binary/config/event, over CoppaHandle ---

/// C-compatible engine configuration for [`coppa_engine_new_with`]
/// (`coppa_config_t` in the generated header).
///
/// # Field semantics
///
/// - `sample_rate`: sample rate in Hz. `0` means "use the resolved
///   profile's/default sample rate" (every shipped profile is 48000 Hz
///   today — see `coppa_engine::profiles`). `0` is never itself a valid
///   sample rate, so it unambiguously means "unset" here.
/// - `profile`: optional pointer to a null-terminated profile name (e.g.
///   `"HF_ROBUST"`), resolved case-insensitively via
///   `coppa_engine::profiles::get_profile` — the same resolution
///   `coppa-cli`'s own `resolve_config` uses. Pass null or an empty string
///   to start from `EngineConfig::default()` instead. An unrecognized
///   non-empty profile name is a config-rejection error (see
///   [`coppa_engine_new_with`]'s return value).
/// - `speed_level`: wire speed-level override (1-9), applied after `profile`
///   resolution. `0` means "use the resolved profile's/default speed_level".
///   A non-zero level outside 1-9, or level 9 on a handle whose profile is
///   HF (HF carries levels 1-8; level 9 is VHF-only), is a config-rejection
///   error. The override changes the level, never the profile's OFDM band:
///   an `"HF_ROBUST"` handle at `speed_level = 8` stays on an HF profile, and
///   so does any later [`coppa_engine_set_speed_level`] call on it (COP-23).
///   Only a handle built with no `profile` keeps the historical level-only
///   rule (HF below level 5, VHF from level 5).
/// - `callsign`: optional pointer to a null-terminated station-callsign
///   string, for the FFI caller's own bookkeeping convenience only —
///   `CoppaCore`/`EngineConfig` have no callsign concept at all (callsign
///   is a daemon-level station-ID/beacon concept layered above the engine,
///   added in Phase 4 Task 3; nothing in `coppa-engine` consumes it). This
///   field is validated (must be valid UTF-8 if non-null, else a
///   config-rejection error) and then **discarded** — it is not retained
///   on the returned handle and has no accessor. Storing it with no reader
///   anywhere would be dead weight the "don't overbuild" discipline this
///   codebase follows argues against; a future task adding a real
///   config-readback API (e.g. `coppa_engine_get_config`) should add
///   storage and an accessor together. Pass null if unused.
#[repr(C)]
#[derive(Debug)]
pub struct CoppaConfig {
    /// Sample rate in Hz; `0` keeps the resolved profile's rate (48000).
    /// Any value other than `0` or `48000` is rejected (COP-139).
    pub sample_rate: u32,
    /// Null-terminated profile name, matched case-insensitively; null or empty
    /// starts from `EngineConfig::default()`.
    pub profile: *const c_char,
    /// Speed-level override 1-9 (9 is VHF-only); `0` keeps the profile's level.
    pub speed_level: u8,
    /// Null-terminated callsign; validated as UTF-8, then discarded. May be null.
    pub callsign: *const c_char,
}

/// One frame popped by [`coppa_next_frame`] (`coppa_frame_t` in the
/// generated header).
///
/// `payload`/`payload_len` are an owned heap buffer allocated by this crate
/// and must be freed with [`coppa_free_frame_payload`] using the exact
/// `payload_len` value — same length-must-match-exactly convention as
/// [`coppa_free_samples`]. When [`coppa_next_frame`] returns `1` ("no frame
/// pending"), `*out` is zeroed (`payload = null`, `payload_len = 0`, all
/// other fields `0`) — freeing a null `payload` is a safe no-op, same as
/// `coppa_free_samples`.
///
/// `seq` is a per-handle monotonic counter this crate assigns (see
/// [`QueuedFrame`]'s doc) — `coppa_engine::StreamFrame` itself has no
/// sequence-number field.
#[repr(C)]
#[derive(Debug)]
pub struct CoppaFrame {
    /// Owned payload buffer; free it with `coppa_free_frame_payload`. Null when
    /// no frame was pending.
    pub payload: *mut u8,
    /// Length of `payload` in bytes; pass this exact value when freeing it.
    pub payload_len: usize,
    /// Per-carrier-noise SNR estimate of the frame, in dB.
    pub snr_db: f32,
    /// Carrier frequency offset the sync detector estimated, in Hz.
    pub cfo_hz: f32,
    /// Speed level (1-9) the sender encoded the frame at.
    pub speed_level: u8,
    /// Per-handle frame counter: 0 for the first frame this handle queued,
    /// incremented once per queued frame.
    pub seq: u64,
}

/// Resolve a [`CoppaConfig`] into an [`coppa_engine::EngineConfig`], or
/// `None` for any config-rejection case (invalid UTF-8 in `profile` or
/// `callsign`, an unrecognized non-empty `profile` name, or a non-zero
/// `speed_level` that isn't valid — see [`coppa_engine_new_with`]'s doc).
///
/// # Safety
///
/// `cfg` must be a valid, non-null pointer to a fully-initialized
/// `CoppaConfig`.
/// The only audio sample rate coppa runs at.
const SUPPORTED_SAMPLE_RATE: u32 = 48_000;

/// Resolve a C profile name (null or empty = `EngineConfig::default()`) to an
/// `EngineConfig`, or the reason it was rejected. Shared by
/// [`coppa_engine_new_with`] and [`coppa_link_new`].
unsafe fn resolve_profile(profile: *const c_char) -> Result<coppa_engine::EngineConfig, String> {
    if profile.is_null() {
        return Ok(coppa_engine::EngineConfig::default());
    }
    let name = CStr::from_ptr(profile)
        .to_str()
        .map_err(|_| "profile is not valid UTF-8".to_owned())?;
    if name.is_empty() {
        return Ok(coppa_engine::EngineConfig::default());
    }
    match coppa_engine::profiles::get_profile(name) {
        Some(p) => Ok(coppa_engine::EngineConfig::from_profile(p)),
        None => {
            let names = [
                coppa_engine::HF_ROBUST.name,
                coppa_engine::HF_STANDARD.name,
                coppa_engine::VHF_FAST.name,
                coppa_engine::EMERGENCY.name,
            ];
            let (last, rest) = names.split_last().expect("four profiles");
            Err(format!(
                "unknown profile \"{name}\" (expected {} or {last})",
                rest.join(", ")
            ))
        }
    }
}

unsafe fn resolve_engine_config(
    cfg: *const CoppaConfig,
) -> Result<coppa_engine::EngineConfig, String> {
    let cfg = &*cfg;

    let mut config = resolve_profile(cfg.profile)?;

    match cfg.sample_rate {
        0 => {}
        SUPPORTED_SAMPLE_RATE => config.sample_rate = cfg.sample_rate,
        other => {
            return Err(format!(
                "sample_rate {other} is not supported: coppa runs at 48000 Hz (pass 0 or 48000)"
            ))
        }
    }

    if !cfg.callsign.is_null() {
        // Validated for UTF-8 (fail fast on bad input, matching this file's
        // existing convention for every other string parameter) but
        // intentionally not retained — see `CoppaConfig::callsign`'s doc.
        CStr::from_ptr(cfg.callsign)
            .to_str()
            .map_err(|_| "callsign is not valid UTF-8".to_owned())?;
    }

    Ok(config)
}

/// Construct a new engine handle from a [`CoppaConfig`] (`coppa_config_t`).
///
/// Returns null if `cfg` is null, or for any config-rejection case (see
/// [`resolve_engine_config`] and [`CoppaConfig`]'s field docs): invalid
/// UTF-8 in `profile` or `callsign`, an unrecognized non-empty `profile`
/// name, a `sample_rate` other than `0` or `48000` (COP-139: coppa runs at
/// 48 kHz only, so any other rate is refused instead of silently ignored),
/// or a non-zero `speed_level` that isn't a valid wire speed level
/// (1-9), or level 9 on an HF-profile handle. After a null return,
/// [`coppa_last_error`] says which.
///
/// The returned handle is a plain [`CoppaHandle`] (`coppa_engine_t`) —
/// exactly what [`coppa_engine_create`] returns, just built from an
/// explicit config instead of `EngineConfig::default()`. It must be freed
/// with [`coppa_engine_destroy`], and every v1 function
/// (`coppa_encode`/`coppa_decode`/etc.) works on it too.
///
/// # Safety
///
/// `cfg` must be null or point to a valid, fully-initialized `CoppaConfig`.
/// `cfg->profile` and `cfg->callsign`, if non-null, must be valid
/// null-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn coppa_engine_new_with(cfg: *const CoppaConfig) -> *mut CoppaHandle {
    const F: &str = "coppa_engine_new_with";
    clear_error();
    if cfg.is_null() {
        set_error(F, "cfg is null");
        return std::ptr::null_mut();
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let config = match resolve_engine_config(cfg) {
            Ok(c) => c,
            Err(reason) => {
                set_error(F, reason);
                return std::ptr::null_mut();
            }
        };
        let mut core = coppa_engine::CoppaCore::with_config(config);
        if (*cfg).speed_level != 0 {
            if let Err(e) = core.set_speed_level((*cfg).speed_level) {
                set_error(F, e);
                return std::ptr::null_mut();
            }
        }
        let handle = Box::new(CoppaHandle {
            engine: Mutex::new(core),
            frame_queue: Mutex::new(VecDeque::new()),
            next_seq: Mutex::new(0),
        });
        Box::into_raw(handle)
    }));
    finish_ptr(F, result)
}

/// Change an existing engine handle's configured speed level.
///
/// Rebuilds the engine's transceiver/streaming receiver for the new level
/// (see `CoppaCore::set_speed_level`) — any samples already buffered inside
/// the streaming receiver but not yet resolved into a completed frame are
/// discarded as a result. Frames already queued for [`coppa_next_frame`]
/// (fully decoded before this call) are **not** affected.
///
/// Returns `0` on success, `-1` if `handle` is null, `-6` if `level` is not
/// a valid wire speed level (1-9) or is level 9 on an HF-profile handle,
/// `-4` if the internal lock is poisoned, or `-5` if an internal panic occurred. **After either
/// of the latter two the handle is permanently broken** — destroy it with
/// `coppa_engine_destroy` and create a new one.
///
/// # Safety
///
/// `handle` must be a valid engine handle.
#[no_mangle]
pub unsafe extern "C" fn coppa_engine_set_speed_level(handle: *mut CoppaHandle, level: u8) -> i32 {
    const F: &str = "coppa_engine_set_speed_level";
    clear_error();
    if handle.is_null() {
        return null_arg(F, "handle");
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut engine = match lock_or_poisoned(&(*handle).engine, F) {
            Ok(e) => e,
            Err(code) => return code,
        };
        match engine.set_speed_level(level) {
            Ok(()) => COPPA_OK,
            Err(e) => {
                set_error(F, e);
                COPPA_ERR_INVALID_PARAM
            }
        }
    }));
    finish(F, result)
}

/// Encode raw binary data to audio samples — binary counterpart to
/// [`coppa_encode`] (which is text/UTF-8-only).
///
/// On success, `out_samples`/`out_len` are populated exactly like
/// `coppa_encode`'s: free with [`coppa_free_samples`] using the exact
/// `out_len` value.
///
/// Returns `-1` if `handle`, `out_samples`, or `out_len` is null, or if
/// `data` is null while `len > 0`. Returns `-3` if encoding failed (e.g.
/// payload too large for the configured speed level). Returns `-4` if the
/// internal lock is poisoned, or `-5` if an internal panic occurred.
/// **After either of the latter two the handle is permanently broken** —
/// destroy it with `coppa_engine_destroy` and create a new one.
///
/// # Safety
///
/// `handle` must be a valid engine handle. `data` must point to `len` valid
/// bytes (`data` may be null only if `len == 0`).
#[no_mangle]
pub unsafe extern "C" fn coppa_encode_bytes(
    handle: *mut CoppaHandle,
    data: *const u8,
    len: usize,
    out_samples: *mut *mut f32,
    out_len: *mut usize,
) -> i32 {
    const F: &str = "coppa_encode_bytes";
    clear_error();
    if handle.is_null() {
        return null_arg(F, "handle");
    }
    if data.is_null() && len > 0 {
        return null_arg(F, "data");
    }
    if out_samples.is_null() {
        return null_arg(F, "out_samples");
    }
    if out_len.is_null() {
        return null_arg(F, "out_len");
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let data_slice: &[u8] = if len == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(data, len)
        };

        let engine = match lock_or_poisoned(&(*handle).engine, F) {
            Ok(e) => e,
            Err(code) => return code,
        };

        match engine.encode_bytes(data_slice) {
            Ok(samples) => {
                let boxed = samples.into_boxed_slice();
                let blen = boxed.len();
                let ptr = Box::into_raw(boxed) as *mut f32;
                *out_len = blen;
                *out_samples = ptr;
                COPPA_OK
            }
            Err(e) => {
                set_error(F, e);
                COPPA_ERR_CODEC
            }
        }
    }));
    finish(F, result)
}

/// Push samples into an engine handle's internal streaming receiver —
/// binary counterpart to the deprecated [`coppa_feed_samples`] (which
/// requires a separate `CoppaStreamHandle` and only surfaces valid-UTF-8
/// text messages).
///
/// Samples are pushed directly into the engine's `StreamingReceiver`,
/// exactly like `coppa_feed_samples` — the caller can feed any chunk size
/// at any time; there is no minimum-samples threshold to reach before a
/// decode is attempted. Any frames the receiver completes as a result of
/// this call whose payload decompressed successfully are queued (raw
/// bytes, no UTF-8 forcing — see `coppa_engine::StreamFrame`'s doc) for
/// [`coppa_next_frame`] to pop; a frame whose payload failed decompression
/// is dropped (there is no raw byte payload to surface for it).
///
/// Returns `0` on success (including when zero frames complete or queue).
/// Returns `-1` if `handle` or `samples` is null (`len == 0` with non-null
/// pointers is a safe no-op returning `0`). Returns `-4` if the internal
/// lock is poisoned, or `-5` if an internal panic occurred. **After either
/// of the latter two the handle is permanently broken** — destroy it with
/// `coppa_engine_destroy` and create a new one.
///
/// # Safety
///
/// `handle` must be a valid engine handle. `samples` must point to `len`
/// valid f32 values.
#[no_mangle]
pub unsafe extern "C" fn coppa_engine_feed_samples(
    handle: *mut CoppaHandle,
    samples: *const f32,
    len: usize,
) -> i32 {
    const F: &str = "coppa_engine_feed_samples";
    clear_error();
    if handle.is_null() {
        return null_arg(F, "handle");
    }
    if samples.is_null() {
        return null_arg(F, "samples");
    }
    if len == 0 {
        return COPPA_OK;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sample_slice = std::slice::from_raw_parts(samples, len);

        let mut engine = match lock_or_poisoned(&(*handle).engine, F) {
            Ok(e) => e,
            Err(code) => return code,
        };
        let frames = engine.push_samples(sample_slice);
        drop(engine);

        if !frames.is_empty() {
            let mut queue = match lock_or_poisoned(&(*handle).frame_queue, F) {
                Ok(q) => q,
                Err(code) => return code,
            };
            let mut next_seq = match lock_or_poisoned(&(*handle).next_seq, F) {
                Ok(s) => s,
                Err(code) => return code,
            };
            for frame in frames {
                if let Ok(payload) = frame.payload {
                    let seq = *next_seq;
                    *next_seq += 1;
                    queue.push_back(QueuedFrame {
                        payload,
                        snr_db: frame.snr_db,
                        cfo_hz: frame.cfo_hz,
                        speed_level: frame.speed_level,
                        seq,
                    });
                }
            }
        }

        COPPA_OK
    }));
    finish(F, result)
}

/// Pop one queued frame from an engine handle into `*out`.
///
/// # Return-code convention
///
/// This module's error-code table establishes `0` = success everywhere
/// else in this file. This function's own originating design note said
/// "0 = none pending" — read literally, that would contradict the
/// module-wide convention for this one function (a caller checking `if
/// (coppa_next_frame(h, &f) == 0)` to mean "success" elsewhere in this API
/// would silently misread an empty queue as a popped frame). Resolved as
/// follows, to stay internally consistent with the rest of this file:
///
/// - `0`: a frame was popped; `*out` is populated and its `payload` must
///   be freed with [`coppa_free_frame_payload`].
/// - `1`: no frame is currently pending — **not an error**; `*out` is
///   zeroed (see [`CoppaFrame`]'s doc).
/// - `-1`: `handle` or `out` is null.
/// - `-4`: internal lock poisoned. `-5`: internal panic. Both leave the
///   handle permanently broken, same as every other function here.
///
/// # Safety
///
/// `handle` must be a valid engine handle. `out` must point to valid,
/// writable [`CoppaFrame`] storage.
#[no_mangle]
pub unsafe extern "C" fn coppa_next_frame(handle: *mut CoppaHandle, out: *mut CoppaFrame) -> i32 {
    const F: &str = "coppa_next_frame";
    clear_error();
    if handle.is_null() {
        return null_arg(F, "handle");
    }
    if out.is_null() {
        return null_arg(F, "out");
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut queue = match lock_or_poisoned(&(*handle).frame_queue, F) {
            Ok(q) => q,
            Err(code) => return code,
        };
        match queue.pop_front() {
            Some(frame) => {
                let boxed = frame.payload.into_boxed_slice();
                let payload_len = boxed.len();
                let payload = Box::into_raw(boxed) as *mut u8;
                *out = CoppaFrame {
                    payload,
                    payload_len,
                    snr_db: frame.snr_db,
                    cfo_hz: frame.cfo_hz,
                    speed_level: frame.speed_level,
                    seq: frame.seq,
                };
                COPPA_OK
            }
            None => {
                *out = CoppaFrame {
                    payload: std::ptr::null_mut(),
                    payload_len: 0,
                    snr_db: 0.0,
                    cfo_hz: 0.0,
                    speed_level: 0,
                    seq: 0,
                };
                COPPA_NONE_PENDING
            }
        }
    }));
    finish(F, result)
}

/// Free a frame payload buffer returned by [`coppa_next_frame`].
///
/// `len` **must** be the exact `payload_len` value written alongside
/// `payload` by `coppa_next_frame`. Passing a different length is undefined
/// behavior. A null `payload` (e.g. from a `1`/"no frame pending" result)
/// is a safe no-op, same as [`coppa_free_samples`].
///
/// # Safety
///
/// `payload` must be a pointer returned by `coppa_next_frame`, and `len`
/// must be the exact length returned alongside it in `payload_len`.
#[no_mangle]
pub unsafe extern "C" fn coppa_free_frame_payload(payload: *mut u8, len: usize) {
    if !payload.is_null() {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            payload, len,
        )));
    }
}

// --- Streaming API (v1, deprecated — see FFI v2 above) ---

/// Start a streaming decode session.
///
/// Returns a handle to the stream, or null on failure.
///
/// **Deprecated since FFI v2 (Phase 4 Task 6):** use [`coppa_engine_new_with`]
/// / [`coppa_engine_create`] plus [`coppa_engine_feed_samples`] and
/// [`coppa_next_frame`] instead — this quartet is kept, unchanged, as a
/// deprecated wrapper API for one release. Its text-only, UTF-8-forcing
/// contract silently drops real binary session/ARQ traffic that the v2 raw
/// byte API surfaces correctly (see [`coppa_feed_samples`]'s doc).
#[no_mangle]
#[deprecated(
    note = "use coppa_engine_new_with with coppa_engine_feed_samples and coppa_next_frame"
)]
pub extern "C" fn coppa_start_stream() -> *mut CoppaStreamHandle {
    clear_error();
    let result = std::panic::catch_unwind(|| {
        let handle = Box::new(CoppaStreamHandle {
            engine: Mutex::new(coppa_engine::CoppaCore::new()),
            decoded_messages: Mutex::new(VecDeque::new()),
        });
        Box::into_raw(handle)
    });
    finish_ptr("coppa_start_stream", result)
}

/// Feed samples into a streaming decode session.
///
/// Samples are pushed directly into the engine's `StreamingReceiver`, which
/// detects frame boundaries and demodulates each completed frame on its own —
/// the caller can feed any chunk size at any time; there is no minimum-samples
/// threshold to reach before a decode is attempted. Any frames the receiver
/// completes as a result of this call are queued for [`coppa_get_decoded`].
///
/// Returns `-4` if the internal lock is poisoned, or `-5` if an internal
/// panic occurred. **After either of these errors the stream handle is
/// permanently broken** — destroy it with `coppa_stop_stream` and create
/// a new one.
///
/// # Safety
///
/// `handle` must be a valid stream handle. `samples` must point to
/// `num_samples` valid f32 values.
///
/// **Deprecated since FFI v2 (Phase 4 Task 6):** use
/// [`coppa_engine_feed_samples`] instead — same underlying
/// `StreamingReceiver`, but surfaces raw binary payloads (no UTF-8
/// requirement) via [`coppa_next_frame`] rather than dropping every frame
/// whose payload isn't valid UTF-8 (see the note in this function's body).
#[no_mangle]
#[deprecated(note = "use coppa_engine_feed_samples")]
pub unsafe extern "C" fn coppa_feed_samples(
    handle: *mut CoppaStreamHandle,
    samples: *const f32,
    num_samples: usize,
) -> i32 {
    const F: &str = "coppa_feed_samples";
    clear_error();
    if handle.is_null() {
        return null_arg(F, "handle");
    }
    if samples.is_null() {
        return null_arg(F, "samples");
    }
    if num_samples == 0 {
        return COPPA_OK;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sample_slice = std::slice::from_raw_parts(samples, num_samples);

        let mut engine = match lock_or_poisoned(&(*handle).engine, F) {
            Ok(e) => e,
            Err(code) => return code,
        };

        let frames = engine.push_samples(sample_slice);
        if !frames.is_empty() {
            if let Ok(mut messages) = (*handle).decoded_messages.lock() {
                // This function's contract (FFI v1) is text messages only: a
                // frame whose raw (post-decompression) payload isn't valid
                // UTF-8 is dropped here, same net behavior as before Phase 4
                // Task 3.5 -- but now the UTF-8 attempt happens here, on the
                // engine's raw `payload: Result<Vec<u8>>`, rather than being
                // forced inside the engine itself (which used to silently
                // drop real binary MAC-PDU/ARQ frames before they ever
                // reached any consumer, FFI included). Frames that failed
                // decompression are still dropped here too, exactly like a
                // failed batch `decode()` was silently dropped before Task
                // 7's migration to `push_samples`. Raw binary frame access
                // for FFI callers is planned separately as Task 6 (FFI v2,
                // not implemented here).
                for frame in frames {
                    if let Ok(payload) = frame.payload {
                        if let Ok(message) = String::from_utf8(payload) {
                            messages.push_back(message);
                        }
                    }
                }
            }
        }

        COPPA_OK
    }));
    finish(F, result)
}

/// Get the next decoded message from a streaming session.
///
/// Returns a pointer to a null-terminated string, or null if no messages
/// are available. The string must be freed with `coppa_free_string`.
///
/// # Safety
///
/// `handle` must be a valid stream handle.
///
/// **Deprecated since FFI v2 (Phase 4 Task 6):** use [`coppa_next_frame`]
/// instead — pops a `coppa_frame_t` with the raw payload plus SNR/CFO/speed
/// level/seq metadata, instead of a bare text message.
#[no_mangle]
#[deprecated(note = "use coppa_next_frame")]
pub unsafe extern "C" fn coppa_get_decoded(handle: *mut CoppaStreamHandle) -> *mut c_char {
    const F: &str = "coppa_get_decoded";
    clear_error();
    if handle.is_null() {
        set_error(F, "handle is null");
        return std::ptr::null_mut();
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut messages = match lock_or_poisoned(&(*handle).decoded_messages, F) {
            Ok(m) => m,
            Err(_) => return std::ptr::null_mut(),
        };

        match messages.pop_front() {
            Some(message) => {
                // Use CString so coppa_free_string can use CString::from_raw.
                match CString::new(message) {
                    Ok(cstr) => cstr.into_raw(),
                    Err(_) => {
                        set_error(F, "decoded message contains a NUL byte; use coppa_engine_feed_samples and coppa_next_frame for binary payloads");
                        std::ptr::null_mut()
                    }
                }
            }
            None => std::ptr::null_mut(),
        }
    }));
    finish_ptr(F, result)
}

/// Stop and destroy a streaming decode session.
///
/// # Safety
///
/// `handle` must be a valid stream handle returned by `coppa_start_stream`.
///
/// **Deprecated since FFI v2 (Phase 4 Task 6):** use [`coppa_engine_destroy`]
/// instead, if this handle was created via [`coppa_engine_new_with`]/
/// [`coppa_engine_create`].
#[no_mangle]
#[deprecated(note = "use coppa_engine_destroy")]
pub unsafe extern "C" fn coppa_stop_stream(handle: *mut CoppaStreamHandle) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

// --- FFI v3 (COP-139): connected links over coppa_engine::link::Link ---

/// Link state: idle (`coppa_link_state`).
pub const COPPA_LINK_STATE_IDLE: i32 = 0;
/// Link state: a CONNECT handshake is in progress.
pub const COPPA_LINK_STATE_CONNECTING: i32 = 1;
/// Link state: connected; `coppa_link_send` is accepted.
pub const COPPA_LINK_STATE_CONNECTED: i32 = 2;
/// Link state: draining queued data before the DISCONNECT frame.
pub const COPPA_LINK_STATE_DISCONNECTING: i32 = 3;
/// Event kind: the link connected; `remote` names the peer.
pub const COPPA_LINK_EVENT_CONNECTED: i32 = 1;
/// Event kind: the link ended; `reason` says why.
pub const COPPA_LINK_EVENT_DISCONNECTED: i32 = 2;
/// Disconnect reason: none (the event is not a disconnect).
pub const COPPA_LINK_REASON_NONE: i32 = 0;
/// Disconnect reason: this side called `coppa_link_disconnect`.
pub const COPPA_LINK_REASON_LOCAL: i32 = 1;
/// Disconnect reason: the peer sent DISCONNECT.
pub const COPPA_LINK_REASON_REMOTE: i32 = 2;
/// Disconnect reason: the session timed out.
pub const COPPA_LINK_REASON_TIMEOUT: i32 = 3;
/// Disconnect reason: a data frame used up its retransmissions.
pub const COPPA_LINK_REASON_RETRIES_EXHAUSTED: i32 = 4;
/// Disconnect reason: the CONNECT handshake got no answer.
pub const COPPA_LINK_REASON_HANDSHAKE_FAILED: i32 = 5;

/// Opaque link handle (`coppa_link_t` in the generated header).
///
/// One connected, reliable session (coppad's session protocol) over audio
/// samples and a caller clock. Calls from several threads are serialized by
/// the lock inside; feed one audio stream from one thread.
#[derive(Debug)]
pub struct CoppaLink {
    inner: Mutex<LinkInner>,
}

#[derive(Debug)]
struct LinkInner {
    link: coppa_engine::link::Link,
    /// The `Instant` that `now_ms == 0` maps to, captured in `coppa_link_new`.
    base: Instant,
    last_now_ms: Option<u64>,
}

impl LinkInner {
    /// Map a caller `now_ms` to an `Instant`, rejecting a decrease.
    fn instant(&mut self, now_ms: u64) -> Result<Instant, (i32, String)> {
        if let Some(prev) = self.last_now_ms {
            if now_ms < prev {
                return Err((
                    COPPA_ERR_INVALID_PARAM,
                    format!(
                        "now_ms {now_ms} is earlier than the previous call's {prev}; pass a monotonic clock"
                    ),
                ));
            }
        }
        let t = self
            .base
            .checked_add(Duration::from_millis(now_ms))
            .ok_or_else(|| {
                (
                    COPPA_ERR_INVALID_PARAM,
                    format!("now_ms {now_ms} is too large"),
                )
            })?;
        self.last_now_ms = Some(now_ms);
        Ok(t)
    }
}

/// Settings for [`coppa_link_new`] (`coppa_link_config_t`).
#[repr(C)]
#[derive(Debug)]
pub struct CoppaLinkConfig {
    /// Set to `sizeof(coppa_link_config_t)`.
    pub struct_size: u32,
    /// Required: this station's callsign, 1-8 of A-Z 0-9 / -.
    pub callsign: *const c_char,
    /// Optional profile name, as `coppa_config_t.profile`; null = defaults.
    /// Both ends of a link must use the same profile.
    pub profile: *const c_char,
    /// Speed level; `0` keeps the profile's level.
    pub speed_level: u8,
    /// Receive-to-transmit turnaround in ms; `0` = 500.
    pub turnaround_ms: u32,
}

/// One link event (`coppa_link_event_t`), filled by [`coppa_link_next_event`].
#[repr(C)]
#[derive(Debug)]
pub struct CoppaLinkEvent {
    /// `COPPA_LINK_EVENT_CONNECTED` or `COPPA_LINK_EVENT_DISCONNECTED`; 0 when none.
    pub kind: i32,
    /// `COPPA_LINK_REASON_*` when `kind` is `_DISCONNECTED`, else 0.
    pub reason: i32,
    /// NUL-terminated peer callsign.
    pub remote: [c_char; 16],
}

fn link_error_code(e: &coppa_engine::LinkError) -> i32 {
    use coppa_engine::LinkError;
    match e {
        LinkError::NotConnected { .. } | LinkError::NotIdle { .. } => COPPA_ERR_INVALID_STATE,
        LinkError::Engine(_) => COPPA_ERR_CODEC,
        _ => COPPA_ERR_INVALID_PARAM,
    }
}

fn link_err(e: coppa_engine::LinkError) -> (i32, String) {
    (link_error_code(&e), e.to_string())
}

/// Null check, clear the error slot, lock, catch panics and record messages:
/// the shared body of every `coppa_link_*` call that takes a link.
unsafe fn with_link<F>(link: *const CoppaLink, func: &str, f: F) -> i32
where
    F: FnOnce(&mut LinkInner) -> Result<i32, (i32, String)>,
{
    clear_error();
    if link.is_null() {
        return null_arg(func, "link");
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut guard = match lock_or_poisoned(&(*link).inner, func) {
            Ok(g) => g,
            Err(code) => return code,
        };
        match f(&mut guard) {
            Ok(code) => code,
            Err((code, reason)) => {
                set_error(func, reason);
                code
            }
        }
    }));
    finish(func, result)
}

/// Create a link from `cfg`. Returns null on failure; `coppa_last_error()`
/// says why. Free it with [`coppa_link_destroy`].
///
/// # Safety
///
/// `cfg` must be null or point to a `coppa_link_config_t` whose
/// `struct_size` is set, with NUL-terminated (or null) strings.
#[no_mangle]
pub unsafe extern "C" fn coppa_link_new(cfg: *const CoppaLinkConfig) -> *mut CoppaLink {
    const F: &str = "coppa_link_new";
    clear_error();
    if cfg.is_null() {
        set_error(F, "cfg is null");
        return std::ptr::null_mut();
    }
    // Read struct_size alone first: an older caller's struct may be shorter.
    let struct_size = std::ptr::addr_of!((*cfg).struct_size).read();
    let need = std::mem::size_of::<CoppaLinkConfig>();
    if (struct_size as usize) < need {
        set_error(
            F,
            format_args!(
                "struct_size {struct_size} is smaller than this library's coppa_link_config_t ({need} bytes); set it to sizeof(coppa_link_config_t)"
            ),
        );
        return std::ptr::null_mut();
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let cfg = &*cfg;
        if cfg.callsign.is_null() {
            set_error(F, "callsign is null");
            return std::ptr::null_mut();
        }
        let callsign = match CStr::from_ptr(cfg.callsign).to_str() {
            Ok(s) => s,
            Err(_) => {
                set_error(F, "callsign is not valid UTF-8");
                return std::ptr::null_mut();
            }
        };
        let mut engine = match resolve_profile(cfg.profile) {
            Ok(c) => c,
            Err(reason) => {
                set_error(F, reason);
                return std::ptr::null_mut();
            }
        };
        if cfg.speed_level != 0 {
            engine.speed_level = cfg.speed_level;
        }
        let mut lc = coppa_engine::LinkConfig::new(callsign);
        lc.engine = engine;
        if cfg.turnaround_ms != 0 {
            lc.turnaround = Duration::from_millis(u64::from(cfg.turnaround_ms));
        }
        match coppa_engine::Link::new(lc) {
            Ok(link) => Box::into_raw(Box::new(CoppaLink {
                inner: Mutex::new(LinkInner {
                    link,
                    base: Instant::now(),
                    last_now_ms: None,
                }),
            })),
            Err(e) => {
                set_error(F, e);
                std::ptr::null_mut()
            }
        }
    }));
    finish_ptr(F, result)
}

/// Destroy a link and set `*link` to null. Null or already-null is a no-op.
///
/// # Safety
///
/// `link` must be null or point to a variable holding a pointer from
/// [`coppa_link_new`]. No other call on that link may be in progress.
#[no_mangle]
pub unsafe extern "C" fn coppa_link_destroy(link: *mut *mut CoppaLink) {
    if link.is_null() {
        return;
    }
    let h = *link;
    if !h.is_null() {
        drop(Box::from_raw(h));
        *link = std::ptr::null_mut();
    }
}

/// Accept (`on = true`) or refuse incoming connections while idle.
///
/// # Safety
///
/// `link` must be null or a live pointer from [`coppa_link_new`].
#[no_mangle]
pub unsafe extern "C" fn coppa_link_listen(link: *mut CoppaLink, on: bool) -> i32 {
    with_link(link, "coppa_link_listen", |l| {
        l.link.set_listening(on);
        Ok(COPPA_OK)
    })
}

/// Start a CONNECT handshake to `remote` at time `now_ms`.
///
/// # Safety
///
/// `link` must be null or a live pointer from [`coppa_link_new`]; `remote`
/// must be null or NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn coppa_link_connect(
    link: *mut CoppaLink,
    remote: *const c_char,
    now_ms: u64,
) -> i32 {
    with_link(link, "coppa_link_connect", |l| {
        if remote.is_null() {
            return Err((COPPA_ERR_NULL_ARG, "remote is null".to_owned()));
        }
        let remote = CStr::from_ptr(remote).to_str().map_err(|_| {
            (
                COPPA_ERR_INVALID_UTF8,
                "remote is not valid UTF-8".to_owned(),
            )
        })?;
        let now = l.instant(now_ms)?;
        l.link.connect(remote, now).map_err(link_err)?;
        Ok(COPPA_OK)
    })
}

/// Queue `len` bytes for reliable delivery. Needs `COPPA_LINK_STATE_CONNECTED`.
///
/// # Safety
///
/// `link` must be null or a live pointer from [`coppa_link_new`]; `data`
/// must point to `len` readable bytes (or be null when `len` is 0).
#[no_mangle]
pub unsafe extern "C" fn coppa_link_send(link: *mut CoppaLink, data: *const u8, len: usize) -> i32 {
    with_link(link, "coppa_link_send", |l| {
        if data.is_null() && len > 0 {
            return Err((COPPA_ERR_NULL_ARG, "data is null".to_owned()));
        }
        let bytes: &[u8] = if len == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(data, len)
        };
        l.link.send(bytes).map_err(link_err)?;
        Ok(COPPA_OK)
    })
}

/// Disconnect gracefully: queued data is delivered first, then DISCONNECT.
///
/// # Safety
///
/// `link` must be null or a live pointer from [`coppa_link_new`].
#[no_mangle]
pub unsafe extern "C" fn coppa_link_disconnect(link: *mut CoppaLink, now_ms: u64) -> i32 {
    with_link(link, "coppa_link_disconnect", |l| {
        let now = l.instant(now_ms)?;
        l.link.disconnect(now).map_err(link_err)?;
        Ok(COPPA_OK)
    })
}

/// Run the link for one audio block: an audio callback.
///
/// `rx` holds the 48 kHz mono samples captured since the last call (any
/// count; ignored while the link transmits). The link writes its signal at
/// the start of `tx`, zero-fills the rest, and stores the signal length in
/// `*tx_active` when that pointer is not null. Key the transmitter while it
/// is above 0. `now_ms` is a monotonic millisecond clock and must never
/// decrease for one link.
///
/// # Safety
///
/// `link` must be null or a live pointer from [`coppa_link_new`]; `rx`
/// must point to `rx_len` floats and `tx` to `tx_len` writable floats (either
/// may be null when its length is 0); `tx_active` must be null or writable.
#[no_mangle]
pub unsafe extern "C" fn coppa_link_process(
    link: *mut CoppaLink,
    now_ms: u64,
    rx: *const f32,
    rx_len: usize,
    tx: *mut f32,
    tx_len: usize,
    tx_active: *mut usize,
) -> i32 {
    with_link(link, "coppa_link_process", |l| {
        if rx.is_null() && rx_len > 0 {
            return Err((COPPA_ERR_NULL_ARG, "rx is null".to_owned()));
        }
        if tx.is_null() && tx_len > 0 {
            return Err((COPPA_ERR_NULL_ARG, "tx is null".to_owned()));
        }
        let now = l.instant(now_ms)?;
        // A caller may pass one buffer for both rx and tx (an in-place
        // audio callback). A shared slice over memory that a mutable slice
        // also covers is undefined behaviour, so copy rx first when the two
        // ranges overlap.
        let overlaps = rx_len > 0 && tx_len > 0 && {
            let (r0, r1) = (
                rx as usize,
                rx as usize + rx_len * std::mem::size_of::<f32>(),
            );
            let (t0, t1) = (
                tx as usize,
                tx as usize + tx_len * std::mem::size_of::<f32>(),
            );
            r0 < t1 && t0 < r1
        };
        let rx_copy: Vec<f32>;
        let rx: &[f32] = if rx_len == 0 {
            &[]
        } else if overlaps {
            rx_copy = std::slice::from_raw_parts(rx, rx_len).to_vec();
            &rx_copy
        } else {
            std::slice::from_raw_parts(rx, rx_len)
        };
        let tx: &mut [f32] = if tx_len == 0 {
            &mut []
        } else {
            std::slice::from_raw_parts_mut(tx, tx_len)
        };
        let active = l.link.process(now, rx, tx);
        if !tx_active.is_null() {
            *tx_active = active;
        }
        Ok(COPPA_OK)
    })
}

/// Copy up to `cap` received bytes into `buf` and store the count in
/// `*out_len` (0 when nothing has arrived).
///
/// # Safety
///
/// `link` must be null or a live pointer from [`coppa_link_new`]; `buf`
/// must point to `cap` writable bytes (or be null when `cap` is 0);
/// `out_len` must be writable.
#[no_mangle]
pub unsafe extern "C" fn coppa_link_recv(
    link: *mut CoppaLink,
    buf: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> i32 {
    with_link(link, "coppa_link_recv", |l| {
        if buf.is_null() && cap > 0 {
            return Err((COPPA_ERR_NULL_ARG, "buf is null".to_owned()));
        }
        if out_len.is_null() {
            return Err((COPPA_ERR_NULL_ARG, "out_len is null".to_owned()));
        }
        let out: &mut [u8] = if cap == 0 {
            &mut []
        } else {
            std::slice::from_raw_parts_mut(buf, cap)
        };
        *out_len = l.link.recv(out);
        Ok(COPPA_OK)
    })
}

/// Pop the next link event into `*out`: `0` when one was written,
/// `COPPA_NONE_PENDING` (1, with `*out` zeroed) when none is pending.
///
/// # Safety
///
/// `link` must be null or a live pointer from [`coppa_link_new`]; `out`
/// must be writable.
#[no_mangle]
pub unsafe extern "C" fn coppa_link_next_event(
    link: *mut CoppaLink,
    out: *mut CoppaLinkEvent,
) -> i32 {
    use coppa_engine::{DisconnectReason, LinkEvent};
    with_link(link, "coppa_link_next_event", |l| {
        if out.is_null() {
            return Err((COPPA_ERR_NULL_ARG, "out is null".to_owned()));
        }
        let mut ev = CoppaLinkEvent {
            kind: 0,
            reason: COPPA_LINK_REASON_NONE,
            remote: [0; 16],
        };
        let remote = loop {
            match l.link.poll_event() {
                Some(LinkEvent::Connected { remote }) => {
                    ev.kind = COPPA_LINK_EVENT_CONNECTED;
                    break Some(remote);
                }
                Some(LinkEvent::Disconnected { remote, reason }) => {
                    ev.kind = COPPA_LINK_EVENT_DISCONNECTED;
                    ev.reason = match reason {
                        DisconnectReason::Local => COPPA_LINK_REASON_LOCAL,
                        DisconnectReason::Remote => COPPA_LINK_REASON_REMOTE,
                        DisconnectReason::Timeout => COPPA_LINK_REASON_TIMEOUT,
                        DisconnectReason::RetriesExhausted => COPPA_LINK_REASON_RETRIES_EXHAUSTED,
                        DisconnectReason::HandshakeFailed => COPPA_LINK_REASON_HANDSHAKE_FAILED,
                        _ => COPPA_LINK_REASON_NONE,
                    };
                    break Some(remote);
                }
                // A future event kind this ABI cannot express: skip it.
                Some(_) => continue,
                None => break None,
            }
        };
        let code = match remote {
            Some(remote) => {
                // NUL-terminated, truncated to 15 bytes (callsigns are <= 8).
                for (dst, src) in ev.remote.iter_mut().zip(remote.bytes().take(15)) {
                    *dst = src as c_char;
                }
                COPPA_OK
            }
            None => COPPA_NONE_PENDING,
        };
        *out = ev;
        Ok(code)
    })
}

/// The link's state, a `COPPA_LINK_STATE_*` value, or a negative error code.
///
/// # Safety
///
/// `link` must be null or a live pointer from [`coppa_link_new`].
#[no_mangle]
pub unsafe extern "C" fn coppa_link_state(link: *const CoppaLink) -> i32 {
    use coppa_engine::LinkState;
    with_link(link, "coppa_link_state", |l| {
        Ok(match l.link.state() {
            LinkState::Idle => COPPA_LINK_STATE_IDLE,
            LinkState::Connecting => COPPA_LINK_STATE_CONNECTING,
            LinkState::Connected => COPPA_LINK_STATE_CONNECTED,
            LinkState::Disconnecting => COPPA_LINK_STATE_DISCONNECTING,
            _ => COPPA_LINK_STATE_IDLE,
        })
    })
}

/// Store the number of bytes queued or sent but not yet acknowledged in
/// `*out_bytes`.
///
/// # Safety
///
/// `link` must be null or a live pointer from [`coppa_link_new`];
/// `out_bytes` must be writable.
#[no_mangle]
pub unsafe extern "C" fn coppa_link_pending(link: *const CoppaLink, out_bytes: *mut usize) -> i32 {
    with_link(link, "coppa_link_pending", |l| {
        if out_bytes.is_null() {
            return Err((COPPA_ERR_NULL_ARG, "out_bytes is null".to_owned()));
        }
        *out_bytes = l.link.pending_tx_bytes();
        Ok(COPPA_OK)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        let ptr = coppa_version();
        let version = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn test_create_destroy() {
        let mut handle = coppa_engine_create();
        assert!(!handle.is_null());
        unsafe {
            coppa_engine_destroy(&mut handle);
        }
    }

    #[test]
    fn test_encode_null_handle() {
        let mut out_samples: *mut f32 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let msg = c"hello".as_ptr();
        let result =
            unsafe { coppa_encode(std::ptr::null_mut(), msg, &mut out_samples, &mut out_len) };
        assert_eq!(result, -1);
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let mut handle = coppa_engine_create();
        assert!(!handle.is_null());

        let msg = c"Hello".as_ptr();
        let mut out_samples: *mut f32 = std::ptr::null_mut();
        let mut out_len: usize = 0;

        let result = unsafe { coppa_encode(handle, msg, &mut out_samples, &mut out_len) };
        assert_eq!(result, 0);
        assert!(!out_samples.is_null());
        assert!(out_len > 0);

        let mut out_message: *mut c_char = std::ptr::null_mut();
        let result = unsafe { coppa_decode(handle, out_samples, out_len, &mut out_message) };
        assert_eq!(result, 0);
        assert!(!out_message.is_null());

        let decoded = unsafe { CStr::from_ptr(out_message) }.to_str().unwrap();
        assert_eq!(decoded, "Hello");

        unsafe {
            coppa_free_samples(out_samples, out_len);
            coppa_free_string(out_message);
            coppa_engine_destroy(&mut handle);
        }
    }

    #[test]
    fn test_decode_null_handle() {
        let mut out_message: *mut c_char = std::ptr::null_mut();
        let samples = [0.0f32; 10];
        let result = unsafe {
            coppa_decode(
                std::ptr::null_mut(),
                samples.as_ptr(),
                samples.len(),
                &mut out_message,
            )
        };
        assert_eq!(result, -1);
    }

    #[test]
    #[allow(deprecated)]
    fn test_stream_lifecycle() {
        let handle = coppa_start_stream();
        assert!(!handle.is_null());

        let samples = [0.0f32; 1000];
        let result = unsafe { coppa_feed_samples(handle, samples.as_ptr(), samples.len()) };
        assert_eq!(result, 0);

        let msg = unsafe { coppa_get_decoded(handle) };
        assert!(msg.is_null());

        unsafe { coppa_stop_stream(handle) };
    }

    #[test]
    #[allow(deprecated)]
    fn test_stream_null_handle() {
        let result = unsafe { coppa_feed_samples(std::ptr::null_mut(), [0.0f32].as_ptr(), 1) };
        assert_eq!(result, -1);

        let msg = unsafe { coppa_get_decoded(std::ptr::null_mut()) };
        assert!(msg.is_null());
    }

    #[test]
    fn test_double_destroy() {
        // Test that passing a null pointer-to-pointer is a safe no-op.
        unsafe {
            coppa_engine_destroy(std::ptr::null_mut());
        }
        // Test that passing a pointer to a null handle is a safe no-op.
        let mut null_handle: *mut CoppaHandle = std::ptr::null_mut();
        unsafe {
            coppa_engine_destroy(&mut null_handle);
        }
    }

    #[test]
    fn test_double_destroy_nonnull() {
        let handle = coppa_engine_create();
        assert!(!handle.is_null());
        let mut handle_slot = handle;
        unsafe {
            coppa_engine_destroy(&mut handle_slot);
            assert!(
                handle_slot.is_null(),
                "handle should be nulled after destroy"
            );
            coppa_engine_destroy(&mut handle_slot); // second call: no-op
        }
    }

    #[test]
    fn test_encode_invalid_utf8() {
        let mut handle = coppa_engine_create();
        let mut out_samples: *mut f32 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let bad_msg = [0xFF, 0xFE, 0x00];
        let result = unsafe {
            coppa_encode(
                handle,
                bad_msg.as_ptr() as *const c_char,
                &mut out_samples,
                &mut out_len,
            )
        };
        assert_eq!(result, -2, "Invalid UTF-8 should return -2");
        unsafe { coppa_engine_destroy(&mut handle) };
    }

    #[test]
    fn test_free_null_samples() {
        unsafe { coppa_free_samples(std::ptr::null_mut(), 0) };
    }

    #[test]
    fn test_free_null_string() {
        unsafe { coppa_free_string(std::ptr::null_mut()) };
    }

    #[test]
    #[allow(deprecated)]
    fn test_stream_roundtrip() {
        // Updated for Task 7's `StreamingReceiver` migration: unlike the old
        // 10 000-sample-threshold + full-buffer-rescan design (which this test
        // used to tolerate a `None` decode result for), the streaming receiver
        // deterministically finds and decodes a frame given a real silence
        // lead-in (its `SyncDetector` needs a clean baseline in its bootstrap
        // window before a preamble — see `coppa_codec::ofdm::sync_detector`'s
        // and `coppa_protocol::modem::streaming`'s own tests) and a little
        // trailing pad (the RX bandpass filter's group delay shifts the frame's
        // content later in the filtered domain the receiver operates in). This
        // test now asserts the decode unconditionally instead of only checking
        // it "if" a message happened to be produced.
        let mut handle = coppa_engine_create();
        let msg = c"Test".as_ptr();
        let mut out_samples: *mut f32 = std::ptr::null_mut();
        let mut out_len: usize = 0;

        let result = unsafe { coppa_encode(handle, msg, &mut out_samples, &mut out_len) };
        assert_eq!(result, 0);

        let stream = coppa_start_stream();
        assert!(!stream.is_null());

        let encoded = unsafe { std::slice::from_raw_parts(out_samples, out_len) };
        let mut sample_slice = vec![0.0f32; 8192];
        sample_slice.extend_from_slice(encoded);
        sample_slice.extend(std::iter::repeat_n(0.0f32, 2048));

        let result =
            unsafe { coppa_feed_samples(stream, sample_slice.as_ptr(), sample_slice.len()) };
        assert_eq!(result, 0);

        let decoded = unsafe { coppa_get_decoded(stream) };
        assert!(
            !decoded.is_null(),
            "streaming receiver should have decoded the fed frame"
        );
        let decoded_str = unsafe { CStr::from_ptr(decoded) }.to_str().unwrap();
        assert_eq!(decoded_str, "Test");
        unsafe { coppa_free_string(decoded) };

        unsafe {
            coppa_free_samples(out_samples, out_len);
            coppa_stop_stream(stream);
            coppa_engine_destroy(&mut handle);
        }
    }

    #[test]
    #[allow(deprecated)]
    fn test_stream_roundtrip_fed_in_odd_chunks() {
        // Exercises the new streaming design's core promise: any chunk size,
        // fed at any time, with no minimum-samples threshold to reach first.
        let mut handle = coppa_engine_create();
        let msg = c"Chunked FFI stream".as_ptr();
        let mut out_samples: *mut f32 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let result = unsafe { coppa_encode(handle, msg, &mut out_samples, &mut out_len) };
        assert_eq!(result, 0);

        let encoded = unsafe { std::slice::from_raw_parts(out_samples, out_len) };
        let mut full = vec![0.0f32; 8192];
        full.extend_from_slice(encoded);
        full.extend(std::iter::repeat_n(0.0f32, 2048));

        let stream = coppa_start_stream();
        assert!(!stream.is_null());

        let mut decoded_str = None;
        let chunk_sizes = [1usize, 7, 64, 480, 4096];
        let mut i = 0;
        let mut chunk_idx = 0;
        while i < full.len() {
            let want = chunk_sizes[chunk_idx % chunk_sizes.len()];
            chunk_idx += 1;
            let end = (i + want).min(full.len());
            let result = unsafe { coppa_feed_samples(stream, full[i..end].as_ptr(), end - i) };
            assert_eq!(result, 0);
            let decoded = unsafe { coppa_get_decoded(stream) };
            if !decoded.is_null() {
                decoded_str = Some(
                    unsafe { CStr::from_ptr(decoded) }
                        .to_str()
                        .unwrap()
                        .to_string(),
                );
                unsafe { coppa_free_string(decoded) };
            }
            i = end;
        }

        assert_eq!(decoded_str.as_deref(), Some("Chunked FFI stream"));

        unsafe {
            coppa_free_samples(out_samples, out_len);
            coppa_stop_stream(stream);
            coppa_engine_destroy(&mut handle);
        }
    }

    #[test]
    fn test_concurrent_handles() {
        use std::thread;

        let handles: Vec<_> = (0..4)
            .map(|_| {
                thread::spawn(|| {
                    let mut handle = coppa_engine_create();
                    assert!(!handle.is_null());

                    let msg = c"Hi".as_ptr();
                    let mut out_samples: *mut f32 = std::ptr::null_mut();
                    let mut out_len: usize = 0;

                    let result =
                        unsafe { coppa_encode(handle, msg, &mut out_samples, &mut out_len) };
                    assert_eq!(result, 0);

                    unsafe {
                        coppa_free_samples(out_samples, out_len);
                        coppa_engine_destroy(&mut handle);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn test_large_message() {
        let mut handle = coppa_engine_create();
        // Use 50 bytes — fits within the default OFDM frame capacity
        // (speed_level=1, hf_standard, 44 data carriers).
        let mut large = vec![b'A'; 50];
        large.push(0);
        let mut out_samples: *mut f32 = std::ptr::null_mut();
        let mut out_len: usize = 0;

        let result = unsafe {
            coppa_encode(
                handle,
                large.as_ptr() as *const c_char,
                &mut out_samples,
                &mut out_len,
            )
        };
        assert_eq!(result, 0);
        assert!(out_len > 0);

        let mut out_message: *mut c_char = std::ptr::null_mut();
        let result = unsafe { coppa_decode(handle, out_samples, out_len, &mut out_message) };
        assert_eq!(result, 0);

        let decoded = unsafe { CStr::from_ptr(out_message) }.to_str().unwrap();
        assert_eq!(decoded.len(), 50);
        assert!(decoded.chars().all(|c| c == 'A'));

        unsafe {
            coppa_free_samples(out_samples, out_len);
            coppa_free_string(out_message);
            coppa_engine_destroy(&mut handle);
        }
    }

    // ── Task 6 (Phase 4): FFI v2 binary/config/event API ──────────────────

    /// Same silence lead-in/trail-out rationale as `coppa-engine`'s own
    /// `with_lead_and_trail` test helper: the streaming `SyncDetector` needs
    /// a clean baseline before a preamble, and the RX bandpass filter's
    /// group delay needs a little trailing pad.
    fn with_lead_and_trail(samples: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0f32; 8192];
        out.extend_from_slice(samples);
        out.extend(std::iter::repeat_n(0.0f32, 2048));
        out
    }

    fn null_config() -> CoppaConfig {
        CoppaConfig {
            sample_rate: 0,
            profile: std::ptr::null(),
            speed_level: 0,
            callsign: std::ptr::null(),
        }
    }

    #[test]
    fn test_encode_bytes_feed_next_frame_roundtrip() {
        // The brief's headline round trip: encode bytes -> feed -> next_frame
        // returns the payload + plausible snr, driven entirely through the
        // real extern "C" surface (no internal shortcuts).
        let mut handle = coppa_engine_create();
        assert!(!handle.is_null());

        let data = b"FFI v2 binary round trip";
        let mut out_samples: *mut f32 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let result = unsafe {
            coppa_encode_bytes(
                handle,
                data.as_ptr(),
                data.len(),
                &mut out_samples,
                &mut out_len,
            )
        };
        assert_eq!(result, 0);
        assert!(!out_samples.is_null());
        assert!(out_len > 0);

        let encoded = unsafe { std::slice::from_raw_parts(out_samples, out_len) };
        let padded = with_lead_and_trail(encoded);

        let feed_result =
            unsafe { coppa_engine_feed_samples(handle, padded.as_ptr(), padded.len()) };
        assert_eq!(feed_result, 0);

        let mut frame = CoppaFrame {
            payload: std::ptr::null_mut(),
            payload_len: 0,
            snr_db: 0.0,
            cfo_hz: 0.0,
            speed_level: 0,
            seq: 0,
        };
        let pop_result = unsafe { coppa_next_frame(handle, &mut frame) };
        assert_eq!(pop_result, 0, "a frame should have been popped");
        assert!(!frame.payload.is_null());
        assert_eq!(frame.payload_len, data.len());

        let payload = unsafe { std::slice::from_raw_parts(frame.payload, frame.payload_len) };
        assert_eq!(payload, data);
        assert!(
            frame.snr_db.is_finite(),
            "snr_db should be a plausible finite estimate"
        );
        assert!(frame.cfo_hz.is_finite());
        assert_eq!(
            frame.speed_level, 1,
            "default engine config is speed_level 1"
        );
        assert_eq!(frame.seq, 0, "first frame queued on this handle");

        unsafe {
            coppa_free_frame_payload(frame.payload, frame.payload_len);
            coppa_free_samples(out_samples, out_len);
            coppa_engine_destroy(&mut handle);
        }
    }

    #[test]
    fn test_next_frame_recovers_non_utf8_binary_payload() {
        // The core differentiator from the deprecated v1 quartet: no UTF-8
        // forcing anywhere in the v2 path.
        let mut handle = coppa_engine_create();
        let data: Vec<u8> = vec![b'A', 0xFF, 0xFE, 0x00, 0x01, b'Z'];
        assert!(String::from_utf8(data.clone()).is_err());

        let mut out_samples: *mut f32 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let result = unsafe {
            coppa_encode_bytes(
                handle,
                data.as_ptr(),
                data.len(),
                &mut out_samples,
                &mut out_len,
            )
        };
        assert_eq!(result, 0);

        let encoded = unsafe { std::slice::from_raw_parts(out_samples, out_len) };
        let padded = with_lead_and_trail(encoded);
        let feed_result =
            unsafe { coppa_engine_feed_samples(handle, padded.as_ptr(), padded.len()) };
        assert_eq!(feed_result, 0);

        let mut frame = CoppaFrame {
            payload: std::ptr::null_mut(),
            payload_len: 0,
            snr_db: 0.0,
            cfo_hz: 0.0,
            speed_level: 0,
            seq: 0,
        };
        let pop_result = unsafe { coppa_next_frame(handle, &mut frame) };
        assert_eq!(pop_result, 0);
        let payload = unsafe { std::slice::from_raw_parts(frame.payload, frame.payload_len) };
        assert_eq!(payload, data.as_slice());

        unsafe {
            coppa_free_frame_payload(frame.payload, frame.payload_len);
            coppa_free_samples(out_samples, out_len);
            coppa_engine_destroy(&mut handle);
        }
    }

    #[test]
    fn test_next_frame_none_pending_returns_1_and_zeroes_out() {
        let mut handle = coppa_engine_create();
        assert!(!handle.is_null());

        let mut frame = CoppaFrame {
            payload: 0xdead_beef as *mut u8, // sentinel, must be overwritten
            payload_len: 12345,
            snr_db: 9.0,
            cfo_hz: 9.0,
            speed_level: 9,
            seq: 9,
        };
        let result = unsafe { coppa_next_frame(handle, &mut frame) };
        assert_eq!(
            result, 1,
            "no frame pending must return 1, not 0 or an error"
        );
        assert!(frame.payload.is_null());
        assert_eq!(frame.payload_len, 0);
        assert_eq!(frame.snr_db, 0.0);
        assert_eq!(frame.cfo_hz, 0.0);
        assert_eq!(frame.speed_level, 0);
        assert_eq!(frame.seq, 0);

        // Freeing the zeroed (null-payload) frame must be a safe no-op.
        unsafe {
            coppa_free_frame_payload(frame.payload, frame.payload_len);
            coppa_engine_destroy(&mut handle);
        }
    }

    #[test]
    fn test_next_frame_seq_increments_per_queued_frame() {
        let mut handle = coppa_engine_create();
        let mut all_samples = Vec::new();
        for msg in [&b"one"[..], &b"two"[..], &b"three"[..]] {
            let mut out_samples: *mut f32 = std::ptr::null_mut();
            let mut out_len: usize = 0;
            let result = unsafe {
                coppa_encode_bytes(
                    handle,
                    msg.as_ptr(),
                    msg.len(),
                    &mut out_samples,
                    &mut out_len,
                )
            };
            assert_eq!(result, 0);
            let encoded = unsafe { std::slice::from_raw_parts(out_samples, out_len) };
            all_samples.extend_from_slice(encoded);
            all_samples.extend(std::iter::repeat_n(0.0f32, 4096));
            unsafe { coppa_free_samples(out_samples, out_len) };
        }
        let padded = with_lead_and_trail(&all_samples);
        let feed_result =
            unsafe { coppa_engine_feed_samples(handle, padded.as_ptr(), padded.len()) };
        assert_eq!(feed_result, 0);

        let mut seqs = Vec::new();
        loop {
            let mut frame = CoppaFrame {
                payload: std::ptr::null_mut(),
                payload_len: 0,
                snr_db: 0.0,
                cfo_hz: 0.0,
                speed_level: 0,
                seq: 0,
            };
            let result = unsafe { coppa_next_frame(handle, &mut frame) };
            if result != 0 {
                break;
            }
            seqs.push(frame.seq);
            unsafe { coppa_free_frame_payload(frame.payload, frame.payload_len) };
        }
        assert!(!seqs.is_empty(), "expected at least one decoded frame");
        for (i, &s) in seqs.iter().enumerate() {
            assert_eq!(
                s, i as u64,
                "seq should be a monotonically increasing per-handle counter"
            );
        }

        unsafe { coppa_engine_destroy(&mut handle) };
    }

    #[test]
    fn test_coppa_engine_feed_samples_null_handle() {
        let samples = [0.0f32; 4];
        let result = unsafe {
            coppa_engine_feed_samples(std::ptr::null_mut(), samples.as_ptr(), samples.len())
        };
        assert_eq!(result, -1);
    }

    #[test]
    fn test_coppa_encode_bytes_null_handle() {
        let mut out_samples: *mut f32 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let data = [1u8, 2, 3];
        let result = unsafe {
            coppa_encode_bytes(
                std::ptr::null_mut(),
                data.as_ptr(),
                data.len(),
                &mut out_samples,
                &mut out_len,
            )
        };
        assert_eq!(result, -1);
    }

    #[test]
    fn test_next_frame_null_args() {
        let mut handle = coppa_engine_create();
        let mut frame = CoppaFrame {
            payload: std::ptr::null_mut(),
            payload_len: 0,
            snr_db: 0.0,
            cfo_hz: 0.0,
            speed_level: 0,
            seq: 0,
        };
        assert_eq!(
            unsafe { coppa_next_frame(std::ptr::null_mut(), &mut frame) },
            -1
        );
        assert_eq!(
            unsafe { coppa_next_frame(handle, std::ptr::null_mut()) },
            -1
        );
        unsafe { coppa_engine_destroy(&mut handle) };
    }

    // ── coppa_engine_new_with / CoppaConfig ────────────────────────────────

    #[test]
    fn test_new_with_null_cfg_returns_null() {
        let handle = unsafe { coppa_engine_new_with(std::ptr::null()) };
        assert!(handle.is_null());
    }

    #[test]
    fn test_new_with_default_config() {
        let cfg = null_config();
        let mut handle = unsafe { coppa_engine_new_with(&cfg) };
        assert!(!handle.is_null());
        let speed_level = unsafe { handle.as_ref() }
            .unwrap()
            .engine
            .lock()
            .unwrap()
            .config()
            .speed_level;
        assert_eq!(
            speed_level, 1,
            "empty config should match EngineConfig::default()"
        );
        unsafe { coppa_engine_destroy(&mut handle) };
    }

    #[test]
    fn test_new_with_named_profile_resolves_like_cli() {
        let profile_name = c"HF_ROBUST";
        let cfg = CoppaConfig {
            sample_rate: 0,
            profile: profile_name.as_ptr(),
            speed_level: 0,
            callsign: std::ptr::null(),
        };
        let mut handle = unsafe { coppa_engine_new_with(&cfg) };
        assert!(!handle.is_null());
        let speed_level = unsafe { handle.as_ref() }
            .unwrap()
            .engine
            .lock()
            .unwrap()
            .config()
            .speed_level;
        assert_eq!(speed_level, coppa_engine::HF_ROBUST.speed_level);
        unsafe { coppa_engine_destroy(&mut handle) };
    }

    #[test]
    fn test_new_with_speed_level_overrides_profile() {
        let profile_name = c"HF_ROBUST"; // profile speed_level = 1
        let cfg = CoppaConfig {
            sample_rate: 0,
            profile: profile_name.as_ptr(),
            speed_level: 3,
            callsign: std::ptr::null(),
        };
        let mut handle = unsafe { coppa_engine_new_with(&cfg) };
        assert!(!handle.is_null());
        let speed_level = unsafe { handle.as_ref() }
            .unwrap()
            .engine
            .lock()
            .unwrap()
            .config()
            .speed_level;
        assert_eq!(
            speed_level, 3,
            "explicit speed_level should override the profile's own"
        );
        unsafe { coppa_engine_destroy(&mut handle) };
    }

    /// COP-23: the `speed_level` override changes the level, never the profile's
    /// band. Pre-fix an `HF_ROBUST` handle using 64-QAM 2/3 (now level 8) went on air as
    /// `vhf_wide`, which a level-1 `HF_ROBUST` handle (`hf_standard`) cannot even
    /// sync to -- so a successful cross-handle decode is the wire-level proof,
    /// using only the C API.
    #[test]
    fn new_with_hf_profile_and_high_speed_level_stays_hf() {
        let profile_name = c"HF_ROBUST";
        let high = CoppaConfig {
            sample_rate: 0,
            profile: profile_name.as_ptr(),
            speed_level: 8,
            callsign: std::ptr::null(),
        };
        let low = CoppaConfig {
            speed_level: 0,
            ..high
        };
        let mut h1 = unsafe { coppa_engine_new_with(&high) };
        let mut h2 = unsafe { coppa_engine_new_with(&low) };
        assert!(!h1.is_null() && !h2.is_null());

        let data = b"HF at level eight";
        let mut out_samples: *mut f32 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let result = unsafe {
            coppa_encode_bytes(
                h1,
                data.as_ptr(),
                data.len(),
                &mut out_samples,
                &mut out_len,
            )
        };
        assert_eq!(result, 0);

        let mut out_message: *mut c_char = std::ptr::null_mut();
        let result = unsafe { coppa_decode(h2, out_samples, out_len, &mut out_message) };
        assert_eq!(
            result, 0,
            "a level-1 HF_ROBUST handle must decode a level-8 HF_ROBUST handle's \
             frame -- both must be on hf_standard"
        );
        let decoded = unsafe { CStr::from_ptr(out_message) }.to_str().unwrap();
        assert_eq!(decoded.as_bytes(), data);

        unsafe {
            coppa_free_samples(out_samples, out_len);
            coppa_free_string(out_message);
            coppa_engine_destroy(&mut h1);
            coppa_engine_destroy(&mut h2);
        }
    }

    #[test]
    fn test_new_with_rejects_unknown_profile() {
        let profile_name = c"NOT_A_REAL_PROFILE";
        let cfg = CoppaConfig {
            sample_rate: 0,
            profile: profile_name.as_ptr(),
            speed_level: 0,
            callsign: std::ptr::null(),
        };
        let handle = unsafe { coppa_engine_new_with(&cfg) };
        assert!(
            handle.is_null(),
            "unrecognized profile name must be rejected"
        );
    }

    #[test]
    fn hf_handles_reject_vhf_only_level_in_constructor_and_setter() {
        let cfg = CoppaConfig {
            profile: c"HF_ROBUST".as_ptr(),
            speed_level: 9,
            ..null_config()
        };
        assert!(unsafe { coppa_engine_new_with(&cfg) }.is_null());
        let mut hf = unsafe {
            coppa_engine_new_with(&CoppaConfig {
                speed_level: 8,
                ..cfg
            })
        };
        assert!(!hf.is_null());
        assert_eq!(unsafe { coppa_engine_set_speed_level(hf, 9) }, -6);
        assert_eq!(
            unsafe { hf.as_ref() }
                .unwrap()
                .engine
                .lock()
                .unwrap()
                .speed_level(),
            8
        );
        unsafe { coppa_engine_destroy(&mut hf) };

        let mut vhf = unsafe {
            coppa_engine_new_with(&CoppaConfig {
                profile: c"VHF_FAST".as_ptr(),
                speed_level: 0,
                ..null_config()
            })
        };
        assert!(!vhf.is_null());
        assert_eq!(unsafe { coppa_engine_set_speed_level(vhf, 9) }, 0);
        unsafe { coppa_engine_destroy(&mut vhf) };
    }

    #[test]
    fn test_new_with_rejects_removed_speed_level_10() {
        let cfg = CoppaConfig {
            speed_level: 10,
            ..null_config()
        };
        let handle = unsafe { coppa_engine_new_with(&cfg) };
        assert!(
            handle.is_null(),
            "speed level 10 is invalid and must be rejected"
        );
    }

    #[test]
    fn test_new_with_rejects_out_of_range_speed_level() {
        let cfg = CoppaConfig {
            speed_level: 200,
            ..null_config()
        };
        let handle = unsafe { coppa_engine_new_with(&cfg) };
        assert!(handle.is_null());
    }

    #[test]
    fn test_new_with_rejects_invalid_utf8_profile() {
        let bad = [0xFFu8, 0xFE, 0x00];
        let cfg = CoppaConfig {
            profile: bad.as_ptr() as *const c_char,
            ..null_config()
        };
        let handle = unsafe { coppa_engine_new_with(&cfg) };
        assert!(handle.is_null());
    }

    #[test]
    fn test_new_with_rejects_invalid_utf8_callsign() {
        let bad = [0xFFu8, 0xFE, 0x00];
        let cfg = CoppaConfig {
            callsign: bad.as_ptr() as *const c_char,
            ..null_config()
        };
        let handle = unsafe { coppa_engine_new_with(&cfg) };
        assert!(handle.is_null());
    }

    #[test]
    fn test_new_with_accepts_valid_callsign_but_does_not_change_behavior() {
        let callsign = c"VK3ABC";
        let cfg = CoppaConfig {
            callsign: callsign.as_ptr(),
            ..null_config()
        };
        let mut handle = unsafe { coppa_engine_new_with(&cfg) };
        assert!(!handle.is_null());
        unsafe { coppa_engine_destroy(&mut handle) };
    }

    #[test]
    fn test_new_with_sample_rate_override() {
        let cfg = CoppaConfig {
            sample_rate: 48_000,
            ..null_config()
        };
        let mut handle = unsafe { coppa_engine_new_with(&cfg) };
        assert!(!handle.is_null());
        let sample_rate = unsafe { handle.as_ref() }
            .unwrap()
            .engine
            .lock()
            .unwrap()
            .config()
            .sample_rate;
        assert_eq!(sample_rate, 48_000);
        unsafe { coppa_engine_destroy(&mut handle) };
    }

    #[test]
    fn test_new_with_handle_interoperates_with_v1_functions() {
        // v2-constructed handles must work with every v1 function too --
        // one handle type, shared by both API generations.
        let cfg = null_config();
        let mut handle = unsafe { coppa_engine_new_with(&cfg) };
        assert!(!handle.is_null());

        let msg = c"v1 on a v2 handle";
        let mut out_samples: *mut f32 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let result = unsafe { coppa_encode(handle, msg.as_ptr(), &mut out_samples, &mut out_len) };
        assert_eq!(result, 0);

        let mut out_message: *mut c_char = std::ptr::null_mut();
        let result = unsafe { coppa_decode(handle, out_samples, out_len, &mut out_message) };
        assert_eq!(result, 0);
        let decoded = unsafe { CStr::from_ptr(out_message) }.to_str().unwrap();
        assert_eq!(decoded, "v1 on a v2 handle");

        unsafe {
            coppa_free_samples(out_samples, out_len);
            coppa_free_string(out_message);
            coppa_engine_destroy(&mut handle);
        }
    }

    // ── coppa_engine_set_speed_level ───────────────────────────────────────

    #[test]
    fn test_set_speed_level_null_handle() {
        let result = unsafe { coppa_engine_set_speed_level(std::ptr::null_mut(), 2) };
        assert_eq!(result, -1);
    }

    #[test]
    fn test_set_speed_level_valid() {
        let mut handle = coppa_engine_create();
        let result = unsafe { coppa_engine_set_speed_level(handle, 4) };
        assert_eq!(result, 0);
        let speed_level = unsafe { handle.as_ref() }
            .unwrap()
            .engine
            .lock()
            .unwrap()
            .config()
            .speed_level;
        assert_eq!(speed_level, 4);
        unsafe { coppa_engine_destroy(&mut handle) };
    }

    #[test]
    fn test_set_speed_level_rejects_out_of_range() {
        let mut handle = coppa_engine_create();
        assert_eq!(unsafe { coppa_engine_set_speed_level(handle, 10) }, -6);
        assert_eq!(unsafe { coppa_engine_set_speed_level(handle, 0) }, -6);
        assert_eq!(unsafe { coppa_engine_set_speed_level(handle, 255) }, -6);
        // A rejected level must not have mutated the config.
        let speed_level = unsafe { handle.as_ref() }
            .unwrap()
            .engine
            .lock()
            .unwrap()
            .config()
            .speed_level;
        assert_eq!(speed_level, 1, "default engine config is speed_level 1");
        unsafe { coppa_engine_destroy(&mut handle) };
    }

    // ── header freshness (committed coppa.h must contain the new symbols) ─

    #[test]
    fn test_checked_in_header_contains_v2_symbols() {
        // Guards against a forgotten `cargo build` before commit silently
        // shipping a stale coppa.h: cbindgen runs from build.rs on every
        // build, but nothing forces that to have happened before a commit.
        let header_path = concat!(env!("CARGO_MANIFEST_DIR"), "/coppa.h");
        let header = std::fs::read_to_string(header_path)
            .expect("checked-in coppa.h must exist and be readable");

        for symbol in [
            "coppa_engine_t",
            "coppa_config_t",
            "coppa_frame_t",
            "coppa_engine_new_with",
            "coppa_engine_set_speed_level",
            "coppa_encode_bytes",
            "coppa_engine_feed_samples",
            "coppa_next_frame",
            "coppa_free_frame_payload",
        ] {
            assert!(
                header.contains(symbol),
                "checked-in coppa.h is missing v2 symbol `{}` -- run `cargo build -p coppa-ffi` \
                 and commit the regenerated coppa.h",
                symbol
            );
        }
    }

    #[test]
    fn test_checked_in_header_has_extern_c_guard() {
        // Without an `extern "C"` block a C++ translation unit including
        // coppa.h name-mangles every declaration and fails to link against
        // the Rust library's plain C symbols (COP-35). The two strings are
        // checked separately so CRLF checkouts don't matter.
        let header_path = concat!(env!("CARGO_MANIFEST_DIR"), "/coppa.h");
        let header = std::fs::read_to_string(header_path)
            .expect("checked-in coppa.h must exist and be readable");

        for guard in ["#ifdef __cplusplus", "extern \"C\" {"] {
            assert!(
                header.contains(guard),
                "checked-in coppa.h is missing `{}` -- set `cpp_compat = true` in \
                 cbindgen.toml, run `cargo build -p coppa-ffi`, and commit the \
                 regenerated coppa.h",
                guard
            );
        }
    }

    // --- COP-139: error strings, ABI, thread safety, links ---

    /// The calling thread's last error as an owned String, or None.
    fn last_error() -> Option<String> {
        let p = coppa_last_error();
        if p.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned())
        }
    }

    fn header() -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/coppa.h"))
            .expect("checked-in coppa.h must exist and be readable")
    }

    fn cfg_with(profile: *const c_char, level: u8, rate: u32) -> CoppaConfig {
        CoppaConfig {
            sample_rate: rate,
            profile,
            speed_level: level,
            callsign: std::ptr::null(),
        }
    }

    #[test]
    fn test_last_error_is_none_before_any_failure() {
        let seen = std::thread::spawn(last_error).join().unwrap();
        assert_eq!(seen, None);
    }

    #[test]
    fn test_null_handle_sets_message() {
        let null = std::ptr::null_mut();
        let mut s: *mut f32 = std::ptr::null_mut();
        let mut n: usize = 0;
        let mut msg: *mut c_char = std::ptr::null_mut();
        let mut frame = std::mem::MaybeUninit::<CoppaFrame>::uninit();
        let one = [0.0f32; 1];
        let cases: Vec<(&str, i32)> = unsafe {
            vec![
                (
                    "coppa_encode",
                    coppa_encode(null, c"x".as_ptr(), &mut s, &mut n),
                ),
                (
                    "coppa_encode_bytes",
                    coppa_encode_bytes(null, [1u8].as_ptr(), 1, &mut s, &mut n),
                ),
                (
                    "coppa_decode",
                    coppa_decode(null, one.as_ptr(), 1, &mut msg),
                ),
                (
                    "coppa_engine_feed_samples",
                    coppa_engine_feed_samples(null, one.as_ptr(), 1),
                ),
                (
                    "coppa_next_frame",
                    coppa_next_frame(null, frame.as_mut_ptr()),
                ),
                (
                    "coppa_engine_set_speed_level",
                    coppa_engine_set_speed_level(null, 1),
                ),
            ]
        };
        for (func, code) in cases {
            assert_eq!(code, COPPA_ERR_NULL_ARG, "{func}");
            // The slot holds the message of the most recent call only, so
            // re-run each case's check right after the call.
        }
        // Check each message individually.
        unsafe {
            coppa_encode(null, c"x".as_ptr(), &mut s, &mut n);
            assert_eq!(
                last_error().as_deref(),
                Some("coppa_encode: handle is null")
            );
            coppa_encode_bytes(null, [1u8].as_ptr(), 1, &mut s, &mut n);
            assert_eq!(
                last_error().as_deref(),
                Some("coppa_encode_bytes: handle is null")
            );
            coppa_decode(null, one.as_ptr(), 1, &mut msg);
            assert_eq!(
                last_error().as_deref(),
                Some("coppa_decode: handle is null")
            );
            coppa_engine_feed_samples(null, one.as_ptr(), 1);
            assert_eq!(
                last_error().as_deref(),
                Some("coppa_engine_feed_samples: handle is null")
            );
            coppa_next_frame(null, frame.as_mut_ptr());
            assert_eq!(
                last_error().as_deref(),
                Some("coppa_next_frame: handle is null")
            );
            coppa_engine_set_speed_level(null, 1);
            assert_eq!(
                last_error().as_deref(),
                Some("coppa_engine_set_speed_level: handle is null")
            );
            #[allow(deprecated)]
            let p = coppa_get_decoded(std::ptr::null_mut());
            assert!(p.is_null());
            assert_eq!(
                last_error().as_deref(),
                Some("coppa_get_decoded: handle is null")
            );
        }
    }

    #[test]
    fn test_null_out_param_names_the_parameter() {
        let mut h = coppa_engine_create();
        let mut n: usize = 0;
        let rc = unsafe { coppa_encode_bytes(h, [1u8].as_ptr(), 1, std::ptr::null_mut(), &mut n) };
        assert_eq!(rc, COPPA_ERR_NULL_ARG);
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_encode_bytes: out_samples is null")
        );
        unsafe { coppa_engine_destroy(&mut h) };
    }

    #[test]
    fn test_invalid_utf8_message() {
        let mut h = coppa_engine_create();
        let mut s: *mut f32 = std::ptr::null_mut();
        let mut n: usize = 0;
        let bad = [0xFFu8, 0xFE, 0x00];
        let rc = unsafe { coppa_encode(h, bad.as_ptr() as *const c_char, &mut s, &mut n) };
        assert_eq!(rc, COPPA_ERR_INVALID_UTF8);
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_encode: message is not valid UTF-8")
        );
        unsafe { coppa_engine_destroy(&mut h) };
    }

    #[test]
    fn test_oversize_payload_message() {
        let mut h = coppa_engine_create();
        let mut s: *mut f32 = std::ptr::null_mut();
        let mut n: usize = 0;
        let big = vec![0xABu8; 4096];
        let rc = unsafe { coppa_encode_bytes(h, big.as_ptr(), big.len(), &mut s, &mut n) };
        assert_eq!(rc, COPPA_ERR_CODEC);
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_encode_bytes: payload too large for this speed level (max 56 bytes)")
        );
        unsafe { coppa_engine_destroy(&mut h) };
    }

    #[test]
    fn test_set_speed_level_message() {
        let mut h = coppa_engine_create();
        assert_eq!(
            unsafe { coppa_engine_set_speed_level(h, 77) },
            COPPA_ERR_INVALID_PARAM
        );
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_engine_set_speed_level: Invalid speed level: 77")
        );
        unsafe { coppa_engine_destroy(&mut h) };

        let cfg = cfg_with(c"HF_ROBUST".as_ptr(), 0, 0);
        let mut h = unsafe { coppa_engine_new_with(&cfg) };
        assert!(!h.is_null());
        assert_eq!(
            unsafe { coppa_engine_set_speed_level(h, 9) },
            COPPA_ERR_INVALID_PARAM
        );
        assert!(last_error()
            .unwrap()
            .contains("is not available on an HF profile"));
        unsafe { coppa_engine_destroy(&mut h) };
    }

    #[test]
    fn test_new_with_messages() {
        assert!(unsafe { coppa_engine_new_with(std::ptr::null()) }.is_null());
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_engine_new_with: cfg is null")
        );

        let cfg = cfg_with(c"NOPE".as_ptr(), 0, 0);
        assert!(unsafe { coppa_engine_new_with(&cfg) }.is_null());
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_engine_new_with: unknown profile \"NOPE\" (expected HF_ROBUST, HF_STANDARD, VHF_FAST or EMERGENCY)")
        );

        let bad = [0xFFu8, 0xFE, 0x00];
        let cfg = cfg_with(bad.as_ptr() as *const c_char, 0, 0);
        assert!(unsafe { coppa_engine_new_with(&cfg) }.is_null());
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_engine_new_with: profile is not valid UTF-8")
        );

        let mut cfg = cfg_with(std::ptr::null(), 0, 0);
        cfg.callsign = bad.as_ptr() as *const c_char;
        assert!(unsafe { coppa_engine_new_with(&cfg) }.is_null());
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_engine_new_with: callsign is not valid UTF-8")
        );

        let cfg = cfg_with(c"HF_ROBUST".as_ptr(), 9, 0);
        assert!(unsafe { coppa_engine_new_with(&cfg) }.is_null());
        let e = last_error().unwrap();
        assert!(
            e.starts_with("coppa_engine_new_with: speed level 9 is not available on an HF profile"),
            "{e}"
        );
    }

    #[test]
    fn test_new_with_rejects_unsupported_sample_rate() {
        for rate in [8000u32, 44100, 96000] {
            let cfg = cfg_with(std::ptr::null(), 0, rate);
            assert!(unsafe { coppa_engine_new_with(&cfg) }.is_null());
            assert_eq!(
                last_error(),
                Some(format!("coppa_engine_new_with: sample_rate {rate} is not supported: coppa runs at 48000 Hz (pass 0 or 48000)"))
            );
        }
        for rate in [0u32, 48000] {
            let cfg = cfg_with(std::ptr::null(), 0, rate);
            let mut h = unsafe { coppa_engine_new_with(&cfg) };
            assert!(!h.is_null(), "rate {rate}");
            assert_eq!(last_error(), None);
            unsafe { coppa_engine_destroy(&mut h) };
        }
    }

    #[test]
    fn test_success_clears_last_error() {
        assert_eq!(
            unsafe { coppa_engine_set_speed_level(std::ptr::null_mut(), 1) },
            -1
        );
        assert!(last_error().is_some());
        let mut h = coppa_engine_create();
        assert_eq!(last_error(), None);
        let mut s: *mut f32 = std::ptr::null_mut();
        let mut n: usize = 0;
        assert_eq!(
            unsafe { coppa_engine_set_speed_level(std::ptr::null_mut(), 1) },
            -1
        );
        let rc = unsafe { coppa_encode_bytes(h, [1u8, 2, 3, 4].as_ptr(), 4, &mut s, &mut n) };
        assert_eq!(rc, 0);
        assert_eq!(last_error(), None);
        unsafe {
            coppa_free_samples(s, n);
            coppa_engine_destroy(&mut h);
        }
    }

    #[test]
    #[allow(deprecated)]
    fn test_get_decoded_distinguishes_empty_from_error() {
        let h = coppa_start_stream();
        let p = unsafe { coppa_get_decoded(h) };
        assert!(p.is_null());
        assert_eq!(last_error(), None);
        let p = unsafe { coppa_get_decoded(std::ptr::null_mut()) };
        assert!(p.is_null());
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_get_decoded: handle is null")
        );
        unsafe { coppa_stop_stream(h) };
    }

    #[test]
    fn test_next_frame_none_pending_is_not_an_error() {
        let mut h = coppa_engine_create();
        let mut frame = std::mem::MaybeUninit::<CoppaFrame>::uninit();
        assert_eq!(
            unsafe { coppa_next_frame(h, frame.as_mut_ptr()) },
            COPPA_NONE_PENDING
        );
        assert_eq!(last_error(), None);
        unsafe { coppa_engine_destroy(&mut h) };
    }

    #[test]
    fn test_last_error_is_per_thread() {
        let (to_b, from_a) = std::sync::mpsc::channel::<()>();
        let (to_a, from_b) = std::sync::mpsc::channel::<Option<String>>();
        let a = std::thread::spawn(move || {
            unsafe { coppa_engine_set_speed_level(std::ptr::null_mut(), 1) };
            to_b.send(()).unwrap();
            let b_saw = from_b.recv().unwrap();
            (b_saw, last_error())
        });
        from_a.recv().unwrap();
        let b = std::thread::spawn(last_error).join().unwrap();
        to_a.send(b).unwrap();
        let (b_saw, a_still) = a.join().unwrap();
        assert_eq!(b_saw, None);
        assert_eq!(
            a_still.as_deref(),
            Some("coppa_engine_set_speed_level: handle is null")
        );
    }

    #[test]
    fn test_poisoned_handle_message() {
        let mut h = coppa_engine_create();
        let addr = h as usize;
        let _ = std::thread::spawn(move || {
            let h = addr as *mut CoppaHandle;
            let _guard = unsafe { (*h).engine.lock() };
            panic!("poison the engine lock on purpose");
        })
        .join();
        let mut s: *mut f32 = std::ptr::null_mut();
        let mut n: usize = 0;
        let rc = unsafe { coppa_encode_bytes(h, [1u8].as_ptr(), 1, &mut s, &mut n) };
        assert_eq!(rc, COPPA_ERR_POISONED);
        let e = last_error().unwrap();
        assert!(e.contains("poisoned"), "{e}");
        assert!(e.contains("destroy it and create a new one"), "{e}");
        unsafe { coppa_engine_destroy(&mut h) };
    }

    #[test]
    fn test_panic_payload_is_reported() {
        let s: Box<dyn std::any::Any + Send> = Box::new("boom");
        assert_eq!(panic_message(&*s), "boom");
        let s: Box<dyn std::any::Any + Send> = Box::new(String::from("bang"));
        assert_eq!(panic_message(&*s), "bang");
        let s: Box<dyn std::any::Any + Send> = Box::new(42u32);
        assert_eq!(panic_message(&*s), "unknown panic payload");
        assert_eq!(finish("f", Err(Box::new("x"))), COPPA_ERR_PANIC);
        assert_eq!(
            last_error().as_deref(),
            Some("f: internal panic: x; the handle is unusable, destroy it and create a new one")
        );
    }

    #[test]
    fn test_strerror_covers_every_code() {
        unsafe { coppa_engine_set_speed_level(std::ptr::null_mut(), 1) };
        let before = last_error();
        for code in [1, 0, -1, -2, -3, -4, -5, -6, -7] {
            let p = coppa_strerror(code);
            assert!(!p.is_null());
            let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap();
            assert!(!s.is_empty());
            assert_ne!(s, "unknown coppa error code", "code {code}");
        }
        let s = unsafe { CStr::from_ptr(coppa_strerror(-42)) }
            .to_str()
            .unwrap();
        assert_eq!(s, "unknown coppa error code");
        assert_eq!(last_error(), before);
    }

    #[test]
    fn test_error_constants_match_codes() {
        assert_eq!(COPPA_OK, 0);
        assert_eq!(COPPA_ERR_NULL_ARG, -1);
        assert_eq!(COPPA_ERR_INVALID_UTF8, -2);
        assert_eq!(COPPA_ERR_CODEC, -3);
        assert_eq!(COPPA_ERR_POISONED, -4);
        assert_eq!(COPPA_ERR_PANIC, -5);
        assert_eq!(COPPA_ERR_INVALID_PARAM, -6);
        assert_eq!(COPPA_ERR_INVALID_STATE, -7);
        assert_eq!(COPPA_NONE_PENDING, 1);
    }

    #[test]
    fn test_abi_version_matches_constants() {
        assert_eq!(
            coppa_abi_version(),
            (COPPA_ABI_VERSION_MAJOR << 16) | COPPA_ABI_VERSION_MINOR
        );
        assert_eq!(COPPA_ABI_VERSION_MAJOR, 1);
        assert_eq!(COPPA_ABI_VERSION_MINOR, 0);
    }

    #[test]
    fn test_checked_in_header_documents_abi_and_threads() {
        let h = header();
        for needle in [
            "#define COPPA_ABI_VERSION_MAJOR 1",
            "#define COPPA_ABI_VERSION_MINOR 0",
            "coppa_abi_version(void)",
            " * Thread safety",
            "must not",
            "run while any other call on the same handle is in progress",
            "coppa_last_error is per thread",
            "#define COPPA_API",
            "coppa_last_error(void)",
            "coppa_strerror(int32_t code)",
        ] {
            assert!(h.contains(needle), "coppa.h is missing `{needle}`");
        }
    }

    #[test]
    fn test_checked_in_header_marks_v1_stream_deprecated() {
        let h = header();
        let lines: Vec<&str> = h.lines().collect();
        for func in [
            "coppa_start_stream(",
            "coppa_feed_samples(",
            "coppa_get_decoded(",
            "coppa_stop_stream(",
        ] {
            let i = lines
                .iter()
                .position(|l| l.contains(func) && !l.trim_start().starts_with('*'))
                .unwrap_or_else(|| panic!("{func} prototype not found"));
            assert!(
                lines[i - 1].starts_with("COPPA_DEPRECATED("),
                "line before {func} is `{}`",
                lines[i - 1]
            );
        }
    }

    #[test]
    fn test_checked_in_header_has_no_leading_space_prototypes() {
        for line in header().lines() {
            assert!(
                !line.starts_with(" struct ") && !line.starts_with(" int32_t "),
                "leading-space prototype: `{line}`"
            );
        }
    }

    #[test]
    fn test_shared_handle_concurrent_calls() {
        let mut h = coppa_engine_create();
        let addr = h as usize;
        let mut threads = Vec::new();
        for _ in 0..2 {
            threads.push(std::thread::spawn(move || {
                let h = addr as *mut CoppaHandle;
                for _ in 0..20 {
                    let mut s: *mut f32 = std::ptr::null_mut();
                    let mut n: usize = 0;
                    let rc = unsafe { coppa_encode_bytes(h, [7u8; 8].as_ptr(), 8, &mut s, &mut n) };
                    assert_eq!(rc, 0);
                    unsafe { coppa_free_samples(s, n) };
                }
            }));
            threads.push(std::thread::spawn(move || {
                let h = addr as *mut CoppaHandle;
                let silence = [0.0f32; 960];
                for _ in 0..200 {
                    let rc = unsafe { coppa_engine_feed_samples(h, silence.as_ptr(), 960) };
                    assert_eq!(rc, 0);
                }
            }));
            threads.push(std::thread::spawn(move || {
                let h = addr as *mut CoppaHandle;
                for _ in 0..2000 {
                    let mut f = std::mem::MaybeUninit::<CoppaFrame>::uninit();
                    let rc = unsafe { coppa_next_frame(h, f.as_mut_ptr()) };
                    assert!(rc == 0 || rc == 1, "rc {rc}");
                    if rc == 0 {
                        let f = unsafe { f.assume_init() };
                        unsafe { coppa_free_frame_payload(f.payload, f.payload_len) };
                    }
                }
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        unsafe { coppa_engine_destroy(&mut h) };
    }

    // --- links ---

    fn link_cfg(callsign: *const c_char) -> CoppaLinkConfig {
        CoppaLinkConfig {
            struct_size: std::mem::size_of::<CoppaLinkConfig>() as u32,
            callsign,
            profile: std::ptr::null(),
            speed_level: 0,
            turnaround_ms: 0,
        }
    }

    #[test]
    fn test_link_new_rejects_bad_config() {
        assert!(unsafe { coppa_link_new(std::ptr::null()) }.is_null());
        assert_eq!(last_error().as_deref(), Some("coppa_link_new: cfg is null"));

        let mut cfg = link_cfg(c"N0AAA".as_ptr());
        cfg.struct_size = 0;
        assert!(unsafe { coppa_link_new(&cfg) }.is_null());
        let need = std::mem::size_of::<CoppaLinkConfig>();
        assert_eq!(
            last_error(),
            Some(format!("coppa_link_new: struct_size 0 is smaller than this library's coppa_link_config_t ({need} bytes); set it to sizeof(coppa_link_config_t)"))
        );

        let cfg = link_cfg(std::ptr::null());
        assert!(unsafe { coppa_link_new(&cfg) }.is_null());
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_link_new: callsign is null")
        );

        let cfg = link_cfg(c"N0 CALL".as_ptr());
        assert!(unsafe { coppa_link_new(&cfg) }.is_null());
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_link_new: invalid callsign \"N0 CALL\": Invalid callsign character: ' '")
        );

        let mut cfg = link_cfg(c"N0AAA".as_ptr());
        cfg.profile = c"NOPE".as_ptr();
        assert!(unsafe { coppa_link_new(&cfg) }.is_null());
        assert!(last_error()
            .unwrap()
            .starts_with("coppa_link_new: unknown profile \"NOPE\""));

        let mut cfg = link_cfg(c"N0AAA".as_ptr());
        cfg.speed_level = 77;
        assert!(unsafe { coppa_link_new(&cfg) }.is_null());
        assert!(last_error().unwrap().contains("Invalid speed level: 77"));
    }

    #[test]
    fn test_link_state_errors() {
        let cfg = link_cfg(c"N0AAA".as_ptr());
        let mut l = unsafe { coppa_link_new(&cfg) };
        assert!(!l.is_null());
        assert_eq!(
            unsafe { coppa_link_send(l, [1u8].as_ptr(), 1) },
            COPPA_ERR_INVALID_STATE
        );
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_link_send: link is not connected (state IDLE)")
        );
        assert_eq!(
            unsafe { coppa_link_disconnect(l, 0) },
            COPPA_ERR_INVALID_STATE
        );
        assert_eq!(unsafe { coppa_link_state(l) }, COPPA_LINK_STATE_IDLE);
        unsafe { coppa_link_destroy(&mut l) };
        assert!(l.is_null());
    }

    #[test]
    fn test_link_now_ms_must_not_go_backwards() {
        let cfg = link_cfg(c"N0AAA".as_ptr());
        let mut l = unsafe { coppa_link_new(&cfg) };
        let mut tx = [0.0f32; 960];
        let p = |l, t, tx: &mut [f32; 960]| unsafe {
            coppa_link_process(
                l,
                t,
                std::ptr::null(),
                0,
                tx.as_mut_ptr(),
                960,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(p(l, 1000, &mut tx), 0);
        assert_eq!(p(l, 900, &mut tx), COPPA_ERR_INVALID_PARAM);
        assert_eq!(
            last_error().as_deref(),
            Some("coppa_link_process: now_ms 900 is earlier than the previous call's 1000; pass a monotonic clock")
        );
        unsafe { coppa_link_destroy(&mut l) };
    }

    #[test]
    fn test_link_null_args() {
        let n = std::ptr::null_mut::<CoppaLink>();
        let mut len = 0usize;
        let mut ev = std::mem::MaybeUninit::<CoppaLinkEvent>::uninit();
        unsafe {
            let cases: [(&str, i32); 9] = [
                ("coppa_link_listen", coppa_link_listen(n, true)),
                (
                    "coppa_link_connect",
                    coppa_link_connect(n, c"N0BBB".as_ptr(), 0),
                ),
                ("coppa_link_send", coppa_link_send(n, [1u8].as_ptr(), 1)),
                ("coppa_link_disconnect", coppa_link_disconnect(n, 0)),
                (
                    "coppa_link_process",
                    coppa_link_process(
                        n,
                        0,
                        std::ptr::null(),
                        0,
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                    ),
                ),
                (
                    "coppa_link_recv",
                    coppa_link_recv(n, std::ptr::null_mut(), 0, &mut len),
                ),
                (
                    "coppa_link_next_event",
                    coppa_link_next_event(n, ev.as_mut_ptr()),
                ),
                ("coppa_link_state", coppa_link_state(n)),
                ("coppa_link_pending", coppa_link_pending(n, &mut len)),
            ];
            for (func, code) in cases {
                assert_eq!(code, COPPA_ERR_NULL_ARG, "{func}");
            }
            coppa_link_pending(n, &mut len);
            assert_eq!(
                last_error().as_deref(),
                Some("coppa_link_pending: link is null")
            );

            let cfg = link_cfg(c"N0AAA".as_ptr());
            let mut l = coppa_link_new(&cfg);
            let rc = coppa_link_process(
                l,
                0,
                std::ptr::null(),
                5,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
            );
            assert_eq!(rc, COPPA_ERR_NULL_ARG);
            assert_eq!(
                last_error().as_deref(),
                Some("coppa_link_process: rx is null")
            );
            let rc = coppa_link_process(
                l,
                0,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
            );
            assert_eq!(rc, 0);
            coppa_link_destroy(&mut l);
        }
    }

    /// Run two links over a lockstep 960-sample cable through the C ABI only,
    /// until `done` or `max_ticks`. Returns the tick count reached.
    fn ffi_cable(
        a: usize,
        b: usize,
        start_tick: u64,
        max_ticks: u64,
        mut done: impl FnMut(u64) -> bool,
    ) -> u64 {
        let (a, b) = (a as *mut CoppaLink, b as *mut CoppaLink);
        let mut a_out = vec![0.0f32; 960];
        let mut b_out = vec![0.0f32; 960];
        let mut a_next = vec![0.0f32; 960];
        let mut b_next = vec![0.0f32; 960];
        for tick in start_tick..start_tick + max_ticks {
            let now = tick * 20;
            unsafe {
                assert_eq!(
                    coppa_link_process(
                        a,
                        now,
                        b_out.as_ptr(),
                        960,
                        a_next.as_mut_ptr(),
                        960,
                        std::ptr::null_mut()
                    ),
                    0
                );
                assert_eq!(
                    coppa_link_process(
                        b,
                        now,
                        a_out.as_ptr(),
                        960,
                        b_next.as_mut_ptr(),
                        960,
                        std::ptr::null_mut()
                    ),
                    0
                );
            }
            std::mem::swap(&mut a_out, &mut a_next);
            std::mem::swap(&mut b_out, &mut b_next);
            if done(tick) {
                return tick + 1;
            }
        }
        start_tick + max_ticks
    }

    fn next_event(l: *mut CoppaLink) -> Option<(i32, i32, String)> {
        let mut ev = std::mem::MaybeUninit::<CoppaLinkEvent>::uninit();
        let rc = unsafe { coppa_link_next_event(l, ev.as_mut_ptr()) };
        assert!(rc == 0 || rc == 1, "rc {rc}");
        if rc == 1 {
            return None;
        }
        let ev = unsafe { ev.assume_init() };
        let remote = unsafe { CStr::from_ptr(ev.remote.as_ptr()) }
            .to_str()
            .unwrap()
            .to_owned();
        Some((ev.kind, ev.reason, remote))
    }

    #[test]
    fn test_link_pair_through_ffi() {
        let (ca, cb) = (link_cfg(c"N0AAA".as_ptr()), link_cfg(c"N0BBB".as_ptr()));
        let mut a = unsafe { coppa_link_new(&ca) };
        let mut b = unsafe { coppa_link_new(&cb) };
        assert!(!a.is_null() && !b.is_null());
        unsafe {
            assert_eq!(coppa_link_listen(b, true), 0);
            assert_eq!(coppa_link_connect(a, c"N0BBB".as_ptr(), 0), 0);
        }
        let mut t = ffi_cable(a as usize, b as usize, 0, 1500, |_| unsafe {
            coppa_link_state(a) == COPPA_LINK_STATE_CONNECTED
                && coppa_link_state(b) == COPPA_LINK_STATE_CONNECTED
        });
        assert_eq!(
            next_event(a),
            Some((COPPA_LINK_EVENT_CONNECTED, 0, "N0BBB".into()))
        );
        assert_eq!(
            next_event(b),
            Some((COPPA_LINK_EVENT_CONNECTED, 0, "N0AAA".into()))
        );

        let msg = b"hello over a coppa link";
        assert_eq!(msg.len(), 23);
        assert_eq!(unsafe { coppa_link_send(a, msg.as_ptr(), msg.len()) }, 0);
        let mut got = Vec::new();
        t = ffi_cable(a as usize, b as usize, t, 3000, |_| {
            let mut buf = [0u8; 64];
            let mut n = 0usize;
            assert_eq!(
                unsafe { coppa_link_recv(b, buf.as_mut_ptr(), 64, &mut n) },
                0
            );
            got.extend_from_slice(&buf[..n]);
            let mut pending = usize::MAX;
            assert_eq!(unsafe { coppa_link_pending(a, &mut pending) }, 0);
            got.len() == msg.len() && pending == 0
        });
        assert_eq!(got, msg);

        assert_eq!(unsafe { coppa_link_disconnect(a, t * 20) }, 0);
        ffi_cable(a as usize, b as usize, t, 3000, |_| unsafe {
            coppa_link_state(a) == COPPA_LINK_STATE_IDLE
                && coppa_link_state(b) == COPPA_LINK_STATE_IDLE
        });
        assert_eq!(
            next_event(a),
            Some((
                COPPA_LINK_EVENT_DISCONNECTED,
                COPPA_LINK_REASON_LOCAL,
                "N0BBB".into()
            ))
        );
        assert_eq!(
            next_event(b),
            Some((
                COPPA_LINK_EVENT_DISCONNECTED,
                COPPA_LINK_REASON_REMOTE,
                "N0AAA".into()
            ))
        );
        unsafe {
            coppa_link_destroy(&mut a);
            coppa_link_destroy(&mut b);
        }
        assert!(a.is_null() && b.is_null());
    }

    #[test]
    fn test_link_shared_between_threads() {
        let (ca, cb) = (link_cfg(c"N0AAA".as_ptr()), link_cfg(c"N0BBB".as_ptr()));
        let mut a = unsafe { coppa_link_new(&ca) };
        let mut b = unsafe { coppa_link_new(&cb) };
        unsafe {
            assert_eq!(coppa_link_listen(b, true), 0);
            assert_eq!(coppa_link_connect(a, c"N0BBB".as_ptr(), 0), 0);
        }
        let (aa, ba) = (a as usize, b as usize);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        let cable = std::thread::spawn(move || {
            ffi_cable(aa, ba, 0, 6000, |_| {
                stop2.load(std::sync::atomic::Ordering::SeqCst)
            });
        });
        let app = std::thread::spawn(move || {
            let (a, b) = (aa as *mut CoppaLink, ba as *mut CoppaLink);
            let msg = b"threads share a link";
            loop {
                let s = unsafe { coppa_link_state(a) };
                assert!(s >= 0);
                if s == COPPA_LINK_STATE_CONNECTED {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(unsafe { coppa_link_send(a, msg.as_ptr(), msg.len()) }, 0);
            let mut got = Vec::new();
            while got.len() < msg.len() {
                let mut buf = [0u8; 64];
                let mut n = 0usize;
                assert_eq!(
                    unsafe { coppa_link_recv(b, buf.as_mut_ptr(), 64, &mut n) },
                    0
                );
                got.extend_from_slice(&buf[..n]);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            got == msg
        });
        let ok = app.join().unwrap();
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        cable.join().unwrap();
        assert!(ok);
        unsafe {
            coppa_link_destroy(&mut a);
            coppa_link_destroy(&mut b);
        }
    }

    #[test]
    fn test_checked_in_header_contains_link_api() {
        let h = header();
        for needle in [
            "coppa_link_new(",
            "coppa_link_destroy(",
            "coppa_link_listen(",
            "coppa_link_connect(",
            "coppa_link_send(",
            "coppa_link_disconnect(",
            "coppa_link_process(",
            "coppa_link_recv(",
            "coppa_link_next_event(",
            "coppa_link_state(",
            "coppa_link_pending(",
            "coppa_link_config_t",
            "coppa_link_event_t",
            "struct_size",
            "char remote[16]",
            "#define COPPA_LINK_REASON_HANDSHAKE_FAILED 5",
        ] {
            assert!(h.contains(needle), "coppa.h is missing `{needle}`");
        }
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn test_link_config_layout() {
        assert_eq!(std::mem::size_of::<CoppaLinkConfig>(), 32);
        let mut cfg = link_cfg(c"N0AAA".as_ptr());
        cfg.struct_size = 8;
        assert!(unsafe { coppa_link_new(&cfg) }.is_null());
        assert!(last_error().unwrap().contains("(32 bytes)"));
    }
}
