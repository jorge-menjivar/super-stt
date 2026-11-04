// SPDX-License-Identifier: GPL-3.0-only
//! Fixed-capacity interleaved sample ring shared by the synthesis producer and
//! the audio output callback.
//!
//! Capacity is allocated once, at open, and never grows. That is the whole
//! point: the consumer runs on the audio callback thread, where an allocation
//! (or a `free`) risks missing a deadline, and this crate builds with
//! `panic = "abort"`, so a slip there takes the daemon down mid-utterance
//! rather than glitching. Both [`write`](Ring::write) and [`read`](Ring::read)
//! are bounded memcpys over a preallocated buffer with no branches that can
//! panic.
//!
//! Overflow is reported, never absorbed by growing: a producer that outruns the
//! consumer is told how much it failed to write and retries, which keeps
//! backpressure visible instead of turning it into unbounded memory.

/// A single-producer / single-consumer ring of interleaved `f32` samples.
///
/// "Sample" here means one interleaved value, not one frame — a stereo frame is
/// two samples. Callers keep channel bookkeeping; the ring only moves floats.
#[derive(Debug)]
pub struct Ring {
    buf: Box<[f32]>,
    /// Next index to read from.
    read: usize,
    /// Next index to write to.
    write: usize,
    /// Samples currently stored. Tracked explicitly so full and empty are
    /// distinguishable without sacrificing a slot.
    len: usize,
}

impl Ring {
    /// Allocate a ring holding `capacity` interleaved samples.
    ///
    /// # Panics
    /// Panics if `capacity` is zero — a zero-capacity ring can never accept
    /// audio, so it is a construction bug rather than a runtime condition.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "playback ring capacity must be non-zero");
        Self {
            buf: vec![0.0; capacity].into_boxed_slice(),
            read: 0,
            write: 0,
            len: 0,
        }
    }

    /// Total capacity in samples.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Samples available to read.
    #[must_use]
    pub const fn available(&self) -> usize {
        self.len
    }

    /// Free space in samples.
    #[must_use]
    pub fn free(&self) -> usize {
        self.capacity() - self.len
    }

    /// Whether no samples are buffered.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Copy as much of `src` as fits, returning how many samples were written.
    ///
    /// A short write is normal backpressure, not an error: the caller retries
    /// with the remainder once the consumer has drained some.
    pub fn write(&mut self, src: &[f32]) -> usize {
        let n = src.len().min(self.free());
        if n == 0 {
            return 0;
        }
        let cap = self.capacity();
        // At most two copies: up to the end of the buffer, then the wrap.
        let first = n.min(cap - self.write);
        self.buf[self.write..self.write + first].copy_from_slice(&src[..first]);
        let rest = n - first;
        if rest > 0 {
            self.buf[..rest].copy_from_slice(&src[first..n]);
        }
        self.write = (self.write + n) % cap;
        self.len += n;
        n
    }

    /// Fill `dst` from the ring, returning how many samples were read.
    ///
    /// The tail of `dst` beyond the return value is left untouched — the
    /// consumer decides what an underrun sounds like, and this type does not
    /// impose silence.
    pub fn read(&mut self, dst: &mut [f32]) -> usize {
        let n = dst.len().min(self.len);
        if n == 0 {
            return 0;
        }
        let cap = self.capacity();
        let first = n.min(cap - self.read);
        dst[..first].copy_from_slice(&self.buf[self.read..self.read + first]);
        let rest = n - first;
        if rest > 0 {
            dst[first..n].copy_from_slice(&self.buf[..rest]);
        }
        self.read = (self.read + n) % cap;
        self.len -= n;
        n
    }

    /// Drop every buffered sample. Used by cancel, where the point is to stop
    /// making sound now rather than to finish what was queued.
    pub fn clear(&mut self) {
        self.read = 0;
        self.write = 0;
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::Ring;

    #[test]
    fn writes_and_reads_back_in_order() {
        let mut r = Ring::new(8);
        assert_eq!(r.write(&[1.0, 2.0, 3.0]), 3);
        assert_eq!(r.available(), 3);
        let mut out = [0.0; 3];
        assert_eq!(r.read(&mut out), 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
        assert!(r.is_empty());
    }

    /// The wrap is where an off-by-one turns into audible garbage, so drive the
    /// indices past the end several times and check ordering survives.
    #[test]
    fn survives_repeated_wrapping() {
        let mut r = Ring::new(4);
        let mut next = 0.0_f32;
        let mut expect = 0.0_f32;
        for _ in 0..10 {
            let batch = [next, next + 1.0, next + 2.0];
            next += 3.0;
            assert_eq!(r.write(&batch), 3);
            let mut out = [0.0; 3];
            assert_eq!(r.read(&mut out), 3);
            assert_eq!(out, [expect, expect + 1.0, expect + 2.0]);
            expect += 3.0;
        }
    }

    #[test]
    fn a_full_ring_reports_a_short_write() {
        let mut r = Ring::new(4);
        assert_eq!(r.write(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), 4);
        assert_eq!(r.free(), 0);
        assert_eq!(r.write(&[7.0]), 0, "a full ring accepts nothing");
        let mut out = [0.0; 4];
        r.read(&mut out);
        assert_eq!(
            out,
            [1.0, 2.0, 3.0, 4.0],
            "the overflow was dropped, not interleaved"
        );
    }

    /// An underrun must leave the caller's buffer alone past the samples it
    /// actually delivered, so the consumer can tell how much to silence.
    #[test]
    fn a_short_read_leaves_the_rest_of_the_destination_untouched() {
        let mut r = Ring::new(8);
        r.write(&[1.0, 2.0]);
        let mut out = [-99.0; 4];
        assert_eq!(r.read(&mut out), 2);
        assert_eq!(out, [1.0, 2.0, -99.0, -99.0]);
    }

    #[test]
    fn reading_an_empty_ring_yields_nothing() {
        let mut r = Ring::new(4);
        let mut out = [-99.0; 2];
        assert_eq!(r.read(&mut out), 0);
        assert_eq!(out, [-99.0, -99.0]);
    }

    #[test]
    fn clear_drops_everything_and_resets_the_indices() {
        let mut r = Ring::new(4);
        r.write(&[1.0, 2.0, 3.0]);
        let mut sink = [0.0; 2];
        r.read(&mut sink); // advance read past 0 so a reset is observable
        r.clear();
        assert!(r.is_empty());
        assert_eq!(r.free(), 4);
        assert_eq!(r.write(&[9.0, 8.0, 7.0, 6.0]), 4);
        let mut out = [0.0; 4];
        r.read(&mut out);
        assert_eq!(out, [9.0, 8.0, 7.0, 6.0]);
    }

    #[test]
    #[should_panic(expected = "non-zero")]
    fn a_zero_capacity_ring_is_a_construction_bug() {
        let _ = Ring::new(0);
    }
}
