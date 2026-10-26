//! Timestamp-addressed stereo audio storage; capture clients do not share a cursor.
use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

const EMPTY: u64 = u64::MAX;
const WRITING: u64 = u64::MAX - 1;

struct Frame {
    time: AtomicU64,
    samples: AtomicU64,
}
impl Frame {
    const fn new() -> Self {
        Self {
            time: AtomicU64::new(EMPTY),
            samples: AtomicU64::new(0),
        }
    }
}

pub struct Ring<const N: usize> {
    frames: [Frame; N],
}
impl<const N: usize> Ring<N> {
    pub const fn new() -> Self {
        Self {
            frames: [const { Frame::new() }; N],
        }
    }

    /// Only called in the stopped-to-started control transition, before host IO starts.
    pub fn clear(&self) {
        for frame in &self.frames {
            frame.time.store(EMPTY, SeqCst);
        }
    }

    /// One bounded CAS publishes a frame; competing writers drop the contested frame.
    /// There is no mutex, waiting, retry loop, allocation, or shared capture cursor.
    /// Both channels are transferred by one atomic word, so stereo never tears.
    pub fn write(&self, time: u64, left: f32, right: f32) -> bool {
        if N == 0 || time >= WRITING {
            return false;
        }
        let frame = &self.frames[time as usize % N];
        let old = frame.time.load(SeqCst);
        if old == WRITING || (old != EMPTY && old >= time) {
            return false;
        }
        if frame
            .time
            .compare_exchange(old, WRITING, SeqCst, SeqCst)
            .is_err()
        {
            return false;
        }
        let pair = u64::from(left.to_bits()) | (u64::from(right.to_bits()) << 32);
        frame.samples.store(pair, SeqCst);
        frame.time.store(time, SeqCst);
        true
    }

    pub fn read(&self, time: u64) -> [f32; 2] {
        if N == 0 || time >= WRITING {
            return [0.0; 2];
        }
        let frame = &self.frames[time as usize % N];
        if frame.time.load(SeqCst) != time {
            return [0.0; 2];
        }
        let pair = frame.samples.load(SeqCst);
        if frame.time.load(SeqCst) != time {
            return [0.0; 2];
        }
        [
            f32::from_bits(pair as u32),
            f32::from_bits((pair >> 32) as u32),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrap_missing_overwrite_and_independent_readers() {
        let ring = Ring::<4>::new();
        assert_eq!(ring.read(0), [0.0; 2]);
        assert!(ring.write(0, 0.25, -0.5));
        assert_eq!(ring.read(0), [0.25, -0.5]);
        assert_eq!(ring.read(0), [0.25, -0.5]);
        assert!(ring.write(4, 0.75, -1.0));
        assert_eq!(ring.read(0), [0.0; 2]);
        assert_eq!(ring.read(4), [0.75, -1.0]);
        assert!(!ring.write(0, 99.0, 99.0));
        ring.clear();
        assert_eq!(ring.read(4), [0.0; 2]);
    }
    #[test]
    fn concurrent_overwrite_never_returns_another_frames_audio() {
        let ring = Ring::<32>::new();
        std::thread::scope(|scope| {
            for producer in 0..2 {
                let ring = &ring;
                scope.spawn(move || {
                    for i in 1..50_000u64 {
                        let t = i * 2 + producer;
                        ring.write(t, t as f32, -(t as f32));
                    }
                });
            }
            for _ in 0..3 {
                let ring = &ring;
                scope.spawn(move || {
                    for time in 1..100_000u64 {
                        let result = ring.read(time);
                        assert!(result == [0.0; 2] || result == [time as f32, -(time as f32)]);
                    }
                });
            }
        });
    }
}
