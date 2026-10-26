//! Portable owner controls and diagnostics. No COM, device calls, or waiting.
use std::sync::{
    atomic::{AtomicU32, AtomicU64, Ordering},
    Mutex,
};

pub(crate) const OWNER_STACK_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_RESERVATIONS: usize = 64;
const READY: u32 = 1;
const REQUESTED: u32 = 2;
const STARTED: u32 = 4;
const PROGRAM: u32 = 8;
const CANCELLED: u32 = 16;

#[derive(Default)]
pub(crate) struct Gates(AtomicU32);
impl Gates {
    pub(crate) fn ready(&self) {
        self.0.fetch_or(READY, Ordering::Release);
    }
    pub(crate) fn is_ready(&self) -> bool {
        self.0.load(Ordering::Acquire) & READY != 0
    }
    pub(crate) fn request_start(&self) -> bool {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state & (READY | CANCELLED) == READY).then_some(state | REQUESTED)
            })
            .is_ok()
    }
    pub(crate) fn start_requested(&self) -> bool {
        self.0.load(Ordering::Acquire) & REQUESTED != 0
    }
    /// A late native success is recorded but cannot undo cancellation.
    pub(crate) fn started(&self) {
        self.0.fetch_or(STARTED, Ordering::Release);
    }
    pub(crate) fn is_started(&self) -> bool {
        self.0.load(Ordering::Acquire) & STARTED != 0
    }
    pub(crate) fn alive(&self) -> bool {
        self.0.load(Ordering::Acquire) & CANCELLED == 0
    }
    pub(crate) fn activate(&self, progress: u64) -> bool {
        if progress == 0 {
            return false;
        }
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state & (READY | STARTED | CANCELLED) == (READY | STARTED))
                    .then_some(state | PROGRAM)
            })
            .is_ok()
    }
    pub(crate) fn program(&self) -> bool {
        self.0.load(Ordering::Acquire) & (PROGRAM | CANCELLED) == PROGRAM
    }
    pub(crate) fn cancel(&self) {
        self.0.fetch_or(CANCELLED, Ordering::AcqRel);
    }
}

/// Opaque endpoint IDs are compared exactly. Native open additionally checks
/// IMMDevice::GetId against the selection, rejecting aliases before Start.
pub(crate) struct Reservations {
    slots: [Option<String>; MAX_RESERVATIONS],
}
impl Reservations {
    pub(crate) const fn new() -> Self {
        Self {
            slots: [const { None }; MAX_RESERVATIONS],
        }
    }
    pub(crate) fn reserve(&mut self, id: &str) -> Option<usize> {
        if self.slots.iter().flatten().any(|held| held == id) {
            return None;
        }
        let index = self.slots.iter().position(Option::is_none)?;
        self.slots[index] = Some(id.to_owned());
        Some(index)
    }
    pub(crate) fn release(&mut self, index: usize) {
        self.slots[index] = None;
    }
}
pub(crate) static RESERVATIONS: Mutex<Reservations> = Mutex::new(Reservations::new());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WasapiOutputPhase {
    OpenRequested,
    OpenReturned,
    PrefillRequested,
    PrefillReturned,
    StartRequested,
    StartReturned,
    StopRequested,
    StopReturned,
    NativeReleaseRequested,
    NativeReleaseReturned,
    OwnerCompleted,
    ReservationRetained,
}
pub const WASAPI_OUTPUT_PHASES: usize = 12;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WasapiOutputEvent {
    pub phase: WasapiOutputPhase,
    pub qpc_ticks: u64,
    pub status: Option<i32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WasapiOutputOperation {
    Open = 1,
    Prefill,
    ResetEvent,
    Start,
    Wait,
    Padding,
    Acquire,
    Release,
    Stop,
    NativeRelease,
    Clock,
    Validation,
}
impl WasapiOutputOperation {
    fn decode(value: u64) -> Self {
        match value {
            2 => Self::Prefill,
            3 => Self::ResetEvent,
            4 => Self::Start,
            5 => Self::Wait,
            6 => Self::Padding,
            7 => Self::Acquire,
            8 => Self::Release,
            9 => Self::Stop,
            10 => Self::NativeRelease,
            11 => Self::Clock,
            12 => Self::Validation,
            _ => Self::Open,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WasapiOutputCall {
    pub operation: WasapiOutputOperation,
    /// For Validation, None means an adapter rejection without a native HRESULT.
    /// Otherwise None means entered without an observed return; Some retains HRESULT.
    pub status: Option<i32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WasapiOutputCompletion {
    Pending,
    Completed,
    ReleasedUnproven,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WasapiOutputWork {
    pub serial: u64,
    pub entry_qpc_ticks: u64,
    pub exit_qpc_ticks: u64,
    pub frames: u32,
    /// Release flag, unavailable when no buffer was acquired.
    pub silent: Option<bool>,
    /// None when acquisition failed and no release was owed.
    pub release_status: Option<i32>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasapiOutputSnapshot {
    pub epoch: u64,
    pub qpc_frequency: u64,
    pub maximum_frames: u32,
    /// Successful post-Start ReleaseBuffer calls, including silence; not playback.
    pub progress_count: u64,
    pub submitted_pcm_frames: u64,
    pub submitted_silent_frames: u64,
    pub prefill_frames: u64,
    pub submitted_version: Option<u64>,
    pub last_call: Option<WasapiOutputCall>,
    pub first_failure: Option<WasapiOutputCall>,
    pub start_status: Option<i32>,
    pub stop_status: Option<i32>,
    pub release_status: Option<i32>,
    pub last_work: Option<WasapiOutputWork>,
    pub work_pending_or_contended: bool,
    pub lifecycle: [Option<WasapiOutputEvent>; WASAPI_OUTPUT_PHASES],
    pub completion: WasapiOutputCompletion,
}
#[derive(Default)]
struct Event {
    ready: AtomicU32,
    ticks: AtomicU64,
    status: AtomicU64,
}
fn status_word(value: Option<i32>) -> u64 {
    value.map_or(0, |v| (1u64 << 32) | u64::from(v as u32))
}
fn status_value(value: u64) -> Option<i32> {
    (value & (1 << 32) != 0).then_some(value as u32 as i32)
}
fn call_word(operation: WasapiOutputOperation, status: Option<i32>) -> u64 {
    status_word(status) | ((operation as u64) << 40)
}
fn call_value(value: u64) -> Option<WasapiOutputCall> {
    (value != 0).then(|| WasapiOutputCall {
        operation: WasapiOutputOperation::decode(value >> 40),
        status: status_value(value),
    })
}

#[derive(Default)]
pub(crate) struct WasapiOutputTiming {
    epoch: AtomicU64,
    frequency: AtomicU64,
    maximum_frames: AtomicU32,
    progress: AtomicU64,
    pcm: AtomicU64,
    silent: AtomicU64,
    prefill: AtomicU64,
    version: AtomicU64,
    version_present: AtomicU32,
    call: AtomicU64,
    failure: AtomicU64,
    start: AtomicU64,
    stop: AtomicU64,
    release: AtomicU64,
    events: [Event; WASAPI_OUTPUT_PHASES],
    completion: AtomicU32,
    cleanup_failed: AtomicU32,
    sequence: AtomicU64,
    entry: AtomicU64,
    exit: AtomicU64,
    frames: AtomicU32,
    flags: AtomicU32,
    work_status: AtomicU64,
}
impl WasapiOutputTiming {
    pub(crate) fn new() -> Self {
        Self::default()
    }
    pub(crate) fn cleanup_error(&self, status: i32) {
        self.cleanup_failed.store(1, Ordering::Release);
        self.failed(WasapiOutputOperation::NativeRelease, status);
    }
    pub(crate) fn cleanup_proven(&self) -> bool {
        self.cleanup_failed.load(Ordering::Acquire) == 0
    }
    pub(crate) fn configure(&self, epoch: u64, frequency: u64, maximum_frames: u32) {
        self.epoch.store(epoch, Ordering::Relaxed);
        self.frequency.store(frequency, Ordering::Relaxed);
        self.maximum_frames.store(maximum_frames, Ordering::Release);
    }
    pub(crate) fn event(&self, phase: WasapiOutputPhase, ticks: u64, status: Option<i32>) {
        let slot = &self.events[phase as usize];
        if slot
            .ready
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        slot.ticks.store(ticks, Ordering::Relaxed);
        slot.status.store(status_word(status), Ordering::Relaxed);
        slot.ready.store(2, Ordering::Release);
    }
    pub(crate) fn entered(&self, operation: WasapiOutputOperation) {
        self.call
            .store(call_word(operation, None), Ordering::Release);
    }
    pub(crate) fn returned(&self, operation: WasapiOutputOperation, status: i32) {
        self.call
            .store(call_word(operation, Some(status)), Ordering::Release);
        let slot = match operation {
            WasapiOutputOperation::Start => Some(&self.start),
            WasapiOutputOperation::Stop => Some(&self.stop),
            WasapiOutputOperation::Release => Some(&self.release),
            _ => None,
        };
        if let Some(slot) = slot {
            slot.store(status_word(Some(status)), Ordering::Release);
        }
        if status < 0 {
            if operation == WasapiOutputOperation::Release {
                self.cleanup_failed.store(1, Ordering::Release);
            }
            self.failed(operation, status);
        }
    }
    pub(crate) fn failed(&self, operation: WasapiOutputOperation, status: i32) {
        let _ = self.failure.compare_exchange(
            0,
            call_word(operation, Some(status)),
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
    }
    pub(crate) fn validation_failed(&self) {
        let _ = self.failure.compare_exchange(
            0,
            call_word(WasapiOutputOperation::Validation, None),
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
    }
    pub(crate) fn failure(&self) -> Option<WasapiOutputCall> {
        call_value(self.failure.load(Ordering::Acquire))
    }
    pub(crate) fn progress_count(&self) -> u64 {
        self.progress.load(Ordering::Acquire)
    }
    pub(crate) fn begin_work(&self, ticks: u64) {
        // Sole native owner writes; odd sequence exposes unfinished/blocked work.
        self.sequence.fetch_add(1, Ordering::SeqCst);
        self.entry.store(ticks, Ordering::SeqCst);
    }
    pub(crate) fn finish_work(&self, ticks: u64, frames: u32, silent: bool, status: Option<i32>) {
        self.exit.store(ticks, Ordering::SeqCst);
        self.frames.store(frames, Ordering::SeqCst);
        self.flags.store(u32::from(silent), Ordering::SeqCst);
        self.work_status
            .store(status_word(status), Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }
    /// Sole owner calls only after complete native release has returned success.
    #[cfg(test)]
    pub(crate) fn submitted(&self, frames: u32, version: Option<u64>, prefill: bool, status: i32) {
        self.submitted_pcm(frames, version.is_some(), version, prefill, status)
    }
    pub(crate) fn submitted_pcm(
        &self,
        frames: u32,
        pcm: bool,
        version: Option<u64>,
        prefill: bool,
        status: i32,
    ) {
        if status < 0 {
            return;
        }
        if prefill {
            self.prefill.fetch_add(u64::from(frames), Ordering::Relaxed);
            return;
        }
        if pcm {
            self.pcm.fetch_add(u64::from(frames), Ordering::Relaxed);
            if let Some(version) = version {
                self.version.store(version, Ordering::Release);
                self.version_present.store(1, Ordering::Release);
            }
        } else {
            self.silent.fetch_add(u64::from(frames), Ordering::Relaxed);
        }
        self.progress.fetch_add(1, Ordering::Release);
    }
    pub(crate) fn released(&self, proven: bool) {
        self.completion
            .store(if proven { 1 } else { 2 }, Ordering::Release);
    }
    pub(crate) fn snapshot(&self) -> WasapiOutputSnapshot {
        const PHASES: [WasapiOutputPhase; WASAPI_OUTPUT_PHASES] = [
            WasapiOutputPhase::OpenRequested,
            WasapiOutputPhase::OpenReturned,
            WasapiOutputPhase::PrefillRequested,
            WasapiOutputPhase::PrefillReturned,
            WasapiOutputPhase::StartRequested,
            WasapiOutputPhase::StartReturned,
            WasapiOutputPhase::StopRequested,
            WasapiOutputPhase::StopReturned,
            WasapiOutputPhase::NativeReleaseRequested,
            WasapiOutputPhase::NativeReleaseReturned,
            WasapiOutputPhase::OwnerCompleted,
            WasapiOutputPhase::ReservationRetained,
        ];
        let before = self.sequence.load(Ordering::SeqCst);
        let work = WasapiOutputWork {
            serial: before / 2,
            entry_qpc_ticks: self.entry.load(Ordering::SeqCst),
            exit_qpc_ticks: self.exit.load(Ordering::SeqCst),
            frames: self.frames.load(Ordering::SeqCst),
            silent: status_value(self.work_status.load(Ordering::SeqCst))
                .map(|_| self.flags.load(Ordering::SeqCst) != 0),
            release_status: status_value(self.work_status.load(Ordering::SeqCst)),
        };
        let after = self.sequence.load(Ordering::SeqCst);
        let contended = before & 1 != 0 || before != after;
        WasapiOutputSnapshot {
            epoch: self.epoch.load(Ordering::Acquire),
            qpc_frequency: self.frequency.load(Ordering::Acquire),
            maximum_frames: self.maximum_frames.load(Ordering::Acquire),
            progress_count: self.progress_count(),
            submitted_pcm_frames: self.pcm.load(Ordering::Acquire),
            submitted_silent_frames: self.silent.load(Ordering::Acquire),
            prefill_frames: self.prefill.load(Ordering::Acquire),
            submitted_version: (self.version_present.load(Ordering::Acquire) != 0)
                .then(|| self.version.load(Ordering::Acquire)),
            last_call: call_value(self.call.load(Ordering::Acquire)),
            first_failure: self.failure(),
            start_status: status_value(self.start.load(Ordering::Acquire)),
            stop_status: status_value(self.stop.load(Ordering::Acquire)),
            release_status: status_value(self.release.load(Ordering::Acquire)),
            last_work: (!contended && before != 0).then_some(work),
            work_pending_or_contended: contended,
            lifecycle: std::array::from_fn(|index| {
                let event = &self.events[index];
                (event.ready.load(Ordering::Acquire) == 2).then(|| WasapiOutputEvent {
                    phase: PHASES[index],
                    qpc_ticks: event.ticks.load(Ordering::Relaxed),
                    status: status_value(event.status.load(Ordering::Relaxed)),
                })
            }),
            completion: match self.completion.load(Ordering::Acquire) {
                1 => WasapiOutputCompletion::Completed,
                2 => WasapiOutputCompletion::ReleasedUnproven,
                _ => WasapiOutputCompletion::Pending,
            },
        }
    }
}
#[cfg(test)]
#[path = "wasapi_output_state_tests.rs"]
mod tests;
