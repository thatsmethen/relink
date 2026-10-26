//! Original portable CoreAudio PCM encoding. Native pointer validation, stream
//! usage, timestamps and callback ownership belong to the future native owner.
//! Every supplied plane is enabled and safely, exclusively borrowed here.
use crate::{
    native_packet::{Encoding, Format, Layout, Plane},
    output_sink::{InvalidOutputBuffer, StereoSink},
    AudioFault,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidRate,
    InvalidFormat(AudioFault),
    InsufficientChannels(u16),
    InvalidBuffers(AudioFault),
    NonSequential {
        expected: usize,
        actual: usize,
    },
    LengthMismatch {
        left: usize,
        right: usize,
    },
    Extent {
        start: usize,
        frames: usize,
        capacity: usize,
    },
    Incomplete {
        written: usize,
        total: usize,
    },
}

/// Created off the callback. Uses the same conservative supported-ASBD policy
/// as capture, with at least two total channels and a positive exact rate.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct OutputLayout(Layout);
impl OutputLayout {
    pub(crate) fn native_layout(&self) -> &Layout {
        &self.0
    }
    pub(crate) fn new(formats: &[Format], rate: u32) -> Result<Self, Error> {
        if rate == 0 {
            return Err(Error::InvalidRate);
        }
        let layout = Layout::new(formats, rate).map_err(Error::InvalidFormat)?;
        if layout.channels < 2 {
            return Err(Error::InsufficientChannels(layout.channels));
        }
        Ok(Self(layout))
    }

    /// Owned layout storage; caller-owned native PCM/descriptors and allocator
    /// bookkeeping are not included. No PCM or per-callback storage is owned.
    pub(crate) fn storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.0.planes.capacity() * std::mem::size_of::<Plane>()
    }

    /// Validates *all* extents before touching bytes. Invalid packets are left
    /// unchanged: the native owner must fail the callback without interpreting
    /// unknown memory. No allocation, pointer cast, descriptor mutation or lock.
    pub(crate) fn prepare<'layout, 'planes, 'bytes>(
        &'layout self,
        planes: &'planes mut [PlaneBuffer<'bytes>],
    ) -> Result<OutputPacket<'layout, 'planes, 'bytes>, Error> {
        let frames = self
            .0
            .validate_sizes(planes.iter().map(|p| (p.channels, p.bytes.len(), true)))
            .map_err(Error::InvalidBuffers)?;
        let mut packet = OutputPacket {
            layout: self,
            planes,
            frames,
            written: 0,
            failure: None,
            preserve_pcm: false,
        };
        packet.clear();
        Ok(packet)
    }
}

/// Safe Rust cannot construct overlapping mutable plane borrows. A native
/// adapter must verify pointer ranges, enabled streams and lifetime *before*
/// constructing these slices; that unsafe boundary is deliberately absent.
pub(crate) struct PlaneBuffer<'a> {
    pub(crate) channels: u32,
    pub(crate) bytes: &'a mut [u8],
}

pub(crate) struct OutputPacket<'layout, 'planes, 'bytes> {
    layout: &'layout OutputLayout,
    planes: &'planes mut [PlaneBuffer<'bytes>],
    frames: usize,
    written: usize,
    failure: Option<Error>,
    preserve_pcm: bool,
}
impl<'layout, 'planes, 'bytes> OutputPacket<'layout, 'planes, 'bytes> {
    pub(crate) fn frames(&self) -> usize {
        self.frames
    }

    pub(crate) fn failure(&self) -> Option<Error> {
        self.failure
    }

    /// Also used after cancellation and on every uncommitted drop. Clearing
    /// cannot erase a malformed-write failure or authorize partial PCM.
    pub(crate) fn clear(&mut self) {
        for (layout, plane) in self.layout.0.planes.iter().zip(self.planes.iter_mut()) {
            plane.bytes.fill(if layout.encoding() == Encoding::U8 {
                128
            } else {
                0
            });
        }
        self.written = 0;
    }

    fn reject(&mut self, error: Error) -> Result<(), Error> {
        self.failure = Some(error);
        self.clear();
        Err(error)
    }

    /// Writes one sequential stereo chunk into the first pair of channels.
    /// The prepared equilibrium in all additional channels/padding is retained.
    pub(crate) fn write_stereo(
        &mut self,
        start: usize,
        left: &[f32],
        right: &[f32],
    ) -> Result<(), Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if start != self.written {
            return self.reject(Error::NonSequential {
                expected: self.written,
                actual: start,
            });
        }
        if left.len() != right.len() {
            return self.reject(Error::LengthMismatch {
                left: left.len(),
                right: right.len(),
            });
        }
        if left.len() > self.frames - self.written {
            return self.reject(Error::Extent {
                start,
                frames: left.len(),
                capacity: self.frames,
            });
        }
        let mut channel = 0;
        for (layout, plane) in self.layout.0.planes.iter().zip(self.planes.iter_mut()) {
            for local in 0..layout.channels.min(2 - channel) as usize {
                let samples = if channel == 0 { left } else { right };
                for (frame, sample) in samples.iter().enumerate() {
                    let offset =
                        (start + frame) * layout.frame_bytes + local * layout.sample_bytes();
                    encode(
                        *layout,
                        *sample,
                        &mut plane.bytes[offset..offset + layout.sample_bytes()],
                    );
                }
                channel += 1;
            }
            if channel == 2 {
                break;
            }
        }
        self.written += left.len();
        Ok(())
    }

    /// Consumes the writable packet. The completion owns its borrows so it
    /// cannot authorize another packet or subsequently modified PCM. This only
    /// establishes complete encoding, not native submission or hardware play.
    /// The completion remains default-silent until its explicit handoff.
    pub(crate) fn complete(self) -> Result<CompletedPacket<'layout, 'planes, 'bytes>, Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.written != self.frames {
            return Err(Error::Incomplete {
                written: self.written,
                total: self.frames,
            });
        }
        Ok(CompletedPacket { packet: self })
    }
}
impl Drop for OutputPacket<'_, '_, '_> {
    fn drop(&mut self) {
        if !self.preserve_pcm {
            self.clear();
        }
    }
}

pub(crate) struct CompletedPacket<'layout, 'planes, 'bytes> {
    packet: OutputPacket<'layout, 'planes, 'bytes>,
}
impl CompletedPacket<'_, '_, '_> {
    pub(crate) fn frames(&self) -> usize {
        self.packet.frames
    }

    /// Clear even after encoding completed, before the native callback hands
    /// its buffer back. Dropping the token has the same conservative effect.
    pub(crate) fn cancel(self) {
        drop(self);
    }

    /// Final software handoff: the native owner must check authorization just
    /// before this call. It preserves encoded bytes through callback return;
    /// it cannot prove native submission/playback or recall an existing buffer.
    pub(crate) fn handoff(mut self) {
        self.packet.preserve_pcm = true;
    }
}

impl StereoSink for OutputPacket<'_, '_, '_> {
    fn frames(&self) -> Result<usize, InvalidOutputBuffer> {
        if self.failure.is_some() {
            Err(InvalidOutputBuffer)
        } else {
            Ok(self.frames)
        }
    }
    fn clear(&mut self) {
        OutputPacket::clear(self);
    }
    fn write_stereo(
        &mut self,
        start: usize,
        left: &[f32],
        right: &[f32],
    ) -> Result<(), InvalidOutputBuffer> {
        OutputPacket::write_stereo(self, start, left, right).map_err(|_| InvalidOutputBuffer)
    }
}

fn encode(plane: Plane, value: f32, bytes: &mut [u8]) {
    let value = if value.is_finite() {
        f64::from(value.clamp(-1.0, 1.0))
    } else {
        0.0
    };
    let word = match plane.encoding() {
        Encoding::F32 => u64::from((value as f32).to_bits()),
        Encoding::F64 => value.to_bits(),
        Encoding::U8 => (value * 128.0 + 128.0).round().clamp(0.0, 255.0) as u64,
        Encoding::Signed { bits, high } => {
            let scale = (1_u64 << (bits - 1)) as f64;
            let signed = (value * scale).round().clamp(-scale, scale - 1.0) as i64;
            let significant = signed as u64 & ((1_u64 << bits) - 1);
            // Canonical padding is zero, for both positive and negative PCM.
            significant
                << if high {
                    plane.sample_bytes() as u32 * 8 - bits
                } else {
                    0
                }
        }
    };
    if plane.big_endian() {
        let raw = word.to_be_bytes();
        bytes.copy_from_slice(&raw[8 - bytes.len()..]);
    } else {
        bytes.copy_from_slice(&word.to_le_bytes()[..bytes.len()]);
    }
}

#[cfg(test)]
#[path = "native_output_packet_tests.rs"]
mod tests;
