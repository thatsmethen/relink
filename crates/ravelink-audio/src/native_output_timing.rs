//! Bounded original output timing custody. Presentation timestamps are native
//! future declarations, never capture timestamps or observations of playback.
use crate::{AudioFault, MAX_CALLBACK_FRAMES};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NativeOutputStamp {
    pub sample_time: f64,
    pub host_time: u64,
    pub rate_scalar: f64,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NativeOutputPacket {
    pub callback_index: u64,
    pub epoch: u64,
    pub frames: u32,
    pub entry_host_time: u64,
    pub now: NativeOutputStamp,
    pub presentation: NativeOutputStamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NativeOutputFaultReason {
    InvalidBuffer = 1,
    InvalidTimestamp = 2,
    SampleDiscontinuity = 3,
    HostDiscontinuity = 4,
    EpochChanged = 5,
    Reentry = 6,
    CounterExhausted = 7,
}
impl NativeOutputFaultReason {
    fn decode(value: u32) -> Self {
        match value {
            2 => Self::InvalidTimestamp,
            3 => Self::SampleDiscontinuity,
            4 => Self::HostDiscontinuity,
            5 => Self::EpochChanged,
            6 => Self::Reentry,
            7 => Self::CounterExhausted,
            _ => Self::InvalidBuffer,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeOutputFault {
    pub fault: AudioFault,
    pub reason: NativeOutputFaultReason,
    /// None when there is no accepted predecessor, or the reentry observer
    /// could not read it coherently without waiting for the active writer.
    pub previous: Option<NativeOutputPacket>,
    pub current: NativeOutputPacket,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeOutputOutcome {
    InactiveSilence,
    PrimingSilence,
    Encoded { accepted_version: u64 },
    Cancelled,
    Rejected(AudioFault),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeOutputCompletion {
    pub packet: NativeOutputPacket,
    pub after_render_host_time: u64,
    /// Sampled immediately before publishing this record and returning. It is
    /// not a measurement made after the native host regains control.
    pub completion_host_time: u64,
    pub outcome: NativeOutputOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NativeOutputLifecyclePhase {
    ReservationAcquired,
    CreateRequested,
    CreateReturned,
    Ready,
    StartRequested,
    StartReturned,
    HandleCancelled,
    GatesRevoked,
    StopRequested,
    StopReturned,
    DestroyRequested,
    DestroyReturned,
    ListenerRemovalRequested,
    ListenerRemovalReturned,
    QuiescenceChecked,
    ContextRetained,
    OwnerCompleted,
}
pub const NATIVE_OUTPUT_LIFECYCLE_PHASES: usize = 17;
const PHASES: [NativeOutputLifecyclePhase; NATIVE_OUTPUT_LIFECYCLE_PHASES] = [
    NativeOutputLifecyclePhase::ReservationAcquired,
    NativeOutputLifecyclePhase::CreateRequested,
    NativeOutputLifecyclePhase::CreateReturned,
    NativeOutputLifecyclePhase::Ready,
    NativeOutputLifecyclePhase::StartRequested,
    NativeOutputLifecyclePhase::StartReturned,
    NativeOutputLifecyclePhase::HandleCancelled,
    NativeOutputLifecyclePhase::GatesRevoked,
    NativeOutputLifecyclePhase::StopRequested,
    NativeOutputLifecyclePhase::StopReturned,
    NativeOutputLifecyclePhase::DestroyRequested,
    NativeOutputLifecyclePhase::DestroyReturned,
    NativeOutputLifecyclePhase::ListenerRemovalRequested,
    NativeOutputLifecyclePhase::ListenerRemovalReturned,
    NativeOutputLifecyclePhase::QuiescenceChecked,
    NativeOutputLifecyclePhase::ContextRetained,
    NativeOutputLifecyclePhase::OwnerCompleted,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeOutputLifecycleEvent {
    pub phase: NativeOutputLifecyclePhase,
    pub host_time: u64,
    pub status: Option<i32>,
    pub callbacks_in_flight: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeOutputTimingSnapshot {
    pub callbacks_seen: u64,
    /// Complete valid callback count, including silence with activation closed.
    /// Native Start acknowledgment remains a separate lifecycle observation.
    pub progress_count: u64,
    pub last_packet: Option<NativeOutputPacket>,
    pub last_packet_contended: bool,
    pub first_fault: Option<NativeOutputFault>,
    pub first_fault_pending: bool,
    pub last_completion: Option<NativeOutputCompletion>,
    pub completion_contended: bool,
    pub lifecycle: [Option<NativeOutputLifecycleEvent>; NATIVE_OUTPUT_LIFECYCLE_PHASES],
}

const PACKET_WORDS: usize = 12;
fn words(p: NativeOutputPacket) -> [u64; PACKET_WORDS] {
    [
        p.callback_index,
        p.epoch,
        u64::from(p.frames),
        p.entry_host_time,
        p.now.sample_time.to_bits(),
        p.now.host_time,
        p.now.rate_scalar.to_bits(),
        u64::from(p.now.flags),
        p.presentation.sample_time.to_bits(),
        p.presentation.host_time,
        p.presentation.rate_scalar.to_bits(),
        u64::from(p.presentation.flags),
    ]
}
fn packet(a: [u64; PACKET_WORDS]) -> NativeOutputPacket {
    NativeOutputPacket {
        callback_index: a[0],
        epoch: a[1],
        frames: a[2] as u32,
        entry_host_time: a[3],
        now: NativeOutputStamp {
            sample_time: f64::from_bits(a[4]),
            host_time: a[5],
            rate_scalar: f64::from_bits(a[6]),
            flags: a[7] as u32,
        },
        presentation: NativeOutputStamp {
            sample_time: f64::from_bits(a[8]),
            host_time: a[9],
            rate_scalar: f64::from_bits(a[10]),
            flags: a[11] as u32,
        },
    }
}
struct AtomicWords<const N: usize>([AtomicU64; N]);
impl<const N: usize> AtomicWords<N> {
    fn new() -> Self {
        Self(std::array::from_fn(|_| AtomicU64::new(0)))
    }
    fn store(&self, words: [u64; N], ordering: Ordering) {
        for (target, word) in self.0.iter().zip(words) {
            target.store(word, ordering);
        }
    }
    fn load(&self, ordering: Ordering) -> [u64; N] {
        std::array::from_fn(|i| self.0[i].load(ordering))
    }
}
struct Latest<const N: usize> {
    sequence: AtomicU64,
    words: AtomicWords<N>,
}
impl<const N: usize> Latest<N> {
    fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            words: AtomicWords::new(),
        }
    }
    /// Only the guarded IOProc writer may publish latest records.
    fn store(&self, words: [u64; N]) {
        self.sequence.fetch_add(1, Ordering::SeqCst);
        self.words.store(words, Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }
    fn load(&self) -> (Option<[u64; N]>, bool) {
        let before = self.sequence.load(Ordering::SeqCst);
        if before == 0 {
            return (None, false);
        }
        let words = self.words.load(Ordering::SeqCst);
        let after = self.sequence.load(Ordering::SeqCst);
        if before != after || before & 1 != 0 {
            (None, true)
        } else {
            (Some(words), false)
        }
    }
}
struct FaultRecord {
    identity: AtomicU32,
    ready: AtomicBool,
    previous_present: AtomicBool,
    previous: AtomicWords<PACKET_WORDS>,
    current: AtomicWords<PACKET_WORDS>,
}
impl FaultRecord {
    fn new() -> Self {
        Self {
            identity: AtomicU32::new(0),
            ready: AtomicBool::new(false),
            previous_present: AtomicBool::new(false),
            previous: AtomicWords::new(),
            current: AtomicWords::new(),
        }
    }
    fn store(&self, value: NativeOutputFault) {
        let identity = value.fault as u32 | (value.reason as u32) << 8;
        if self
            .identity
            .compare_exchange(0, identity, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        if let Some(previous) = value.previous {
            self.previous.store(words(previous), Ordering::Relaxed);
            self.previous_present.store(true, Ordering::Relaxed);
        }
        self.current.store(words(value.current), Ordering::Relaxed);
        self.ready.store(true, Ordering::Release);
    }
    fn load(&self) -> (Option<NativeOutputFault>, bool) {
        let identity = self.identity.load(Ordering::Acquire);
        if identity == 0 {
            return (None, false);
        }
        if !self.ready.load(Ordering::Acquire) {
            return (None, true);
        }
        (
            Some(NativeOutputFault {
                fault: AudioFault::from_code(identity & 0xff),
                reason: NativeOutputFaultReason::decode(identity >> 8),
                previous: self
                    .previous_present
                    .load(Ordering::Relaxed)
                    .then(|| packet(self.previous.load(Ordering::Relaxed))),
                current: packet(self.current.load(Ordering::Relaxed)),
            }),
            false,
        )
    }
}
struct LifecycleSlot {
    state: AtomicU32,
    values: AtomicWords<5>,
}
impl LifecycleSlot {
    fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
            values: AtomicWords::new(),
        }
    }
    fn store(&self, e: NativeOutputLifecycleEvent) {
        if self
            .state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        self.values.store(
            [
                e.host_time,
                e.status.unwrap_or_default() as u32 as u64,
                u64::from(e.status.is_some()),
                e.callbacks_in_flight.unwrap_or_default(),
                u64::from(e.callbacks_in_flight.is_some()),
            ],
            Ordering::Relaxed,
        );
        self.state.store(2, Ordering::Release);
    }
    fn load(&self, phase: NativeOutputLifecyclePhase) -> Option<NativeOutputLifecycleEvent> {
        if self.state.load(Ordering::Acquire) != 2 {
            return None;
        }
        let a = self.values.load(Ordering::Relaxed);
        Some(NativeOutputLifecycleEvent {
            phase,
            host_time: a[0],
            status: (a[2] != 0).then_some(a[1] as u32 as i32),
            callbacks_in_flight: (a[4] != 0).then_some(a[3]),
        })
    }
}

pub(crate) struct NativeOutputTiming {
    callbacks: AtomicU64,
    progress: AtomicU64,
    last: Latest<PACKET_WORDS>,
    completion: Latest<16>,
    fault: FaultRecord,
    lifecycle: [LifecycleSlot; NATIVE_OUTPUT_LIFECYCLE_PHASES],
}
impl NativeOutputTiming {
    pub(crate) fn new() -> Self {
        Self {
            callbacks: AtomicU64::new(0),
            progress: AtomicU64::new(0),
            last: Latest::new(),
            completion: Latest::new(),
            fault: FaultRecord::new(),
            lifecycle: std::array::from_fn(|_| LifecycleSlot::new()),
        }
    }
    pub(crate) fn enter(&self) -> u64 {
        self.callbacks
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }
    pub(crate) fn progress_count(&self) -> u64 {
        self.progress.load(Ordering::Acquire)
    }
    pub(crate) fn accept(&self, value: NativeOutputPacket) {
        self.last.store(words(value));
    }
    pub(crate) fn reject(&self, value: NativeOutputFault) {
        self.fault.store(value);
    }
    pub(crate) fn last_packet(&self) -> Option<NativeOutputPacket> {
        self.last.load().0.map(packet)
    }
    pub(crate) fn complete(&self, value: NativeOutputCompletion, valid_progress: bool) {
        let (outcome, version) = match value.outcome {
            NativeOutputOutcome::InactiveSilence => (0, 0),
            NativeOutputOutcome::PrimingSilence => (1, 0),
            NativeOutputOutcome::Encoded { accepted_version } => (2, accepted_version),
            NativeOutputOutcome::Cancelled => (3, 0),
            NativeOutputOutcome::Rejected(fault) => (u64::from(fault as u32) + 4, 0),
        };
        let mut a = [0; 16];
        a[..PACKET_WORDS].copy_from_slice(&words(value.packet));
        a[12..].copy_from_slice(&[
            value.after_render_host_time,
            value.completion_host_time,
            outcome,
            version,
        ]);
        self.completion.store(a);
        if valid_progress {
            self.progress.fetch_add(1, Ordering::Release);
        }
    }
    pub(crate) fn lifecycle(&self, value: NativeOutputLifecycleEvent) {
        self.lifecycle[value.phase as usize].store(value);
    }
    pub(crate) fn snapshot(&self) -> NativeOutputTimingSnapshot {
        let (last, last_packet_contended) = self.last.load();
        let (completion, completion_contended) = self.completion.load();
        let (first_fault, first_fault_pending) = self.fault.load();
        NativeOutputTimingSnapshot {
            callbacks_seen: self.callbacks.load(Ordering::Acquire),
            progress_count: self.progress_count(),
            last_packet: last.map(packet),
            last_packet_contended,
            first_fault,
            first_fault_pending,
            last_completion: completion.map(|a| NativeOutputCompletion {
                packet: packet(std::array::from_fn(|i| a[i])),
                after_render_host_time: a[12],
                completion_host_time: a[13],
                outcome: match a[14] {
                    0 => NativeOutputOutcome::InactiveSilence,
                    1 => NativeOutputOutcome::PrimingSilence,
                    2 => NativeOutputOutcome::Encoded {
                        accepted_version: a[15],
                    },
                    3 => NativeOutputOutcome::Cancelled,
                    other => {
                        NativeOutputOutcome::Rejected(AudioFault::from_code((other - 4) as u32))
                    }
                },
            }),
            completion_contended,
            lifecycle: std::array::from_fn(|i| self.lifecycle[i].load(PHASES[i])),
        }
    }
}

#[derive(Default)]
pub(crate) struct Tracker {
    pub(crate) previous: Option<NativeOutputPacket>,
}
impl Tracker {
    pub(crate) fn check(
        &self,
        p: NativeOutputPacket,
    ) -> Result<(), (AudioFault, NativeOutputFaultReason)> {
        use NativeOutputFaultReason as Reason;
        if p.frames == 0 || p.frames as usize > MAX_CALLBACK_FRAMES {
            return Err((AudioFault::InvalidBuffer, Reason::InvalidBuffer));
        }
        for stamp in [p.now, p.presentation] {
            if stamp.flags & 3 != 3
                || !stamp.sample_time.is_finite()
                || stamp.host_time == 0
                || (stamp.flags & 4 != 0
                    && (!stamp.rate_scalar.is_finite() || stamp.rate_scalar <= 0.0))
            {
                return Err((AudioFault::StreamInvalidated, Reason::InvalidTimestamp));
            }
        }
        let magnitude = self
            .previous
            .map_or(p.presentation.sample_time.abs(), |old| {
                old.presentation
                    .sample_time
                    .abs()
                    .max(p.presentation.sample_time.abs())
            });
        let tolerance = (4.0 * f64::EPSILON * magnitude).max(1.0e-7);
        if tolerance >= 0.5 {
            return Err((AudioFault::StreamInvalidated, Reason::InvalidTimestamp));
        }
        if let Some(previous) = self.previous {
            if previous.epoch != p.epoch {
                return Err((AudioFault::StreamInvalidated, Reason::EpochChanged));
            }
            if p.presentation.host_time <= previous.presentation.host_time {
                return Err((AudioFault::Xrun, Reason::HostDiscontinuity));
            }
            let delta = p.presentation.sample_time
                - previous.presentation.sample_time
                - f64::from(previous.frames);
            if delta.abs() > tolerance {
                return Err((AudioFault::Xrun, Reason::SampleDiscontinuity));
            }
        }
        Ok(())
    }
    pub(crate) fn commit(&mut self, p: NativeOutputPacket) {
        self.previous = Some(p);
    }
}

#[cfg(test)]
#[path = "native_output_timing_tests.rs"]
mod tests;
