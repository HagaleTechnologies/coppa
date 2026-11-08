//! COP-121: application records on a session's stream 1 (SPEC §23).
//!
//! When both stations negotiate `app_messages` (CONNECT flags bit 4), the
//! transport `session_id` nibble of a session `Reliable` segment names its
//! stream: [`HOST_STREAM`] carries host bytes exactly as before, and
//! [`APP_STREAM`] carries length-prefixed [`AppPdu`] records,
//! `[length: u16 BE][AppPdu]`. Records may span segments, so a receiver feeds
//! each delivered stream 1 payload, in sequence order, to a [`RecordReader`].
//!
//! A sender tracks where each message ends in the stream with a
//! [`DeliveryTracker`]: a message is delivered when the ARQ ACK for the
//! segment holding its last byte arrives. There is no receipt frame.
//!
//! No clock and no I/O, so this module builds for `wasm32-unknown-unknown`.

use std::collections::VecDeque;

use crate::app::{AppPdu, APP_HEADER_SIZE};
use crate::error::ProtocolError;

/// Stream 0: host bytes, the only stream a session without `app_messages` has.
pub const HOST_STREAM: u8 = 0;
/// Stream 1: application records (SPEC §23).
pub const APP_STREAM: u8 = 1;
/// Bytes of the record length prefix.
pub const RECORD_HEADER_SIZE: usize = 2;
/// Longest chat message, in UTF-8 bytes.
pub const CHAT_MAX_BYTES: usize = 512;
/// Most chat messages a session holds unacknowledged before refusing more.
pub const CHAT_MAX_WAITING: usize = 32;

/// Refusal and failure reasons a station reports for a chat message
/// (`reason` of a failed `chat_receipt`, or a WebSocket `error`).
pub mod chat_reason {
    /// No session is established.
    pub const NO_SESSION: &str = "No session: call a station first";
    /// The session is draining for a disconnect.
    pub const SESSION_CLOSING: &str = "The session is closing";
    /// The session did not negotiate `app_messages`.
    pub const PEER_WITHOUT_CHAT: &str = "The peer's coppad does not accept chat: it needs coppad 0.11.0 or later with its WebSocket API enabled";
    /// The session ended with the message unacknowledged.
    pub const SESSION_ENDED: &str = "The session ended before the peer acknowledged the message";
    /// [`super::CHAT_MAX_WAITING`] messages are already unacknowledged.
    pub const TOO_MANY_WAITING: &str = "Too many messages are waiting to be delivered (32)";
}

/// Check a chat message: 1 to [`CHAT_MAX_BYTES`] UTF-8 bytes with at least one
/// non-whitespace character.
///
/// # Errors
///
/// [`ProtocolError::ChatEmpty`] when the text has no non-whitespace character,
/// [`ProtocolError::ChatTooLong`] when it is longer than the limit.
pub fn check_chat_text(text: &str) -> Result<(), ProtocolError> {
    if text.trim().is_empty() {
        return Err(ProtocolError::ChatEmpty);
    }
    if text.len() > CHAT_MAX_BYTES {
        return Err(ProtocolError::ChatTooLong {
            len: text.len(),
            max: CHAT_MAX_BYTES,
        });
    }
    Ok(())
}

/// Encode one record: `[length: u16 BE][AppPdu]`.
///
/// # Errors
///
/// [`ProtocolError::AppRecordTooLong`] when the encoded `AppPdu` is longer
/// than 65,535 bytes.
pub fn encode_record(pdu: &AppPdu) -> Result<Vec<u8>, ProtocolError> {
    let body = pdu.to_bytes();
    let len = u16::try_from(body.len())
        .map_err(|_| ProtocolError::AppRecordTooLong { len: body.len() })?;
    let mut out = Vec::with_capacity(RECORD_HEADER_SIZE + body.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Encode a chat message as a complete `AppProtocol::Text` record.
///
/// # Errors
///
/// The [`check_chat_text`] errors.
pub fn chat_record(text: &str) -> Result<Vec<u8>, ProtocolError> {
    check_chat_text(text)?;
    encode_record(&AppPdu::new_text(text.as_bytes()))
}

/// Splits a stream 1 byte stream back into records.
///
/// A record whose length is below the `AppPdu` header size makes the rest of
/// the stream unreadable: the reader returns one
/// [`ProtocolError::AppRecordTooShort`] and then drops everything it is given
/// ([`RecordReader::is_broken`]).
#[derive(Debug, Default)]
pub struct RecordReader {
    buf: Vec<u8>,
    broken: bool,
}

impl RecordReader {
    /// Append stream bytes and return every record they complete, in order.
    /// A record with an unknown protocol id comes back as an `Err` and the
    /// reader continues with the next record.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Result<AppPdu, ProtocolError>> {
        let mut out = Vec::new();
        if self.broken {
            return out;
        }
        self.buf.extend_from_slice(bytes);
        let mut start = 0;
        while self.buf.len() - start >= RECORD_HEADER_SIZE {
            let len = u16::from_be_bytes([self.buf[start], self.buf[start + 1]]);
            if usize::from(len) < APP_HEADER_SIZE {
                out.push(Err(ProtocolError::AppRecordTooShort { len }));
                self.broken = true;
                self.buf.clear();
                return out;
            }
            let end = start + RECORD_HEADER_SIZE + usize::from(len);
            if self.buf.len() < end {
                break;
            }
            out.push(AppPdu::from_bytes(
                &self.buf[start + RECORD_HEADER_SIZE..end],
            ));
            start = end;
        }
        self.buf.drain(..start);
        out
    }

    /// True once a too-short length made the stream unreadable.
    pub fn is_broken(&self) -> bool {
        self.broken
    }
}

/// Tracks, on the sending side, which queued messages the peer's ARQ ACKs
/// have fully covered.
#[derive(Debug, Default)]
pub struct DeliveryTracker {
    queued: u64,
    acked: u64,
    pending: VecDeque<(u64, u64)>,
}

impl DeliveryTracker {
    /// Record that message `id`'s record, `record_len` bytes long, was
    /// appended to the stream after everything queued before it.
    pub fn queue(&mut self, id: u64, record_len: usize) {
        self.queued = self.queued.saturating_add(record_len as u64);
        self.pending.push_back((id, self.queued));
    }

    /// Account for an ACK that covered `bytes` more stream bytes, and return
    /// the ids whose last byte is now acknowledged, in queue order.
    pub fn ack(&mut self, bytes: usize) -> Vec<u64> {
        self.acked = self.acked.saturating_add(bytes as u64).min(self.queued);
        let mut done = Vec::new();
        while let Some(&(id, end)) = self.pending.front() {
            if end > self.acked {
                break;
            }
            done.push(id);
            self.pending.pop_front();
        }
        done
    }

    /// Return every unacknowledged id, in queue order, and forget them and
    /// the stream position (the session ended).
    pub fn fail_all(&mut self) -> Vec<u64> {
        let ids = self.pending.drain(..).map(|(id, _)| id).collect();
        self.queued = 0;
        self.acked = 0;
        ids
    }

    /// Number of messages not yet acknowledged.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppProtocol;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn chat_record_frames_text_as_a_complete_text_app_pdu() {
        let record = chat_record("73").unwrap();
        assert_eq!(hex(&record), "0006000000013733");
    }

    #[test]
    fn chat_text_must_have_a_non_whitespace_character() {
        for text in ["", "  \n", "\t"] {
            let err = check_chat_text(text).unwrap_err();
            assert!(matches!(err, ProtocolError::ChatEmpty));
            assert_eq!(err.to_string(), "Message is empty");
            assert!(chat_record(text).is_err());
        }
    }

    #[test]
    fn chat_text_is_limited_in_utf8_bytes_not_chars() {
        let max = "é".repeat(256);
        assert_eq!(max.len(), 512);
        assert!(check_chat_text(&max).is_ok());
        let over = format!("{max}a");
        let err = check_chat_text(&over).unwrap_err();
        assert!(matches!(
            err,
            ProtocolError::ChatTooLong { len: 513, max: 512 }
        ));
        assert_eq!(err.to_string(), "Message is 513 bytes; the limit is 512");
    }

    #[test]
    fn encode_record_refuses_a_pdu_longer_than_u16() {
        let pdu = AppPdu::new_text(&vec![b'a'; 65_536 - APP_HEADER_SIZE]);
        let err = encode_record(&pdu).unwrap_err();
        assert!(matches!(
            err,
            ProtocolError::AppRecordTooLong { len: 65_536 }
        ));
        let fits = AppPdu::new_text(&vec![b'a'; 65_535 - APP_HEADER_SIZE]);
        assert_eq!(encode_record(&fits).unwrap().len(), 65_537);
    }

    #[test]
    fn reader_reassembles_a_record_pushed_one_byte_at_a_time() {
        let record = chat_record("hello").unwrap();
        let mut reader = RecordReader::default();
        for (i, byte) in record.iter().enumerate() {
            let out = reader.push(std::slice::from_ref(byte));
            if i + 1 < record.len() {
                assert!(out.is_empty(), "record complete early at byte {i}");
            } else {
                assert_eq!(out.len(), 1);
                let pdu = out[0].as_ref().unwrap();
                assert_eq!(pdu.protocol, AppProtocol::Text);
                assert_eq!(pdu.payload, b"hello");
            }
        }
    }

    #[test]
    fn reader_returns_every_record_of_one_push_in_order() {
        let mut stream = chat_record("one").unwrap();
        stream.extend(chat_record("two").unwrap());
        stream.extend(chat_record("three").unwrap());
        let mut reader = RecordReader::default();
        let out = reader.push(&stream);
        let texts: Vec<Vec<u8>> = out.into_iter().map(|r| r.unwrap().payload).collect();
        assert_eq!(
            texts,
            vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
        );
    }

    #[test]
    fn reader_skips_a_record_with_an_unknown_protocol_and_continues() {
        // Protocol nibble 9 is not an AppProtocol.
        let mut stream = vec![0x00, 0x05, 0x90, 0x00, 0x00, 0x01, b'x'];
        stream.extend(chat_record("ok").unwrap());
        let mut reader = RecordReader::default();
        let out = reader.push(&stream);
        assert_eq!(out.len(), 2);
        assert!(matches!(
            out[0],
            Err(ProtocolError::UnknownAppProtocol { value: 9 })
        ));
        assert_eq!(out[1].as_ref().unwrap().payload, b"ok");
        assert!(!reader.is_broken());
    }

    #[test]
    fn reader_stops_after_a_length_below_the_app_header() {
        let mut reader = RecordReader::default();
        let out = reader.push(&[0x00, 0x03, 0x00, 0x00, 0x00]);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            Err(ProtocolError::AppRecordTooShort { len: 3 })
        ));
        assert!(reader.is_broken());
        assert!(reader.push(&chat_record("later").unwrap()).is_empty());
    }

    #[test]
    fn tracker_releases_an_id_when_its_last_byte_is_acked() {
        let record = chat_record("73").unwrap();
        let mut tracker = DeliveryTracker::default();
        tracker.queue(7, record.len());
        assert_eq!(tracker.ack(5), Vec::<u64>::new());
        assert_eq!(tracker.pending(), 1);
        assert_eq!(tracker.ack(3), vec![7]);
        assert_eq!(tracker.pending(), 0);
    }

    #[test]
    fn tracker_releases_several_ids_on_one_ack() {
        let mut tracker = DeliveryTracker::default();
        tracker.queue(1, 8);
        tracker.queue(2, 9);
        tracker.queue(3, 10);
        assert_eq!(tracker.ack(17), vec![1, 2]);
        assert_eq!(tracker.ack(10), vec![3]);
    }

    #[test]
    fn tracker_fail_all_returns_only_unacked_ids_in_queue_order() {
        let mut tracker = DeliveryTracker::default();
        tracker.queue(4, 8);
        tracker.queue(5, 8);
        tracker.queue(6, 8);
        assert_eq!(tracker.ack(8), vec![4]);
        assert_eq!(tracker.fail_all(), vec![5, 6]);
        assert_eq!(tracker.pending(), 0);
        assert!(tracker.fail_all().is_empty());
    }

    #[test]
    fn too_many_waiting_reason_names_the_limit() {
        assert!(chat_reason::TOO_MANY_WAITING.contains(&format!("({CHAT_MAX_WAITING})")));
    }

    #[test]
    fn tracker_ignores_acks_beyond_what_was_queued() {
        let mut tracker = DeliveryTracker::default();
        assert!(tracker.ack(100).is_empty());
        tracker.queue(1, 8);
        assert_eq!(tracker.ack(1_000), vec![1]);
        assert_eq!(tracker.pending(), 0);
        assert!(tracker.ack(usize::MAX).is_empty());
    }
}
