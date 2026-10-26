//! First-winner fault identity is one atomic word. The winning writer publishes
//! fixed metadata once; readers never wait, spin, or read a partially written
//! record. A suspended winner still stops audio and exposes its coupled identity.
use crate::AudioFault;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FaultDirection {
    Runtime = 0,
    Input = 1,
    Output = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FaultOrigin {
    Runtime = 0,
    NativeInputPacket = 1,
    NativeInputReentry = 2,
    NativeInputProperty = 3,
    NativeInputOverload = 4,
    NativeInputLifecycle = 5,
    CpalOutputError = 6,
    /// On pinned CPAL/CoreAudio, Xrun comes from ProcessorOverload listeners.
    /// The native object/property address is not forwarded by CPAL.
    CpalCoreAudioOutputOverload = 7,
    CaptureTransportProducer = 8,
    CaptureTransportConsumer = 9,
    CaptureStartup = 10,
    OutputBuffer = 11,
    MixerControls = 12,
    MixerRender = 13,
    WasapiInput = 14,
    CpalInputError = 15,
    NativeOutputPacket = 16,
    NativeOutputReentry = 17,
    NativeOutputProperty = 18,
    NativeOutputOverload = 19,
    NativeOutputLifecycle = 20,
    NativeOutputStartup = 21,
    WasapiOutput = 22,
    NativeOutputProgress = 23,
}
impl FaultOrigin {
    fn decode(value: u8) -> Self {
        match value {
            1 => Self::NativeInputPacket,
            2 => Self::NativeInputReentry,
            3 => Self::NativeInputProperty,
            4 => Self::NativeInputOverload,
            5 => Self::NativeInputLifecycle,
            6 => Self::CpalOutputError,
            7 => Self::CpalCoreAudioOutputOverload,
            8 => Self::CaptureTransportProducer,
            9 => Self::CaptureTransportConsumer,
            10 => Self::CaptureStartup,
            11 => Self::OutputBuffer,
            12 => Self::MixerControls,
            13 => Self::MixerRender,
            14 => Self::WasapiInput,
            15 => Self::CpalInputError,
            16 => Self::NativeOutputPacket,
            17 => Self::NativeOutputReentry,
            18 => Self::NativeOutputProperty,
            19 => Self::NativeOutputOverload,
            20 => Self::NativeOutputLifecycle,
            21 => Self::NativeOutputStartup,
            22 => Self::WasapiOutput,
            23 => Self::NativeOutputProgress,
            _ => Self::Runtime,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultProperty {
    pub selector: u32,
    pub scope: u32,
    pub element: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FaultMetadata {
    pub epoch: u64,
    /// Raw AudioGetCurrentHostTime ticks on macOS, unavailable elsewhere.
    /// This is sampled at observation, not a claim of physical fault onset.
    pub observed_mach_host_time: Option<u64>,
    pub native_object: Option<u32>,
    pub property: Option<FaultProperty>,
    /// Native OSStatus or HRESULT where the reporting API exposes it.
    pub status: Option<i32>,
    /// Links a native input packet fault to its retained native timing pair.
    pub packet_index: Option<u64>,
}
impl FaultMetadata {
    pub(crate) fn now(epoch: u64) -> Self {
        Self {
            epoch,
            observed_mach_host_time: {
                #[cfg(target_os = "macos")]
                {
                    // CoreAudio declares this CA_REALTIME_API. No OS object
                    // query, allocation, or formatting occurs in this call.
                    Some(unsafe { objc2_core_audio::AudioGetCurrentHostTime() })
                }
                #[cfg(not(target_os = "macos"))]
                {
                    None
                }
            },
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultAttribution {
    pub fault: AudioFault,
    pub origin: FaultOrigin,
    pub direction: FaultDirection,
    /// Index in the runtime's sorted unique capture list, or output zero. None
    /// denotes a runtime-wide fault. This identity is coupled to the fault code.
    pub endpoint_index: Option<u32>,
    /// Host-qualified identity resolved off callback; None if unavailable.
    pub endpoint_id: Option<String>,
    /// None means the winning callback has not finished publishing metadata.
    /// A later fault never fills, replaces, or repairs the winner's record.
    pub metadata: Option<FaultMetadata>,
}

#[derive(Clone, Copy)]
pub(crate) struct FaultSource {
    pub(crate) direction: FaultDirection,
    pub(crate) index: u32,
}
impl FaultSource {
    pub(crate) const RUNTIME: Self = Self {
        direction: FaultDirection::Runtime,
        index: 0,
    };
    pub(crate) const OUTPUT: Self = Self {
        direction: FaultDirection::Output,
        index: 0,
    };
    pub(crate) fn input(index: usize) -> Self {
        // The graph budget bounds this far below u32::MAX before construction.
        Self {
            direction: FaultDirection::Input,
            index: index as u32,
        }
    }
}

#[derive(Default)]
pub(crate) struct FaultLatch {
    identity: AtomicU64,
    ready: AtomicBool,
    epoch: AtomicU64,
    host: AtomicU64,
    object: AtomicU32,
    selector: AtomicU32,
    scope: AtomicU32,
    element: AtomicU32,
    status: AtomicU32,
    packet: AtomicU64,
    present: AtomicU32,
}
impl FaultLatch {
    pub(crate) fn load(&self, ordering: Ordering) -> u32 {
        (self.identity.load(ordering) & 0xff) as u32
    }
    fn claim(&self, fault: AudioFault, origin: FaultOrigin, source: FaultSource) -> bool {
        if fault == AudioFault::None {
            return false;
        }
        let word = fault as u64
            | (origin as u64) << 8
            | (source.direction as u64) << 16
            | (u64::from(source.index) << 24);
        self.identity
            .compare_exchange(0, word, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }
    pub(crate) fn record(
        &self,
        fault: AudioFault,
        origin: FaultOrigin,
        source: FaultSource,
        metadata: FaultMetadata,
    ) {
        if !self.claim(fault, origin, source) {
            return;
        }
        self.publish(metadata);
    }
    fn publish(&self, metadata: FaultMetadata) {
        self.epoch.store(metadata.epoch, Ordering::Relaxed);
        self.host.store(
            metadata.observed_mach_host_time.unwrap_or(0),
            Ordering::Relaxed,
        );
        self.object
            .store(metadata.native_object.unwrap_or(0), Ordering::Relaxed);
        if let Some(property) = metadata.property {
            self.selector.store(property.selector, Ordering::Relaxed);
            self.scope.store(property.scope, Ordering::Relaxed);
            self.element.store(property.element, Ordering::Relaxed);
        }
        self.status
            .store(metadata.status.unwrap_or(0) as u32, Ordering::Relaxed);
        self.packet
            .store(metadata.packet_index.unwrap_or(0), Ordering::Relaxed);
        self.present.store(
            u32::from(metadata.observed_mach_host_time.is_some())
                | u32::from(metadata.native_object.is_some()) << 1
                | u32::from(metadata.property.is_some()) << 2
                | u32::from(metadata.status.is_some()) << 3
                | u32::from(metadata.packet_index.is_some()) << 4,
            Ordering::Relaxed,
        );
        self.ready.store(true, Ordering::Release);
    }
    pub(crate) fn snapshot(&self) -> Option<FaultAttribution> {
        let word = self.identity.load(Ordering::Acquire);
        if word == 0 {
            return None;
        }
        let direction = match (word >> 16) & 0xff {
            1 => FaultDirection::Input,
            2 => FaultDirection::Output,
            _ => FaultDirection::Runtime,
        };
        let metadata = self.ready.load(Ordering::Acquire).then(|| {
            let present = self.present.load(Ordering::Relaxed);
            FaultMetadata {
                epoch: self.epoch.load(Ordering::Relaxed),
                observed_mach_host_time: (present & 1 != 0)
                    .then(|| self.host.load(Ordering::Relaxed)),
                native_object: (present & 2 != 0).then(|| self.object.load(Ordering::Relaxed)),
                property: (present & 4 != 0).then(|| FaultProperty {
                    selector: self.selector.load(Ordering::Relaxed),
                    scope: self.scope.load(Ordering::Relaxed),
                    element: self.element.load(Ordering::Relaxed),
                }),
                status: (present & 8 != 0).then(|| self.status.load(Ordering::Relaxed) as i32),
                packet_index: (present & 16 != 0).then(|| self.packet.load(Ordering::Relaxed)),
            }
        });
        Some(FaultAttribution {
            fault: AudioFault::from_code((word & 0xff) as u32),
            origin: FaultOrigin::decode((word >> 8) as u8),
            direction,
            endpoint_index: (direction != FaultDirection::Runtime).then_some((word >> 24) as u32),
            endpoint_id: None,
            metadata,
        })
    }
}

#[cfg(test)]
#[path = "fault_tests.rs"]
mod tests;
