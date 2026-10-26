use crate::AudioFault;

/// Unmodified CoreAudio input timestamp fields plus this adapter's packet and
/// accepted-ring positions. Host ticks are retained without clock conversion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeCapturePacket {
    pub packet_index: u64,
    pub queue_frame: u64,
    pub sample_time: f64,
    pub host_time: u64,
    pub observed_host_time: u64,
    pub rate_scalar: f64,
    pub flags: u32,
    pub frames: u32,
    pub epoch: u64,
}

/// The first rejected packet and the last previously accepted packet. Later
/// callbacks cannot replace this evidence, even if they carry valid audio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeCaptureFault {
    pub fault: AudioFault,
    pub previous: Option<NativeCapturePacket>,
    pub current: NativeCapturePacket,
}

/// Result of the most recently completed, authorized native callback. A
/// cancelled callback does not publish its PCM to the transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeCallbackOutcome {
    Published,
    Cancelled,
    Rejected(AudioFault),
}

/// Host-clock observations around the original callback work. Completion is
/// sampled immediately before publishing this fixed record and returning;
/// it is not an observation made after the OS receives control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeCallbackCompletion {
    pub packet_index: u64,
    pub entry_host_time: u64,
    pub after_pcm_host_time: u64,
    pub completion_host_time: u64,
    pub os_now_sample_time: f64,
    pub os_now_host_time: u64,
    pub os_now_flags: u32,
    pub outcome: NativeCallbackOutcome,
}

/// Each phase has one fixed slot per capture owner. These observations retain
/// the request/return boundary when an OS operation blocks or fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NativeLifecyclePhase {
    CreateRequested,
    CreateReturned,
    StartRequested,
    StartReturned,
    HandleCancelled,
    GatesRevoked,
    StopRequested,
    StopReturned,
    DestroyRequested,
    DestroyReturned,
    ListenersRemovalRequested,
    ListenersRemovalReturned,
    QuiescenceChecked,
    ContextRetained,
    OwnerCompleted,
}
pub const NATIVE_LIFECYCLE_PHASES: usize = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeLifecycleEvent {
    pub phase: NativeLifecyclePhase,
    pub host_time: u64,
    /// Unmodified OSStatus for native call returns. Listener removal retains
    /// the first nonzero status, or zero when all removals succeeded.
    pub status: Option<i32>,
    pub callbacks_in_flight: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeTimingSnapshot {
    pub packets_seen: u64,
    pub last_packet: Option<NativeCapturePacket>,
    pub first_fault: Option<NativeCaptureFault>,
    /// A concurrent callback prevented a coherent last-packet read. The reader
    /// never spins, and first-fault evidence remains independently available.
    pub last_packet_contended: bool,
    pub last_completed_callback: Option<NativeCallbackCompletion>,
    pub callback_completion_contended: bool,
    /// Fixed phase order, not chronological order. A missing return phase
    /// means that operation has not returned at the time of this observation.
    pub lifecycle: [Option<NativeLifecycleEvent>; NATIVE_LIFECYCLE_PHASES],
}
