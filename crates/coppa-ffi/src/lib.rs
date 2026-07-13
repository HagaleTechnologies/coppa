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
//! | `-6` | Invalid parameter value (e.g. out-of-range/reserved speed level, unresolvable profile name) — added in FFI v2, see below |
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
//! function's own doc comment.

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;

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
    let result = std::panic::catch_unwind(|| {
        let handle = Box::new(CoppaHandle {
            engine: Mutex::new(coppa_engine::CoppaCore::new()),
            frame_queue: Mutex::new(VecDeque::new()),
            next_seq: Mutex::new(0),
        });
        Box::into_raw(handle)
    });
    result.unwrap_or(std::ptr::null_mut())
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
    if handle.is_null() || message.is_null() || out_samples.is_null() || out_len.is_null() {
        return -1;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let msg = match CStr::from_ptr(message).to_str() {
            Ok(s) => s,
            Err(_) => return -2,
        };

        let engine = match (*handle).engine.lock() {
            Ok(e) => e,
            Err(_) => return -4,
        };

        match engine.encode(msg) {
            Ok(samples) => {
                let boxed = samples.into_boxed_slice();
                let len = boxed.len();
                let ptr = Box::into_raw(boxed) as *mut f32;
                *out_len = len;
                *out_samples = ptr;
                0
            }
            Err(_) => -3,
        }
    }));
    result.unwrap_or(-5)
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
    if handle.is_null() || samples.is_null() || out_message.is_null() || num_samples == 0 {
        return -1;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sample_slice = std::slice::from_raw_parts(samples, num_samples);
        let engine = match (*handle).engine.lock() {
            Ok(e) => e,
            Err(_) => return -4,
        };

        match engine.decode(sample_slice) {
            Ok(message) => {
                // Use CString so coppa_free_string can use CString::from_raw.
                match CString::new(message) {
                    Ok(cstr) => {
                        *out_message = cstr.into_raw();
                        0
                    }
                    // Message contained an interior NUL — treat as encode error.
                    Err(_) => -3,
                }
            }
            Err(_) => -3,
        }
    }));
    result.unwrap_or(-5)
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
/// - `speed_level`: wire speed-level override (1-10; 8 is reserved),
///   applied after `profile` resolution. `0` means "use the resolved
///   profile's/default speed_level" (0 is never itself a valid wire speed
///   level, so it unambiguously means "unset" here too). A non-zero,
///   invalid level (out of 1-10, or the reserved 8) is a config-rejection
///   error.
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
pub struct CoppaConfig {
    pub sample_rate: u32,
    pub profile: *const c_char,
    pub speed_level: u8,
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
pub struct CoppaFrame {
    pub payload: *mut u8,
    pub payload_len: usize,
    pub snr_db: f32,
    pub cfo_hz: f32,
    pub speed_level: u8,
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
unsafe fn resolve_engine_config(cfg: *const CoppaConfig) -> Option<coppa_engine::EngineConfig> {
    let cfg = &*cfg;

    let mut config = if cfg.profile.is_null() {
        coppa_engine::EngineConfig::default()
    } else {
        let name = CStr::from_ptr(cfg.profile).to_str().ok()?;
        if name.is_empty() {
            coppa_engine::EngineConfig::default()
        } else {
            coppa_engine::EngineConfig::from_profile(coppa_engine::profiles::get_profile(name)?)
        }
    };

    if cfg.sample_rate != 0 {
        config.sample_rate = cfg.sample_rate;
    }

    if !cfg.callsign.is_null() {
        // Validated for UTF-8 (fail fast on bad input, matching this file's
        // existing convention for every other string parameter) but
        // intentionally not retained — see `CoppaConfig::callsign`'s doc.
        CStr::from_ptr(cfg.callsign).to_str().ok()?;
    }

    Some(config)
}

/// Construct a new engine handle from a [`CoppaConfig`] (`coppa_config_t`).
///
/// Returns null if `cfg` is null, or for any config-rejection case (see
/// [`resolve_engine_config`] and [`CoppaConfig`]'s field docs): invalid
/// UTF-8 in `profile` or `callsign`, an unrecognized non-empty `profile`
/// name, or a non-zero `speed_level` that isn't a valid wire speed level
/// (1-10; 8 is reserved).
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
    if cfg.is_null() {
        return std::ptr::null_mut();
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let config = match resolve_engine_config(cfg) {
            Some(c) => c,
            None => return std::ptr::null_mut(),
        };
        let mut core = coppa_engine::CoppaCore::with_config(config);
        if (*cfg).speed_level != 0 && core.set_speed_level((*cfg).speed_level).is_err() {
            return std::ptr::null_mut();
        }
        let handle = Box::new(CoppaHandle {
            engine: Mutex::new(core),
            frame_queue: Mutex::new(VecDeque::new()),
            next_seq: Mutex::new(0),
        });
        Box::into_raw(handle)
    }));
    result.unwrap_or(std::ptr::null_mut())
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
/// a valid wire speed level (1-10; 8 is reserved — see
/// `coppa_protocol::modem::speed_level_components`), `-4` if the internal
/// lock is poisoned, or `-5` if an internal panic occurred. **After either
/// of the latter two the handle is permanently broken** — destroy it with
/// `coppa_engine_destroy` and create a new one.
///
/// # Safety
///
/// `handle` must be a valid engine handle.
#[no_mangle]
pub unsafe extern "C" fn coppa_engine_set_speed_level(handle: *mut CoppaHandle, level: u8) -> i32 {
    if handle.is_null() {
        return -1;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut engine = match (*handle).engine.lock() {
            Ok(e) => e,
            Err(_) => return -4,
        };
        match engine.set_speed_level(level) {
            Ok(()) => 0,
            Err(_) => -6,
        }
    }));
    result.unwrap_or(-5)
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
    if handle.is_null() || (data.is_null() && len > 0) || out_samples.is_null() || out_len.is_null()
    {
        return -1;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let data_slice: &[u8] = if len == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(data, len)
        };

        let engine = match (*handle).engine.lock() {
            Ok(e) => e,
            Err(_) => return -4,
        };

        match engine.encode_bytes(data_slice) {
            Ok(samples) => {
                let boxed = samples.into_boxed_slice();
                let blen = boxed.len();
                let ptr = Box::into_raw(boxed) as *mut f32;
                *out_len = blen;
                *out_samples = ptr;
                0
            }
            Err(_) => -3,
        }
    }));
    result.unwrap_or(-5)
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
    if handle.is_null() || samples.is_null() {
        return -1;
    }
    if len == 0 {
        return 0;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sample_slice = std::slice::from_raw_parts(samples, len);

        let mut engine = match (*handle).engine.lock() {
            Ok(e) => e,
            Err(_) => return -4,
        };
        let frames = engine.push_samples(sample_slice);
        drop(engine);

        if !frames.is_empty() {
            let mut queue = match (*handle).frame_queue.lock() {
                Ok(q) => q,
                Err(_) => return -4,
            };
            let mut next_seq = match (*handle).next_seq.lock() {
                Ok(s) => s,
                Err(_) => return -4,
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

        0
    }));
    result.unwrap_or(-5)
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
    if handle.is_null() || out.is_null() {
        return -1;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut queue = match (*handle).frame_queue.lock() {
            Ok(q) => q,
            Err(_) => return -4,
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
                0
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
                1
            }
        }
    }));
    result.unwrap_or(-5)
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
pub extern "C" fn coppa_start_stream() -> *mut CoppaStreamHandle {
    let result = std::panic::catch_unwind(|| {
        let handle = Box::new(CoppaStreamHandle {
            engine: Mutex::new(coppa_engine::CoppaCore::new()),
            decoded_messages: Mutex::new(VecDeque::new()),
        });
        Box::into_raw(handle)
    });
    result.unwrap_or(std::ptr::null_mut())
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
pub unsafe extern "C" fn coppa_feed_samples(
    handle: *mut CoppaStreamHandle,
    samples: *const f32,
    num_samples: usize,
) -> i32 {
    if handle.is_null() || samples.is_null() {
        return -1;
    }
    if num_samples == 0 {
        return 0;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sample_slice = std::slice::from_raw_parts(samples, num_samples);

        let mut engine = match (*handle).engine.lock() {
            Ok(e) => e,
            Err(_) => return -4,
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

        0
    }));
    result.unwrap_or(-5)
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
pub unsafe extern "C" fn coppa_get_decoded(handle: *mut CoppaStreamHandle) -> *mut c_char {
    if handle.is_null() {
        return std::ptr::null_mut();
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut messages = match (*handle).decoded_messages.lock() {
            Ok(m) => m,
            Err(_) => return std::ptr::null_mut(),
        };

        match messages.pop_front() {
            Some(message) => {
                // Use CString so coppa_free_string can use CString::from_raw.
                match CString::new(message) {
                    Ok(cstr) => cstr.into_raw(),
                    Err(_) => std::ptr::null_mut(),
                }
            }
            None => std::ptr::null_mut(),
        }
    }));
    result.unwrap_or(std::ptr::null_mut())
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
pub unsafe extern "C" fn coppa_stop_stream(handle: *mut CoppaStreamHandle) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
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
        let speed_level = unsafe { (*handle).engine.lock().unwrap().config().speed_level };
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
        let speed_level = unsafe { (*handle).engine.lock().unwrap().config().speed_level };
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
        let speed_level = unsafe { (*handle).engine.lock().unwrap().config().speed_level };
        assert_eq!(
            speed_level, 3,
            "explicit speed_level should override the profile's own"
        );
        unsafe { coppa_engine_destroy(&mut handle) };
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
    fn test_new_with_rejects_reserved_speed_level_8() {
        let cfg = CoppaConfig {
            speed_level: 8,
            ..null_config()
        };
        let handle = unsafe { coppa_engine_new_with(&cfg) };
        assert!(
            handle.is_null(),
            "speed level 8 is reserved and must be rejected"
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
        let sample_rate = unsafe { (*handle).engine.lock().unwrap().config().sample_rate };
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
        let speed_level = unsafe { (*handle).engine.lock().unwrap().config().speed_level };
        assert_eq!(speed_level, 4);
        unsafe { coppa_engine_destroy(&mut handle) };
    }

    #[test]
    fn test_set_speed_level_rejects_reserved_and_out_of_range() {
        let mut handle = coppa_engine_create();
        assert_eq!(unsafe { coppa_engine_set_speed_level(handle, 8) }, -6);
        assert_eq!(unsafe { coppa_engine_set_speed_level(handle, 0) }, -6);
        assert_eq!(unsafe { coppa_engine_set_speed_level(handle, 255) }, -6);
        // A rejected level must not have mutated the config.
        let speed_level = unsafe { (*handle).engine.lock().unwrap().config().speed_level };
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
}
