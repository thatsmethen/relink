//! Single-producer/single-consumer asynchronous stereo transport.
//!
//! Slots are atomic packed stereo frames. The producer publishes the write
//! cursor only after a complete callback; it never overwrites unread history.
//! The consumer releases history only after filtering it. This uses no unsafe
//! Rust, callback allocation, locks, retry loops, or competing cursor writers.
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};

pub(crate) const CAPACITY: usize = 65536;
const TAPS: usize = 128;
const HALF: usize = TAPS / 2;
const PHASES: usize = 1024;
pub(crate) const MAX_CORRECTION: f64 = 0.002; // +/- 2,000 ppm; nominal rates must match.

pub(crate) struct Ring {
    frames: Box<[AtomicU64]>,
    written: AtomicU64,
    released: AtomicU64,
    largest_packet: AtomicUsize,
    pub(crate) captured: AtomicU64,
    pub(crate) buffered: AtomicU64,
    pub(crate) correction_ppm: AtomicU64,
    #[cfg(any(target_os = "macos", test))]
    pub(crate) native_timing: super::native_packet::Evidence,
}
impl Ring {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            frames: (0..CAPACITY).map(|_| AtomicU64::new(0)).collect(),
            written: AtomicU64::new(0),
            released: AtomicU64::new(0),
            largest_packet: AtomicUsize::new(0),
            captured: AtomicU64::new(0),
            buffered: AtomicU64::new(0),
            correction_ppm: AtomicU64::new(0),
            #[cfg(any(target_os = "macos", test))]
            native_timing: super::native_packet::Evidence::new(),
        })
    }
    pub(crate) fn bytes() -> usize {
        CAPACITY * std::mem::size_of::<AtomicU64>() + std::mem::size_of::<Self>()
    }
    #[cfg(test)]
    pub(crate) fn test_sample(&self, frame: u64) -> [f32; 2] {
        self.sample(frame)
    }
    fn sample(&self, frame: u64) -> [f32; 2] {
        let bits = self.frames[frame as usize & (CAPACITY - 1)].load(Ordering::Relaxed);
        [
            f32::from_bits(bits as u32),
            f32::from_bits((bits >> 32) as u32),
        ]
    }
}

pub(crate) struct Producer {
    pub(crate) ring: Arc<Ring>,
    cursor: u64,
}
impl Producer {
    pub(crate) fn new(ring: Arc<Ring>) -> Self {
        Self { ring, cursor: 0 }
    }
    #[cfg(any(not(any(target_os = "windows", target_os = "macos")), test))]
    pub(crate) fn push<T: cpal::Sample>(
        &mut self,
        input: &[T],
        channels: usize,
    ) -> Result<(), TransportError>
    where
        f32: cpal::FromSample<T>,
    {
        if channels == 0 || !input.len().is_multiple_of(channels) {
            return Err(TransportError::InvalidBuffer);
        }
        let frames = input.len() / channels;
        self.push_frames(frames, |offset| {
            let frame = &input[offset * channels..(offset + 1) * channels];
            let convert = |x: T| <f32 as cpal::FromSample<T>>::from_sample_(x);
            [convert(frame[0]), convert(frame[usize::from(channels > 1)])]
        })
    }
    pub(crate) fn push_frames(
        &mut self,
        frames: usize,
        mut read: impl FnMut(usize) -> [f32; 2],
    ) -> Result<(), TransportError> {
        if frames > super::MAX_CALLBACK_FRAMES {
            return Err(TransportError::InvalidBuffer);
        }
        let released = self.ring.released.load(Ordering::Acquire);
        let end = self
            .cursor
            .checked_add(frames as u64)
            .ok_or(TransportError::Overflow)?;
        if end - released > CAPACITY as u64 {
            return Err(TransportError::Overflow);
        }
        for offset in 0..frames {
            let frame = read(offset);
            let sanitize = |x: f32| if x.is_finite() { x } else { 0.0 };
            let bits = u64::from(sanitize(frame[0]).to_bits())
                | (u64::from(sanitize(frame[1]).to_bits()) << 32);
            self.ring.frames[(self.cursor as usize + offset) & (CAPACITY - 1)]
                .store(bits, Ordering::Relaxed);
        }
        self.cursor = end;
        self.ring
            .largest_packet
            .fetch_max(frames, Ordering::Relaxed);
        self.ring.written.store(end, Ordering::Release);
        self.ring
            .captured
            .fetch_add(frames as u64, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransportError {
    InvalidBuffer,
    Underflow,
    Overflow,
}

/// Original Blackman-windowed sinc table. Adjacent phase rows are interpolated,
/// avoiding phase quantization steps; rows have unity DC gain. Cutoff is 0.94
/// of Nyquist, retaining anti-alias margin at the bounded clock correction.
/// Constructed only on the control thread and shared by all capture streams.
pub(crate) struct Kernel {
    rows: Box<[[f32; TAPS]]>,
}
impl Kernel {
    pub(crate) const fn taps() -> usize {
        TAPS
    }
    pub(crate) const fn half() -> usize {
        HALF
    }
    /// Borrow adjacent rows for the independent output reader. Capture keeps
    /// its existing arithmetic and priming policy unchanged.
    pub(crate) fn phase_rows(&self, fraction: f64) -> (&[f32; TAPS], &[f32; TAPS], f32) {
        let phase = fraction * PHASES as f64;
        let row = phase.floor() as usize;
        (
            &self.rows[row],
            &self.rows[row + 1],
            (phase - row as f64) as f32,
        )
    }
    pub(crate) fn new() -> Arc<Self> {
        let mut rows = Vec::with_capacity(PHASES + 1);
        for phase in 0..=PHASES {
            let fraction = phase as f64 / PHASES as f64;
            let mut row = [0.0; TAPS];
            let mut sum = 0.0;
            for (tap, coefficient) in row.iter_mut().enumerate() {
                let distance = tap as f64 - (HALF - 1) as f64 - fraction;
                let sinc = if distance.abs() < 1e-12 {
                    0.94
                } else {
                    (std::f64::consts::PI * 0.94 * distance).sin()
                        / (std::f64::consts::PI * distance)
                };
                let angle = std::f64::consts::PI * distance / HALF as f64;
                let window = 0.42 + 0.5 * angle.cos() + 0.08 * (2.0 * angle).cos();
                *coefficient = (sinc * window) as f32;
                sum += f64::from(*coefficient);
            }
            for c in &mut row {
                *c = (f64::from(*c) / sum) as f32;
            }
            rows.push(row);
        }
        Arc::new(Self {
            rows: rows.into_boxed_slice(),
        })
    }
    pub(crate) fn bytes() -> usize {
        (PHASES + 1) * TAPS * std::mem::size_of::<f32>()
    }
}

pub(crate) struct Consumer {
    pub(crate) ring: Arc<Ring>,
    kernel: Arc<Kernel>,
    sample_rate: f64,
    position: u64,
    fraction: f64,
    ratio: f64,
    integral: f64,
    filtered_error: f64,
    target: usize,
    primed: bool,
}
impl Consumer {
    pub(crate) fn new(ring: Arc<Ring>, kernel: Arc<Kernel>, sample_rate: u32) -> Self {
        Self {
            ring,
            kernel,
            sample_rate: f64::from(sample_rate),
            position: 0,
            fraction: 0.0,
            ratio: 1.0,
            integral: 0.0,
            filtered_error: 0.0,
            target: 0,
            primed: false,
        }
    }
    /// Prepare once per device callback, not once per mixer chunk. During initial
    /// startup, silence is explicit until there is enough data for this packet
    /// plus two input/output packet spans. After startup a gap is a latched fault.
    pub(crate) fn prepare(&mut self, frames: usize) -> Result<bool, TransportError> {
        let written = self.ring.written.load(Ordering::Acquire);
        if !self.primed {
            let packet = frames.max(self.ring.largest_packet.load(Ordering::Relaxed));
            self.target = self.target.max((packet * 2 + TAPS).max(1024));
            if written < (self.target + HALF) as u64 {
                return Ok(false);
            }
            self.position = written - self.target as u64;
            self.primed = true;
        }
        let available = written
            .checked_sub(self.position)
            .ok_or(TransportError::Underflow)?;
        // Reserve the complete callback before consuming anything. Concurrent
        // production can add data; it cannot invalidate already published data.
        let required = (frames as f64 * (1.0 + MAX_CORRECTION)).ceil() as u64 + HALF as u64 + 1;
        if available < required {
            return Err(TransportError::Underflow);
        }
        let dt = frames as f64 / self.sample_rate;
        let error = available as f64 - self.target as f64;
        // Smooth packet scheduling jitter before a bounded PI occupancy servo.
        // No frame insertion/deletion: correction changes the continuous phase.
        self.filtered_error += (1.0 - (-dt / 0.5).exp()) * (error - self.filtered_error);
        self.integral = (self.integral + self.filtered_error / self.sample_rate * dt * 0.02)
            .clamp(-MAX_CORRECTION, MAX_CORRECTION);
        let correction = (self.integral + self.filtered_error / self.sample_rate * 0.1)
            .clamp(-MAX_CORRECTION, MAX_CORRECTION);
        self.ratio = 1.0 + correction;
        self.ring.buffered.store(available, Ordering::Relaxed);
        self.ring
            .correction_ppm
            .store((correction * 1_000_000.0).to_bits(), Ordering::Relaxed);
        Ok(true)
    }
    pub(crate) fn read(&mut self, left: &mut [f32], right: &mut [f32]) {
        debug_assert_eq!(left.len(), right.len());
        for (left, right) in left.iter_mut().zip(right) {
            let phase = self.fraction * PHASES as f64;
            let row = phase.floor() as usize;
            let blend = (phase - row as f64) as f32;
            let mut output = [0.0_f32; 2];
            for tap in 0..TAPS {
                let coefficient = self.kernel.rows[row][tap]
                    + blend * (self.kernel.rows[row + 1][tap] - self.kernel.rows[row][tap]);
                let source = self
                    .ring
                    .sample(self.position - (HALF - 1) as u64 + tap as u64);
                output[0] += source[0] * coefficient;
                output[1] += source[1] * coefficient;
            }
            *left = output[0];
            *right = output[1];
            self.fraction += self.ratio;
            let whole = self.fraction.floor();
            self.position += whole as u64;
            self.fraction -= whole;
        }
        self.ring
            .released
            .store(self.position.saturating_sub(HALF as u64), Ordering::Release);
    }
}

#[cfg(test)]
mod spectral_tests {
    use super::*;
    #[test]
    fn bandlimited_filter_has_full_audible_passband_and_rejects_alias_band() {
        let kernel = Kernel::new();
        for sample_rate in [44100.0_f64, 48000.0] {
            for phase in (0..=PHASES).step_by(16) {
                let magnitude = |hz: f64| {
                    let mut real = 0.0;
                    let mut imaginary = 0.0;
                    for (tap, coefficient) in kernel.rows[phase].iter().enumerate() {
                        let angle = std::f64::consts::TAU * hz * tap as f64 / sample_rate;
                        real += f64::from(*coefficient) * angle.cos();
                        imaginary += f64::from(*coefficient) * angle.sin();
                    }
                    real.hypot(imaginary)
                };
                for hz in [20.0, 440.0, 1000.0, 10000.0, 18000.0, 20000.0] {
                    let db = 20.0 * magnitude(hz).log10();
                    assert!(
                        db.abs() < 0.1,
                        "rate {sample_rate} phase {phase} hz {hz}: {db} dB"
                    );
                }
                // Fastest allowed read ratio moves this input frequency to the
                // output Nyquist limit. Check the whole folded interval.
                for fraction in 0..=16 {
                    let nyquist = sample_rate / 2.0;
                    let start = nyquist / (1.0 + MAX_CORRECTION);
                    let hz = start + (nyquist - start) * fraction as f64 / 16.0;
                    let db = 20.0 * magnitude(hz).max(1e-20).log10();
                    assert!(
                        db < -65.0,
                        "rate {sample_rate} phase {phase} hz {hz}: {db} dB"
                    );
                }
            }
        }
    }
}
