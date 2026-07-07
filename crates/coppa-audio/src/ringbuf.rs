//! Lock-free SPSC ring buffer for audio sample transfer.

use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Producer side of an audio ring buffer.
pub struct AudioRingProducer {
    inner: Producer<f32>,
    overflow_count: Arc<AtomicU64>,
}

/// Consumer side of an audio ring buffer.
pub struct AudioRingConsumer {
    inner: Consumer<f32>,
    /// Shared with the producer, so the consuming side (e.g. the daemon's
    /// `audio_in`, which only ever holds the consumer half) can also observe
    /// overflow on the producer side that's writing into this ring — added for
    /// Task 7's streaming migration, which checks this each poll and logs a
    /// warning when it grows (silent RX sample loss was a Phase-0-era finding).
    overflow_count: Arc<AtomicU64>,
}

/// Create a split audio ring buffer with the given capacity in samples.
pub fn audio_ring(capacity: usize) -> (AudioRingProducer, AudioRingConsumer) {
    let (producer, consumer) = RingBuffer::new(capacity);
    let overflow_count = Arc::new(AtomicU64::new(0));
    (
        AudioRingProducer {
            inner: producer,
            overflow_count: overflow_count.clone(),
        },
        AudioRingConsumer {
            inner: consumer,
            overflow_count,
        },
    )
}

impl AudioRingProducer {
    /// Write samples into the ring buffer, returning how many were written.
    /// Non-blocking: drops samples if buffer is full.
    pub fn write(&mut self, samples: &[f32]) -> usize {
        let available = self.inner.slots();
        let to_write = samples.len().min(available);
        let dropped = samples.len() - to_write;
        if dropped > 0 {
            self.overflow_count
                .fetch_add(dropped as u64, Ordering::Relaxed);
        }
        for &sample in &samples[..to_write] {
            let _ = self.inner.push(sample);
        }
        to_write
    }

    /// Returns the total number of samples dropped due to buffer overflow.
    pub fn overflow_count(&self) -> u64 {
        self.overflow_count.load(Ordering::Relaxed)
    }

    /// Number of slots available for writing.
    pub fn available(&self) -> usize {
        self.inner.slots()
    }

    /// Returns true if the consumer has been dropped.
    pub fn is_abandoned(&self) -> bool {
        self.inner.is_abandoned()
    }
}

impl AudioRingConsumer {
    /// Read samples from the ring buffer into the provided slice.
    /// Returns the number of samples actually read (may be less than buf.len()).
    pub fn read(&mut self, buf: &mut [f32]) -> usize {
        let available = self.inner.slots();
        let to_read = buf.len().min(available);
        for sample in &mut buf[..to_read] {
            *sample = self.inner.pop().unwrap_or_default();
        }
        to_read
    }

    /// Number of samples available for reading.
    pub fn available(&self) -> usize {
        self.inner.slots()
    }

    /// Discard all buffered samples.
    pub fn drain(&mut self) {
        while self.inner.pop().is_ok() {}
    }

    /// Returns true if the producer has been dropped.
    pub fn is_abandoned(&self) -> bool {
        self.inner.is_abandoned()
    }

    /// Returns the total number of samples dropped on the producer side of this
    /// ring due to buffer overflow (see `AudioRingProducer::overflow_count`).
    pub fn overflow_count(&self) -> u64 {
        self.overflow_count.load(Ordering::Relaxed)
    }
}

// Safety: rtrb types are Send
unsafe impl Send for AudioRingProducer {}
unsafe impl Send for AudioRingConsumer {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_write_read() {
        let (mut tx, mut rx) = audio_ring(1024);
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let written = tx.write(&data);
        assert_eq!(written, 5);

        let mut buf = vec![0.0f32; 5];
        let read = rx.read(&mut buf);
        assert_eq!(read, 5);
        assert_eq!(buf, data);
    }

    #[test]
    fn test_ring_partial_read() {
        let (mut tx, mut rx) = audio_ring(1024);
        tx.write(&[1.0, 2.0, 3.0]);

        let mut buf = vec![0.0f32; 10];
        let read = rx.read(&mut buf);
        assert_eq!(read, 3);
        assert_eq!(&buf[..3], &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_ring_overflow() {
        let (mut tx, _rx) = audio_ring(4);
        let data = vec![1.0; 10];
        let written = tx.write(&data);
        assert!(written <= 4);
        // G1: overflow counter should track dropped samples
        let dropped = 10 - written;
        assert_eq!(tx.overflow_count(), dropped as u64);
    }

    #[test]
    fn test_ring_overflow_counter_accumulates() {
        let (mut tx, _rx) = audio_ring(4);
        tx.write(&[1.0; 10]);
        let first = tx.overflow_count();
        assert!(first > 0);
        tx.write(&[1.0; 10]);
        assert!(
            tx.overflow_count() > first,
            "overflow counter should accumulate"
        );
    }

    #[test]
    fn test_ring_no_overflow() {
        let (mut tx, _rx) = audio_ring(1024);
        tx.write(&[1.0, 2.0, 3.0]);
        assert_eq!(tx.overflow_count(), 0, "no overflow should mean count is 0");
    }

    #[test]
    fn test_ring_drain() {
        let (mut tx, mut rx) = audio_ring(1024);
        tx.write(&[1.0, 2.0, 3.0]);
        rx.drain();
        assert_eq!(rx.available(), 0);
    }

    #[test]
    fn test_ring_abandoned() {
        let (tx, rx) = audio_ring(1024);
        drop(rx);
        assert!(tx.is_abandoned());
    }
}
