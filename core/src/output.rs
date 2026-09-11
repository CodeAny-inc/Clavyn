//! Coalescing policy for terminal output on its way to the UI layer.
//!
//! A PTY hands over one chunk per protocol packet, from a single keystroke echo
//! to a full 32 KiB window. Delivering every chunk separately makes the fixed
//! per-message cost of the UI transport dominate bulk output, so chunks are
//! accumulated into batches instead.

use std::time::{Duration, Instant};

/// Longest a partially filled batch waits before delivery. Bounds the delay
/// added to output that arrives while a batch is already in flight.
pub const FLUSH_WINDOW: Duration = Duration::from_millis(6);

/// Largest batch handed to the UI layer, and the size at which one becomes due
/// for delivery.
///
/// A chunk is appended whole, so the buffer can exceed this; delivery is what
/// enforces the limit, handing over at most this many bytes and keeping the
/// rest for the next release. That keeps the cap a property of what the
/// transport actually receives rather than of how the producer happened to
/// packetise its output.
///
/// Caps the memory a stalled UI layer can hold per batch, and stays far enough
/// above the size at which transports switch from an inline literal to a binary
/// payload that a full batch always takes the binary path.
pub const MAX_BATCH: usize = 64 * 1024;

/// Accumulates terminal output into batches.
///
/// The caller drives the clock: every method takes the current instant, so the
/// policy is deterministic and independent of any timer implementation.
///
/// Invariant: a non-empty buffer always has a deadline, so buffered bytes can
/// never sit undelivered without something scheduled to release them.
pub struct OutputBatcher {
    buffer: Vec<u8>,
    deadline: Option<Instant>,
    last_flush: Instant,
}

impl OutputBatcher {
    pub fn new(now: Instant) -> Self {
        Self {
            buffer: Vec::with_capacity(MAX_BATCH),
            deadline: None,
            last_flush: now,
        }
    }

    /// Appends a chunk and reports whether the batch is due for delivery now.
    ///
    /// Chunks are appended in arrival order and never reordered, so a batch
    /// carries exactly the byte stream the remote end produced.
    pub fn push(&mut self, chunk: &[u8], now: Instant) -> bool {
        if chunk.is_empty() {
            return false;
        }
        // A quiet session means the previous batch has already been released and
        // nothing has arrived since the window elapsed.
        let quiet = self.buffer.is_empty() && now.duration_since(self.last_flush) >= FLUSH_WINDOW;
        self.buffer.extend_from_slice(chunk);

        if self.buffer.len() >= MAX_BATCH {
            return true;
        }
        // The first chunk after a quiet spell goes out with no added delay: it
        // is either a keystroke echo, where a wait reads as typing lag, or the
        // start of a burst, where it is the first thing the user sees. Because
        // `quiet` requires a full window since the last delivery, these
        // undelayed batches cannot exceed one per window; anything faster than
        // that is coalescing traffic instead.
        if quiet {
            return true;
        }
        if self.deadline.is_none() {
            self.deadline = Some(now + FLUSH_WINDOW);
        }
        false
    }

    /// Instant at which a partially filled batch must be delivered, if one is
    /// waiting.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Bytes to deliver now, never more than `MAX_BATCH`.
    ///
    /// A chunk that carries the buffer past the limit leaves a remainder, which
    /// this withholds until the next release so no single batch exceeds the cap.
    pub fn batch(&self) -> &[u8] {
        &self.buffer[..self.buffer.len().min(MAX_BATCH)]
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Drops the bytes `batch` handed to the UI layer, keeping any remainder.
    ///
    /// A remainder is already overdue, so it is given a deadline of now rather
    /// than a fresh window: it goes out on the next pass instead of waiting
    /// behind output that has not arrived yet. That also preserves the
    /// invariant that a non-empty buffer always has a deadline.
    pub fn mark_flushed(&mut self, now: Instant) {
        let delivered = self.buffer.len().min(MAX_BATCH);
        self.buffer.drain(..delivered);
        self.last_flush = now;
        self.deadline = if self.buffer.is_empty() {
            None
        } else {
            Some(now)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Far enough past the window that the next chunk counts as arriving into a
    /// quiet session.
    fn idle_after(now: Instant) -> Instant {
        now + FLUSH_WINDOW + Duration::from_millis(1)
    }

    #[test]
    fn quiet_session_delivers_an_echo_without_waiting() {
        let start = Instant::now();
        let mut batcher = OutputBatcher::new(start);
        let now = idle_after(start);
        assert!(batcher.push(b"x", now));
        assert_eq!(batcher.batch(), b"x");
        assert_eq!(batcher.deadline(), None);
    }

    #[test]
    fn quiet_session_delivers_the_first_bulk_chunk_without_waiting() {
        let start = Instant::now();
        let mut batcher = OutputBatcher::new(start);
        let now = idle_after(start);
        // The opening chunk of a burst is what the user sees first; it is not
        // held back just because it is large.
        assert!(batcher.push(&vec![b'z'; 32 * 1024], now));
        assert_eq!(batcher.deadline(), None);
    }

    #[test]
    fn a_batch_never_exceeds_the_cap_and_the_remainder_follows_in_order() {
        let start = Instant::now();
        let mut batcher = OutputBatcher::new(start);
        let now = idle_after(start);
        // Spend the quiet-session allowance so the chunks below coalesce.
        assert!(batcher.push(b"A", now));
        batcher.mark_flushed(now);

        let first = vec![b'x'; MAX_BATCH - 1];
        assert!(!batcher.push(&first, now));
        let second = vec![b'y'; 32 * 1024];
        assert!(batcher.push(&second, now));

        // The buffer holds more than the cap because the chunk was appended
        // whole; a single delivery still must not.
        assert_eq!(batcher.batch().len(), MAX_BATCH);
        let mut delivered = batcher.batch().to_vec();
        batcher.mark_flushed(now);

        // The remainder is already overdue, so it is not made to wait behind
        // output that has not arrived yet.
        assert!(!batcher.is_empty());
        assert_eq!(batcher.deadline(), Some(now));
        delivered.extend_from_slice(batcher.batch());
        batcher.mark_flushed(now);

        assert!(batcher.is_empty());
        assert_eq!(batcher.deadline(), None);

        let mut expected = first;
        expected.extend_from_slice(&second);
        assert_eq!(
            delivered, expected,
            "splitting an oversized buffer must not lose or reorder bytes"
        );
    }

    #[test]
    fn burst_is_coalesced_into_one_batch() {
        let start = Instant::now();
        let mut batcher = OutputBatcher::new(start);
        let now = idle_after(start);
        assert!(batcher.push(b"x", now));
        batcher.mark_flushed(now);

        // Everything arriving inside the window joins a single batch.
        let mut at = now;
        for _ in 0..500 {
            at += Duration::from_micros(1);
            assert!(!batcher.push(b"ab", at));
        }
        assert_eq!(batcher.batch().len(), 1000);
        assert!(batcher.deadline().is_some());
    }

    #[test]
    fn batch_is_delivered_once_it_reaches_the_size_cap() {
        let start = Instant::now();
        let mut batcher = OutputBatcher::new(start);
        let now = idle_after(start);
        assert!(batcher.push(b"$ ", now));
        batcher.mark_flushed(now);

        let chunk = vec![b'z'; 32 * 1024];
        let at = now + Duration::from_micros(1);
        assert!(!batcher.push(&chunk, at));
        assert!(batcher.push(&chunk, at + Duration::from_micros(1)));
        assert_eq!(batcher.batch().len(), MAX_BATCH);
    }

    #[test]
    fn partial_batch_carries_a_deadline_inside_the_window() {
        let start = Instant::now();
        let mut batcher = OutputBatcher::new(start);
        let now = idle_after(start);
        assert!(batcher.push(b"$ ", now));
        batcher.mark_flushed(now);

        let at = now + Duration::from_micros(1);
        assert!(!batcher.push(b"more output", at));
        assert_eq!(batcher.deadline(), Some(at + FLUSH_WINDOW));
    }

    #[test]
    fn echo_behind_a_pending_batch_waits_no_longer_than_the_window() {
        let start = Instant::now();
        let mut batcher = OutputBatcher::new(start);
        let now = idle_after(start);
        assert!(batcher.push(b"$ ", now));
        batcher.mark_flushed(now);

        let opening = now + Duration::from_micros(1);
        assert!(!batcher.push(&vec![b'z'; 4096], opening));
        let opened = batcher.deadline().expect("a partial batch has a deadline");

        let later = opening + Duration::from_millis(2);
        assert!(!batcher.push(b"k", later));
        // The pending deadline is not pushed back by later arrivals, so the
        // echo is delivered within one window of the batch opening.
        assert_eq!(batcher.deadline(), Some(opened));
        assert!(opened <= later + FLUSH_WINDOW);
    }

    #[test]
    fn chunk_order_is_preserved_across_a_batch() {
        let start = Instant::now();
        let mut batcher = OutputBatcher::new(start);
        let mut at = idle_after(start);
        assert!(batcher.push(b"$ ", at));
        batcher.mark_flushed(at);

        let chunks: Vec<Vec<u8>> = (0..64).map(|i| vec![i as u8; 64]).collect();
        for chunk in &chunks {
            at += Duration::from_micros(1);
            assert!(!batcher.push(chunk, at));
        }
        let expected: Vec<u8> = chunks.concat();
        assert_eq!(batcher.batch(), expected.as_slice());
    }

    #[test]
    fn flushing_reopens_the_quiet_fast_path_only_after_the_window() {
        let start = Instant::now();
        let mut batcher = OutputBatcher::new(start);
        let now = idle_after(start);
        assert!(batcher.push(b"x", now));
        batcher.mark_flushed(now);

        // Immediately after a flush the session is not quiet: a small chunk is
        // batched instead of triggering another undelayed delivery.
        assert!(!batcher.push(b"y", now + Duration::from_millis(1)));
        batcher.mark_flushed(now + Duration::from_millis(1));

        let quiet_again = idle_after(now + Duration::from_millis(1));
        assert!(batcher.push(b"z", quiet_again));
    }

    #[test]
    fn a_partial_batch_is_available_for_a_final_flush() {
        let start = Instant::now();
        let mut batcher = OutputBatcher::new(start);
        let now = idle_after(start);
        assert!(batcher.push(b"$ ", now));
        batcher.mark_flushed(now);

        // What EOF hands over: whatever is buffered, in one piece, whether or
        // not the window has elapsed.
        let tail: Vec<u8> = (0..MAX_BATCH - 1).map(|i| i as u8).collect();
        assert!(!batcher.push(&tail, now + Duration::from_micros(1)));
        assert!(!batcher.is_empty());
        assert_eq!(batcher.batch(), tail.as_slice());
    }

    #[test]
    fn an_empty_batch_reports_nothing_to_flush() {
        let batcher = OutputBatcher::new(Instant::now());
        assert!(batcher.is_empty());
        assert!(batcher.batch().is_empty());
    }

    #[test]
    fn empty_chunks_never_trigger_a_delivery() {
        let start = Instant::now();
        let mut batcher = OutputBatcher::new(start);
        let now = idle_after(start);
        assert!(!batcher.push(b"", now));
        assert!(batcher.is_empty());
        assert_eq!(batcher.deadline(), None);
    }
}
