//! Portable, allocation-free WASAPI packet semantics. Native code borrows a
//! packet only between GetBuffer and ReleaseBuffer; SILENT never borrows PCM.
use super::{AudioFault, MAX_CALLBACK_FRAMES, MAX_DEVICE_CHANNELS};

pub(crate) const DISCONTINUITY: u32 = 1;
pub(crate) const SILENT: u32 = 2;
pub(crate) const TIMESTAMP_ERROR: u32 = 4;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Encoding {
    F32,
    F64,
    I16,
    I24In32,
    I32,
    U8,
}
impl Encoding {
    pub(crate) fn from_cpal(format: cpal::SampleFormat) -> Option<Self> {
        Some(match format {
            cpal::SampleFormat::F32 => Self::F32,
            cpal::SampleFormat::F64 => Self::F64,
            cpal::SampleFormat::I16 => Self::I16,
            cpal::SampleFormat::I24 => Self::I24In32,
            cpal::SampleFormat::I32 => Self::I32,
            cpal::SampleFormat::U8 => Self::U8,
            _ => return None,
        })
    }
    pub(crate) fn bytes(self) -> usize {
        match self {
            Self::F64 => 8,
            Self::I16 => 2,
            Self::U8 => 1,
            _ => 4,
        }
    }
    fn decode(self, bytes: &[u8]) -> f32 {
        let sample = match self {
            Self::F32 => f32::from_le_bytes(bytes[..4].try_into().expect("validated sample width")),
            Self::F64 => {
                f64::from_le_bytes(bytes[..8].try_into().expect("validated sample width")) as f32
            }
            Self::I16 => {
                f32::from(i16::from_le_bytes(
                    bytes[..2].try_into().expect("validated sample width"),
                )) / 32768.0
            }
            Self::I24In32 => {
                (i32::from_le_bytes(bytes[..4].try_into().expect("validated sample width")) >> 8)
                    as f32
                    / 8_388_608.0
            }
            Self::I32 => {
                i32::from_le_bytes(bytes[..4].try_into().expect("validated sample width")) as f32
                    / 2_147_483_648.0
            }
            Self::U8 => (f32::from(bytes[0]) - 128.0) / 128.0,
        };
        if sample.is_finite() {
            sample
        } else {
            0.0
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub(crate) channels: u16,
    pub(crate) encoding: Encoding,
}
impl Layout {
    pub(crate) fn bytes(self, frames: usize) -> Result<usize, AudioFault> {
        if !(1..=MAX_DEVICE_CHANNELS).contains(&self.channels) || frames > MAX_CALLBACK_FRAMES {
            return Err(AudioFault::InvalidBuffer);
        }
        frames
            .checked_mul(usize::from(self.channels))
            .and_then(|n| n.checked_mul(self.encoding.bytes()))
            .ok_or(AudioFault::InvalidBuffer)
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Metadata {
    pub(crate) frames: usize,
    pub(crate) flags: u32,
    pub(crate) position: u64,
    pub(crate) qpc_100ns: u64,
}
#[derive(Default)]
pub(crate) struct Tracker {
    expected_position: Option<u64>,
    last_qpc: Option<u64>,
}
impl Tracker {
    pub(crate) fn accept<'a>(
        &mut self,
        meta: Metadata,
        layout: Layout,
        bytes: Option<&'a [u8]>,
    ) -> Result<Packet<'a>, AudioFault> {
        let length = layout.bytes(meta.frames)?;
        if meta.frames == 0 {
            return Err(AudioFault::InvalidBuffer);
        }
        if meta.flags & TIMESTAMP_ERROR != 0 {
            return Err(AudioFault::CaptureTimestamp);
        }
        // WASAPI's first packet at device position zero may carry the initial
        // discontinuity flag. Every later flag or frame-position gap is a fault.
        let initial_zero = self.expected_position.is_none() && meta.position == 0;
        if (!initial_zero && meta.flags & DISCONTINUITY != 0)
            || self
                .expected_position
                .is_some_and(|expected| meta.position != expected)
        {
            return Err(AudioFault::Xrun);
        }
        if self
            .last_qpc
            .is_some_and(|previous| meta.qpc_100ns <= previous)
        {
            return Err(AudioFault::CaptureTimestamp);
        }
        let next_position = meta
            .position
            .checked_add(meta.frames as u64)
            .ok_or(AudioFault::CaptureTimestamp)?;
        let data = if meta.flags & SILENT != 0 {
            None
        } else {
            let data = bytes.ok_or(AudioFault::InvalidBuffer)?;
            if data.len() != length {
                return Err(AudioFault::InvalidBuffer);
            }
            Some(data)
        };
        self.expected_position = Some(next_position);
        self.last_qpc = Some(meta.qpc_100ns);
        Ok(Packet {
            data,
            layout,
            frames: meta.frames,
        })
    }
}
pub(crate) struct Packet<'a> {
    data: Option<&'a [u8]>,
    layout: Layout,
    pub(crate) frames: usize,
}
impl Packet<'_> {
    pub(crate) fn frame(&self, frame: usize) -> [f32; 2] {
        debug_assert!(frame < self.frames);
        let Some(data) = self.data else {
            return [0.0; 2];
        };
        let width = self.layout.encoding.bytes();
        let start = frame * usize::from(self.layout.channels) * width;
        let right = start + usize::from(self.layout.channels > 1) * width;
        [
            self.layout.encoding.decode(&data[start..start + width]),
            self.layout.encoding.decode(&data[right..right + width]),
        ]
    }
}

/// Every acquired, nonempty native packet is released once, including early
/// returns. Calling finish reports release failure without a second attempt.
pub(crate) struct ReleaseGuard<F: FnOnce(u32) -> Result<(), AudioFault>> {
    frames: u32,
    release: Option<F>,
}
impl<F: FnOnce(u32) -> Result<(), AudioFault>> ReleaseGuard<F> {
    pub(crate) fn new(frames: u32, release: F) -> Self {
        Self {
            frames,
            release: Some(release),
        }
    }
    pub(crate) fn finish(mut self) -> Result<(), AudioFault> {
        self.release.take().expect("packet release present")(self.frames)
    }
}
impl<F: FnOnce(u32) -> Result<(), AudioFault>> Drop for ReleaseGuard<F> {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release(self.frames);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata(flags: u32) -> Metadata {
        Metadata {
            frames: 2,
            flags,
            position: 0,
            qpc_100ns: 1000,
        }
    }
    fn layout() -> Layout {
        Layout {
            channels: 2,
            encoding: Encoding::F32,
        }
    }
    #[test]
    fn native_encoding_selection_and_packet_to_ring_are_allocation_free() {
        assert!(Encoding::from_cpal(cpal::SampleFormat::F32).is_some());
        assert!(Encoding::from_cpal(cpal::SampleFormat::F64).is_some());
        assert!(Encoding::from_cpal(cpal::SampleFormat::I16).is_some());
        assert!(Encoding::from_cpal(cpal::SampleFormat::I24).is_some());
        assert!(Encoding::from_cpal(cpal::SampleFormat::I32).is_some());
        assert!(Encoding::from_cpal(cpal::SampleFormat::U8).is_some());
        assert!(Encoding::from_cpal(cpal::SampleFormat::DsdU8).is_none());
        let session = ravelink_core::Session::new("WASAPI packet", 48000);
        let ring = crate::capture_transport::Ring::new();
        let shared = std::sync::Arc::new(crate::Shared {
            active_gate: arc_swap::ArcSwapOption::empty(),
            state: std::sync::Mutex::new(crate::ControlState::default()),
            epoch: std::sync::atomic::AtomicU64::new(0),
        });
        let telemetry = std::sync::Arc::new(crate::Telemetry::new(&session));
        let mut capture = crate::capture::CaptureState::new(
            ring.clone(),
            0,
            2,
            0,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            shared,
            telemetry,
        );
        let data: Vec<u8> = [0.25_f32, -0.125, 0.5, -0.25]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let mut tracker = Tracker::default();
        crate::tests::OPERATIONS.with(|n| n.set(0));
        crate::tests::COUNTING.with(|on| on.set(true));
        let packet = tracker.accept(metadata(0), layout(), Some(&data)).unwrap();
        capture.push_frames(packet.frames, |index| packet.frame(index));
        let packet = tracker
            .accept(
                Metadata {
                    flags: SILENT,
                    position: 2,
                    qpc_100ns: 2000,
                    ..metadata(0)
                },
                layout(),
                None,
            )
            .unwrap();
        capture.push_frames(packet.frames, |index| packet.frame(index));
        crate::tests::COUNTING.with(|on| on.set(false));
        assert_eq!(crate::tests::OPERATIONS.with(std::cell::Cell::get), 0);
        assert_eq!(ring.test_sample(0), [0.25, -0.125]);
        assert_eq!(ring.test_sample(1), [0.5, -0.25]);
        assert_eq!(ring.test_sample(2), [0.0; 2]);
        assert_eq!(ring.test_sample(3), [0.0; 2]);
    }
    #[test]
    fn silent_flag_ignores_nonzero_short_or_absent_pcm() {
        for bytes in [Some(&[255_u8; 16][..]), Some(&[255_u8; 1][..]), None] {
            let mut tracker = Tracker::default();
            let packet = tracker.accept(metadata(SILENT), layout(), bytes).unwrap();
            assert_eq!(packet.frame(0), [0.0; 2]);
            assert_eq!(packet.frame(1), [0.0; 2]);
        }
        assert!(matches!(
            Tracker::default().accept(metadata(0), layout(), None),
            Err(AudioFault::InvalidBuffer)
        ));
    }
    #[test]
    fn discontinuity_after_initial_zero_and_position_gap_stop_capture() {
        let mut tracker = Tracker::default();
        assert!(tracker
            .accept(metadata(DISCONTINUITY | SILENT), layout(), None)
            .is_ok());
        for (position, flags) in [(2, DISCONTINUITY | SILENT), (3, SILENT)] {
            assert!(matches!(
                tracker.accept(
                    Metadata {
                        position,
                        flags,
                        qpc_100ns: 2000,
                        ..metadata(0)
                    },
                    layout(),
                    None
                ),
                Err(AudioFault::Xrun)
            ));
        }
        assert!(tracker
            .accept(
                Metadata {
                    position: 2,
                    flags: SILENT,
                    qpc_100ns: 2000,
                    ..metadata(0)
                },
                layout(),
                None
            )
            .is_ok());
    }
    #[test]
    fn timestamp_error_and_nonmonotonic_qpc_do_not_publish_a_packet() {
        let mut tracker = Tracker::default();
        assert!(matches!(
            tracker.accept(metadata(TIMESTAMP_ERROR | SILENT), layout(), None),
            Err(AudioFault::CaptureTimestamp)
        ));
        tracker.accept(metadata(SILENT), layout(), None).unwrap();
        assert!(matches!(
            tracker.accept(
                Metadata {
                    position: 2,
                    ..metadata(SILENT)
                },
                layout(),
                None
            ),
            Err(AudioFault::CaptureTimestamp)
        ));
        assert!(tracker
            .accept(
                Metadata {
                    position: 2,
                    qpc_100ns: 2000,
                    ..metadata(SILENT)
                },
                layout(),
                None
            )
            .is_ok());
    }
    #[test]
    fn pcm_decoding_obeys_signedness_valid_bits_and_channel_mapping() {
        let cases = [
            (Encoding::U8, vec![0, 128], [-1.0, 0.0]),
            (Encoding::I16, vec![0, 128, 0, 64], [-1.0, 0.5]),
            (
                Encoding::I24In32,
                vec![255, 255, 255, 127, 255, 0, 0, 128],
                [8388607.0 / 8388608.0, -1.0],
            ),
            (Encoding::I32, vec![0, 0, 0, 128, 0, 0, 0, 64], [-1.0, 0.5]),
        ];
        for (encoding, bytes, expected) in cases {
            let packet = Tracker::default()
                .accept(
                    Metadata {
                        frames: 1,
                        ..metadata(0)
                    },
                    Layout {
                        channels: 2,
                        encoding,
                    },
                    Some(&bytes),
                )
                .unwrap();
            assert_eq!(packet.frame(0), expected);
        }
        let bytes: Vec<_> = [0.25_f64, -0.5]
            .into_iter()
            .flat_map(f64::to_le_bytes)
            .collect();
        let packet = Tracker::default()
            .accept(
                metadata(0),
                Layout {
                    channels: 1,
                    encoding: Encoding::F64,
                },
                Some(&bytes),
            )
            .unwrap();
        assert_eq!(packet.frame(0), [0.25; 2]);
        assert_eq!(packet.frame(1), [-0.5; 2]);
    }
    #[test]
    fn acquired_packet_releases_once_on_success_processing_error_and_release_error() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let release = |frames| {
            assert_eq!(frames, 128);
            calls.set(calls.get() + 1);
            Ok(())
        };
        ReleaseGuard::new(128, release).finish().unwrap();
        assert_eq!(calls.get(), 1);
        drop(ReleaseGuard::new(128, release));
        assert_eq!(calls.get(), 2);
        let failed = ReleaseGuard::new(128, |_| {
            calls.set(calls.get() + 1);
            Err(AudioFault::DeviceUnavailable)
        });
        assert_eq!(failed.finish(), Err(AudioFault::DeviceUnavailable));
        assert_eq!(calls.get(), 3);
    }
}
