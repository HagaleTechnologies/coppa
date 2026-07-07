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
//! `coppa_encode` returns a sample buffer via `out_samples` / `out_len`. The
//! caller **must** preserve the exact `out_len` value and pass it back to
//! [`coppa_free_samples`] — the allocator needs the original length to
//! reconstruct the `Box<[f32]>`. Passing a different length is undefined
//! behavior.

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;

/// Opaque handle to a Coppa engine instance.
pub struct CoppaHandle {
    engine: Mutex<coppa_engine::CoppaCore>,
}

/// Opaque handle to a streaming decode session.
///
/// `coppa_feed_samples` pushes samples directly into the engine's internal
/// `StreamingReceiver` (via `CoppaCore::push_samples`), which owns its own
/// ring/sync-detector/frame-boundary bookkeeping — no buffer growth cap or
/// rescanning is needed here, since the receiver never re-examines samples it's
/// already consumed and bounds its own memory (`2 * max_frame_samples`; see
/// `coppa_protocol::modem::streaming`).
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

// --- Streaming API ---

/// Start a streaming decode session.
///
/// Returns a handle to the stream, or null on failure.
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
                // Frames that failed decompression/UTF-8 conversion are dropped,
                // exactly like a failed batch `decode()` was silently dropped
                // before Task 7's migration to `push_samples`.
                for frame in frames {
                    if let Ok(message) = frame.message {
                        messages.push_back(message);
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
}
