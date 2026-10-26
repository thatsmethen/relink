//! Original portable CoreAudio packet validation, decoding and timing custody.
use crate::{
    AudioFault, NativeCallbackCompletion, NativeCallbackOutcome, NativeCaptureFault,
    NativeCapturePacket, NativeLifecycleEvent, NativeLifecyclePhase, NativeTimingSnapshot,
    MAX_CALLBACK_FRAMES, MAX_DEVICE_CHANNELS, NATIVE_LIFECYCLE_PHASES,
};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub(crate) const LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
pub(crate) const FLOAT: u32 = 1;
pub(crate) const BIG_ENDIAN: u32 = 2;
pub(crate) const SIGNED: u32 = 4;
pub(crate) const PACKED: u32 = 8;
pub(crate) const HIGH_ALIGNED: u32 = 16;
pub(crate) const NON_INTERLEAVED: u32 = 32;
const SAMPLE_VALID: u32 = 1;
const HOST_VALID: u32 = 2;
const RATE_VALID: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Format {
    pub(crate) rate: f64,
    pub(crate) id: u32,
    pub(crate) flags: u32,
    pub(crate) packet_bytes: u32,
    pub(crate) packet_frames: u32,
    pub(crate) frame_bytes: u32,
    pub(crate) channels: u32,
    pub(crate) bits: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Encoding {
    F32,
    F64,
    Signed { bits: u32, high: bool },
    U8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Plane {
    pub(crate) channels: u32,
    pub(crate) frame_bytes: usize,
    sample_bytes: usize,
    big_endian: bool,
    encoding: Encoding,
}
impl Plane {
    // Read-only access to the already validated native representation. Output
    // encoding shares format validation without changing capture decoding.
    pub(crate) fn sample_bytes(&self) -> usize {
        self.sample_bytes
    }
    pub(crate) fn big_endian(&self) -> bool {
        self.big_endian
    }
    pub(crate) fn encoding(&self) -> Encoding {
        self.encoding
    }

    fn sample(&self, bytes: &[u8], frame: usize, channel: usize) -> f32 {
        let offset = frame * self.frame_bytes + channel * self.sample_bytes;
        let bytes = &bytes[offset..offset + self.sample_bytes];
        let mut value = 0_u64;
        if self.big_endian {
            for byte in bytes {
                value = (value << 8) | u64::from(*byte);
            }
        } else {
            for (shift, byte) in bytes.iter().enumerate() {
                value |= u64::from(*byte) << (shift * 8);
            }
        }
        let sample = match self.encoding {
            Encoding::F32 => f32::from_bits(value as u32),
            Encoding::F64 => f64::from_bits(value) as f32,
            Encoding::U8 => (value as f32 - 128.0) / 128.0,
            Encoding::Signed { bits, high } => {
                if high {
                    value >>= self.sample_bytes as u32 * 8 - bits;
                }
                // Sign-extend only the declared significant bits. Low padding
                // or a wider native container cannot become audible data.
                let signed = ((value << (64 - bits)) as i64) >> (64 - bits);
                signed as f32 / (1_u64 << (bits - 1)) as f32
            }
        };
        if sample.is_finite() {
            sample
        } else {
            0.0
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) planes: Vec<Plane>,
    pub(crate) channels: u16,
}
impl Layout {
    /// Must run off the callback. Formats are in native stream/channel order.
    pub(crate) fn new(formats: &[Format], rate: u32) -> Result<Self, AudioFault> {
        let mut planes = Vec::new();
        let mut channels = 0_u32;
        for format in formats {
            if format.rate != f64::from(rate)
                || format.id != LINEAR_PCM
                || !(1..=u32::from(MAX_DEVICE_CHANNELS)).contains(&format.channels)
                || format.packet_frames != 1
                || format.packet_bytes != format.frame_bytes
                || format.frame_bytes == 0
                || format.flags
                    & !(FLOAT | BIG_ENDIAN | SIGNED | PACKED | HIGH_ALIGNED | NON_INTERLEAVED)
                    != 0
            {
                return Err(AudioFault::StreamInvalidated);
            }
            channels = channels
                .checked_add(format.channels)
                .ok_or(AudioFault::InvalidBuffer)?;
            if channels > u32::from(MAX_DEVICE_CHANNELS) {
                return Err(AudioFault::InvalidBuffer);
            }
            let interleaved = format.flags & NON_INTERLEAVED == 0;
            let plane_channels = if interleaved { format.channels } else { 1 };
            if !format.frame_bytes.is_multiple_of(plane_channels) {
                return Err(AudioFault::StreamInvalidated);
            }
            let width = format.frame_bytes / plane_channels;
            let encoding = if format.flags & FLOAT != 0 {
                if !matches!(width, 4 | 8)
                    || format.flags & (SIGNED | HIGH_ALIGNED) != 0
                    || format.bits != width * 8
                {
                    return Err(AudioFault::StreamInvalidated);
                }
                match (format.bits, width) {
                    (32, 4) => Encoding::F32,
                    (64, 8) => Encoding::F64,
                    _ => return Err(AudioFault::StreamInvalidated),
                }
            } else if format.flags & SIGNED != 0 {
                if !(1..=4).contains(&width)
                    || !matches!(format.bits, 8 | 16 | 24 | 32)
                    || format.bits > width * 8
                    || (format.flags & PACKED != 0
                        && (format.bits != width * 8 || format.flags & HIGH_ALIGNED != 0))
                {
                    return Err(AudioFault::StreamInvalidated);
                }
                Encoding::Signed {
                    bits: format.bits,
                    high: format.flags & HIGH_ALIGNED != 0,
                }
            } else if width == 1 && format.bits == 8 && format.flags & HIGH_ALIGNED == 0 {
                Encoding::U8
            } else {
                return Err(AudioFault::StreamInvalidated);
            };
            let plane = Plane {
                channels: plane_channels,
                frame_bytes: format.frame_bytes as usize,
                sample_bytes: width as usize,
                big_endian: format.flags & BIG_ENDIAN != 0,
                encoding,
            };
            for _ in 0..if interleaved { 1 } else { format.channels } {
                planes.push(plane);
            }
        }
        if planes.is_empty() {
            return Err(AudioFault::InvalidBuffer);
        }
        Ok(Self {
            planes,
            channels: channels as u16,
        })
    }

    /// Shape validation uses actual packet lengths and accepts bounded variable
    /// callback sizes. Null enabled planes, partial frames and mismatched plane
    /// durations are faults, never invented silence or implicit format changes.
    pub(crate) fn validate_sizes(
        &self,
        sizes: impl Iterator<Item = (u32, usize, bool)>,
    ) -> Result<usize, AudioFault> {
        let mut count = 0;
        let mut frames = None;
        for (index, (channels, bytes, present)) in sizes.enumerate() {
            let plane = self.planes.get(index).ok_or(AudioFault::InvalidBuffer)?;
            if !present || channels != plane.channels || !bytes.is_multiple_of(plane.frame_bytes) {
                return Err(AudioFault::InvalidBuffer);
            }
            let packet_frames = bytes / plane.frame_bytes;
            if packet_frames == 0
                || packet_frames > MAX_CALLBACK_FRAMES
                || frames.is_some_and(|value| value != packet_frames)
            {
                return Err(AudioFault::InvalidBuffer);
            }
            frames = Some(packet_frames);
            count += 1;
        }
        if count != self.planes.len() {
            return Err(AudioFault::InvalidBuffer);
        }
        frames.ok_or(AudioFault::InvalidBuffer)
    }

    pub(crate) fn stereo_reader<'a>(&self, bytes: impl Fn(usize) -> &'a [u8]) -> StereoReader<'a> {
        let right_plane = usize::from(self.planes[0].channels == 1 && self.channels > 1);
        let right_channel = usize::from(self.planes[0].channels > 1);
        StereoReader {
            left: (self.planes[0], bytes(0), 0),
            right: (self.planes[right_plane], bytes(right_plane), right_channel),
        }
    }
}
pub(crate) struct StereoReader<'a> {
    left: (Plane, &'a [u8], usize),
    right: (Plane, &'a [u8], usize),
}
impl StereoReader<'_> {
    pub(crate) fn frame(&self, frame: usize) -> [f32; 2] {
        [
            self.left.0.sample(self.left.1, frame, self.left.2),
            self.right.0.sample(self.right.1, frame, self.right.2),
        ]
    }
}

#[derive(Default)]
pub(crate) struct Tracker {
    pub(crate) previous: Option<NativeCapturePacket>,
}
impl Tracker {
    pub(crate) fn check(&self, packet: &NativeCapturePacket) -> Result<(), AudioFault> {
        if packet.frames == 0 || packet.frames as usize > MAX_CALLBACK_FRAMES {
            return Err(AudioFault::InvalidBuffer);
        }
        if packet.flags & (SAMPLE_VALID | HOST_VALID) != SAMPLE_VALID | HOST_VALID
            || !packet.sample_time.is_finite()
            || packet.host_time == 0
            || (packet.flags & RATE_VALID != 0
                && (!packet.rate_scalar.is_finite() || packet.rate_scalar <= 0.0))
        {
            return Err(AudioFault::CaptureTimestamp);
        }
        // Sample-domain rounding tolerance stays strictly below one frame.
        // Very large positions that have lost single-frame precision are not
        // silently accepted. Negative but valid initial sample positions work.
        let magnitude = self.previous.map_or(packet.sample_time.abs(), |p| {
            p.sample_time.abs().max(packet.sample_time.abs())
        });
        let tolerance = (4.0 * f64::EPSILON * magnitude).max(1.0e-7);
        if tolerance >= 0.5 {
            return Err(AudioFault::CaptureTimestamp);
        }
        if let Some(previous) = self.previous {
            if packet.epoch != previous.epoch || packet.host_time <= previous.host_time {
                return Err(AudioFault::CaptureTimestamp);
            }
            let difference = packet.sample_time - previous.sample_time - f64::from(previous.frames);
            if difference.abs() > tolerance {
                return Err(AudioFault::Xrun);
            }
        }
        Ok(())
    }
    pub(crate) fn commit(&mut self, packet: NativeCapturePacket) {
        self.previous = Some(packet);
    }
}

const WORDS: usize = 9;
struct AtomicPacket([AtomicU64; WORDS]);
impl AtomicPacket {
    fn new() -> Self {
        Self(std::array::from_fn(|_| AtomicU64::new(0)))
    }
    fn store(&self, p: NativeCapturePacket, ordering: Ordering) {
        for (slot, word) in self.0.iter().zip([
            p.packet_index,
            p.queue_frame,
            p.sample_time.to_bits(),
            p.host_time,
            p.observed_host_time,
            p.rate_scalar.to_bits(),
            u64::from(p.flags),
            u64::from(p.frames),
            p.epoch,
        ]) {
            slot.store(word, ordering);
        }
    }
    fn load(&self, ordering: Ordering) -> NativeCapturePacket {
        let a: [u64; WORDS] = std::array::from_fn(|i| self.0[i].load(ordering));
        NativeCapturePacket {
            packet_index: a[0],
            queue_frame: a[1],
            sample_time: f64::from_bits(a[2]),
            host_time: a[3],
            observed_host_time: a[4],
            rate_scalar: f64::from_bits(a[5]),
            flags: a[6] as u32,
            frames: a[7] as u32,
            epoch: a[8],
        }
    }
}

struct AtomicCompletion {
    sequence: AtomicU64,
    words: [AtomicU64; 8],
}
impl AtomicCompletion {
    fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
    fn store(&self, value: NativeCallbackCompletion) {
        let outcome = match value.outcome {
            NativeCallbackOutcome::Published => 0,
            NativeCallbackOutcome::Cancelled => 1,
            NativeCallbackOutcome::Rejected(fault) => u64::from(fault as u32) + 2,
        };
        self.sequence.fetch_add(1, Ordering::SeqCst);
        for (slot, word) in self.words.iter().zip([
            value.packet_index,
            value.entry_host_time,
            value.after_pcm_host_time,
            value.completion_host_time,
            value.os_now_sample_time.to_bits(),
            value.os_now_host_time,
            u64::from(value.os_now_flags),
            outcome,
        ]) {
            slot.store(word, Ordering::SeqCst);
        }
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }
    fn load(&self) -> (Option<NativeCallbackCompletion>, bool) {
        let before = self.sequence.load(Ordering::SeqCst);
        if before == 0 {
            return (None, false);
        }
        let a: [u64; 8] = std::array::from_fn(|i| self.words[i].load(Ordering::SeqCst));
        let after = self.sequence.load(Ordering::SeqCst);
        if before != after || !before.is_multiple_of(2) {
            return (None, true);
        }
        (
            Some(NativeCallbackCompletion {
                packet_index: a[0],
                entry_host_time: a[1],
                after_pcm_host_time: a[2],
                completion_host_time: a[3],
                os_now_sample_time: f64::from_bits(a[4]),
                os_now_host_time: a[5],
                os_now_flags: a[6] as u32,
                outcome: match a[7] {
                    0 => NativeCallbackOutcome::Published,
                    1 => NativeCallbackOutcome::Cancelled,
                    code => {
                        NativeCallbackOutcome::Rejected(AudioFault::from_code((code - 2) as u32))
                    }
                },
            }),
            false,
        )
    }
}

const PHASES: [NativeLifecyclePhase; NATIVE_LIFECYCLE_PHASES] = [
    NativeLifecyclePhase::CreateRequested,
    NativeLifecyclePhase::CreateReturned,
    NativeLifecyclePhase::StartRequested,
    NativeLifecyclePhase::StartReturned,
    NativeLifecyclePhase::HandleCancelled,
    NativeLifecyclePhase::GatesRevoked,
    NativeLifecyclePhase::StopRequested,
    NativeLifecyclePhase::StopReturned,
    NativeLifecyclePhase::DestroyRequested,
    NativeLifecyclePhase::DestroyReturned,
    NativeLifecyclePhase::ListenersRemovalRequested,
    NativeLifecyclePhase::ListenersRemovalReturned,
    NativeLifecyclePhase::QuiescenceChecked,
    NativeLifecyclePhase::ContextRetained,
    NativeLifecyclePhase::OwnerCompleted,
];
struct AtomicLifecycle {
    state: AtomicU32,
    words: [AtomicU64; 3],
}
impl AtomicLifecycle {
    fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
    fn store(&self, value: NativeLifecycleEvent) {
        // One immutable publication per phase. A duplicate cannot replace the
        // first observation. Neither readers nor writers retry or spin.
        if self
            .state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        self.words[0].store(value.host_time, Ordering::Relaxed);
        self.words[1].store(
            value
                .status
                .map_or(0, |s| (1_u64 << 32) | u64::from(s as u32)),
            Ordering::Relaxed,
        );
        self.words[2].store(
            value.callbacks_in_flight.unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.state.store(2, Ordering::Release);
    }
    fn load(&self, phase: NativeLifecyclePhase) -> Option<NativeLifecycleEvent> {
        if self.state.load(Ordering::Acquire) != 2 {
            return None;
        }
        let status = self.words[1].load(Ordering::Relaxed);
        let in_flight = self.words[2].load(Ordering::Relaxed);
        Some(NativeLifecycleEvent {
            phase,
            host_time: self.words[0].load(Ordering::Relaxed),
            status: (status >> 32 != 0).then_some(status as i32),
            callbacks_in_flight: (in_flight != u64::MAX).then_some(in_flight),
        })
    }
}

/// One callback writer. The last packet uses a bounded, nonretrying sequence
/// read; first-fault data is written once and independently release-published.
pub(crate) struct Evidence {
    sequence: AtomicU64,
    seen: AtomicU64,
    last: AtomicPacket,
    fault: AtomicU32,
    has_previous: AtomicU32,
    previous: AtomicPacket,
    rejected: AtomicPacket,
    completed: AtomicCompletion,
    lifecycle: [AtomicLifecycle; NATIVE_LIFECYCLE_PHASES],
}
impl Evidence {
    pub(crate) fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            seen: AtomicU64::new(0),
            last: AtomicPacket::new(),
            fault: AtomicU32::new(0),
            has_previous: AtomicU32::new(0),
            previous: AtomicPacket::new(),
            rejected: AtomicPacket::new(),
            completed: AtomicCompletion::new(),
            lifecycle: std::array::from_fn(|_| AtomicLifecycle::new()),
        }
    }
    pub(crate) fn complete(&self, value: NativeCallbackCompletion) {
        self.completed.store(value);
    }
    pub(crate) fn lifecycle(&self, value: NativeLifecycleEvent) {
        self.lifecycle[value.phase as usize].store(value);
    }
    pub(crate) fn record(
        &self,
        packet: NativeCapturePacket,
        previous: Option<NativeCapturePacket>,
        fault: Option<AudioFault>,
    ) {
        // Sequential consistency gives the values and sequence one common
        // order, including when a reader is preempted around the writer.
        self.sequence.fetch_add(1, Ordering::SeqCst);
        self.last.store(packet, Ordering::SeqCst);
        self.seen.store(packet.packet_index, Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
        if let Some(fault) = fault {
            if self.fault.load(Ordering::Relaxed) == 0 {
                if let Some(previous) = previous {
                    self.previous.store(previous, Ordering::Relaxed);
                }
                self.has_previous
                    .store(u32::from(previous.is_some()), Ordering::Relaxed);
                self.rejected.store(packet, Ordering::Relaxed);
                self.fault.store(fault as u32, Ordering::Release);
            }
        }
    }
    pub(crate) fn snapshot(&self) -> Option<NativeTimingSnapshot> {
        let before = self.sequence.load(Ordering::SeqCst);
        let last = self.last.load(Ordering::SeqCst);
        let seen = self.seen.load(Ordering::SeqCst);
        let after = self.sequence.load(Ordering::SeqCst);
        let coherent = before == after && before.is_multiple_of(2);
        let (last_completed_callback, callback_completion_contended) = self.completed.load();
        let lifecycle = std::array::from_fn(|i| self.lifecycle[i].load(PHASES[i]));
        if before == 0
            && after == 0
            && last_completed_callback.is_none()
            && !callback_completion_contended
            && lifecycle.iter().all(Option::is_none)
        {
            return None;
        }
        let fault = self.fault.load(Ordering::Acquire);
        let first_fault = (fault != 0).then(|| NativeCaptureFault {
            fault: AudioFault::from_code(fault),
            previous: (self.has_previous.load(Ordering::Relaxed) != 0)
                .then(|| self.previous.load(Ordering::Relaxed)),
            current: self.rejected.load(Ordering::Relaxed),
        });
        Some(NativeTimingSnapshot {
            packets_seen: seen,
            last_packet: (coherent && before != 0).then_some(last),
            first_fault,
            last_packet_contended: !coherent,
            last_completed_callback,
            callback_completion_contended,
            lifecycle,
        })
    }
}
