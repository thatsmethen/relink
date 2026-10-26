//! Portable WASAPI output encoding and single-owner render-buffer leases.
//! Native acquisition, COM, scheduling and lifecycle are deliberately absent.
use crate::wasapi_packet::{Encoding, Layout, SILENT};
use std::{cell::Cell, marker::PhantomData, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidBuffer,
    Incomplete,
    Cancelled,
}

/// Byte-only writes avoid alignment assumptions about a native buffer pointer.
/// The complete extent is validated before any byte is changed.
struct Packet<'a> {
    bytes: &'a mut [u8],
    layout: Layout,
    frames: usize,
    written: usize,
    failure: Option<Error>,
}
impl<'a> Packet<'a> {
    fn new(layout: Layout, frames: usize, bytes: &'a mut [u8]) -> Result<Self, Error> {
        if layout.channels < 2 || frames == 0 {
            return Err(Error::InvalidBuffer);
        }
        let extent = layout.bytes(frames).map_err(|_| Error::InvalidBuffer)?;
        if bytes.len() != extent {
            return Err(Error::InvalidBuffer);
        }
        let mut packet = Self {
            bytes,
            layout,
            frames,
            written: 0,
            failure: None,
        };
        packet.clear();
        Ok(packet)
    }
    fn clear(&mut self) {
        self.bytes
            .fill(if matches!(self.layout.encoding, Encoding::U8) {
                128
            } else {
                0
            });
        self.written = 0;
        // A failed write cannot be made committable by clearing its bytes.
    }
    fn write_stereo(&mut self, start: usize, left: &[f32], right: &[f32]) -> Result<(), Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if start != self.written
            || left.len() != right.len()
            || left.len() > self.frames - self.written
        {
            self.failure = Some(Error::InvalidBuffer);
            return Err(Error::InvalidBuffer);
        }
        let width = self.layout.encoding.bytes();
        let stride = usize::from(self.layout.channels) * width;
        for (offset, (&left, &right)) in left.iter().zip(right).enumerate() {
            let at = (start + offset) * stride;
            encode(self.layout.encoding, left, &mut self.bytes[at..at + width]);
            encode(
                self.layout.encoding,
                right,
                &mut self.bytes[at + width..at + 2 * width],
            );
        }
        self.written += left.len();
        Ok(())
    }
    fn complete(&self) -> Result<(), Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.written == self.frames {
            Ok(())
        } else {
            Err(Error::Incomplete)
        }
    }
}

fn encode(encoding: Encoding, value: f32, bytes: &mut [u8]) {
    let value = if value.is_finite() {
        f64::from(value.clamp(-1.0, 1.0))
    } else {
        0.0
    };
    let integer = |scale: f64| (value * scale).round().clamp(-scale, scale - 1.0) as i32;
    match encoding {
        Encoding::F32 => bytes.copy_from_slice(&(value as f32).to_le_bytes()),
        Encoding::F64 => bytes.copy_from_slice(&value.to_le_bytes()),
        Encoding::I16 => bytes.copy_from_slice(&(integer(32768.0) as i16).to_le_bytes()),
        Encoding::I24In32 => {
            // WAVEFORMATEXTENSIBLE valid PCM bits are high-aligned. Padding is
            // canonical zero even for negative samples.
            bytes.copy_from_slice(&(integer(8_388_608.0) << 8).to_le_bytes());
        }
        Encoding::I32 => bytes.copy_from_slice(&integer(2_147_483_648.0).to_le_bytes()),
        Encoding::U8 => bytes[0] = (value * 128.0 + 128.0).round().clamp(0.0, 255.0) as u8,
    }
}

/// Stored even when release happens in Drop or preparation fails. The caller
/// owns this Cell and can preserve a native HRESULT without this module knowing
/// its meaning. Use a fresh record per acquired lease and inspect it afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReleaseRecord<E> {
    pub(crate) frames: u32,
    pub(crate) flags: u32,
    pub(crate) result: Result<(), E>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommitError<E> {
    Preparation(Error),
    Release(E),
}

/// Construct immediately after one successful, nonzero GetBuffer acquisition.
/// This object owns its byte borrow until release: no token from another buffer
/// can authorize its commit. It is !Send/!Sync even when its closure is Send.
/// It must stay in the acquiring owner's invocation; the type cannot enforce a
/// wall-time deadline or prevent callers from deliberately forgetting a guard.
pub(crate) struct RenderLease<'data, 'record, F, E>
where
    F: FnOnce(u32, u32) -> Result<(), E>,
    E: Copy,
{
    packet: Option<Packet<'data>>,
    frames: u32,
    release: Option<F>,
    record: &'record Cell<Option<ReleaseRecord<E>>>,
    cancelled: bool,
    _owner_thread: PhantomData<Rc<()>>,
}
impl<'data, 'record, F, E> RenderLease<'data, 'record, F, E>
where
    F: FnOnce(u32, u32) -> Result<(), E>,
    E: Copy,
{
    pub(crate) fn new(
        frames: u32,
        layout: Layout,
        bytes: Result<&'data mut [u8], Error>,
        record: &'record Cell<Option<ReleaseRecord<E>>>,
        release: F,
    ) -> Result<Self, Error> {
        // Establish the release obligation before any fallible validation.
        let mut lease = Self {
            packet: None,
            frames,
            release: Some(release),
            record,
            cancelled: false,
            _owner_thread: PhantomData,
        };
        record.set(None);
        // The native owner may reject a null/malformed pointer before forming
        // any slice. That path still owns a successful GetBuffer acquisition.
        lease.packet = Some(Packet::new(layout, frames as usize, bytes?)?);
        Ok(lease)
    }
    pub(crate) fn frames(&self) -> usize {
        self.frames as usize
    }
    pub(crate) fn clear(&mut self) {
        self.packet.as_mut().expect("validated packet").clear();
    }
    pub(crate) fn write_stereo(
        &mut self,
        start: usize,
        left: &[f32],
        right: &[f32],
    ) -> Result<(), Error> {
        if self.cancelled {
            return Err(Error::Cancelled);
        }
        self.packet
            .as_mut()
            .expect("validated packet")
            .write_stereo(start, left, right)
    }
    pub(crate) fn cancel(&mut self) {
        self.cancelled = true;
        self.clear();
    }
    /// The owner supplies its final authorization check immediately before this
    /// call. A later cancellation cannot recall an already submitted OS buffer.
    /// Any preparation error releases SILENT; its release result remains in the
    /// record even if this method returns the earlier preparation error.
    pub(crate) fn commit(mut self, authorized: bool) -> Result<(), CommitError<E>> {
        let prepared = if self.cancelled || !authorized {
            Err(Error::Cancelled)
        } else {
            self.packet.as_ref().expect("validated packet").complete()
        };
        let released = self.release_once(if prepared.is_ok() { 0 } else { SILENT });
        prepared.map_err(CommitError::Preparation)?;
        released.map_err(CommitError::Release)
    }
    pub(crate) fn finish_silent(mut self) -> Result<(), E> {
        self.release_once(SILENT)
    }
    fn release_once(&mut self, flags: u32) -> Result<(), E> {
        // End the byte borrow before returning ownership to the native engine.
        self.packet = None;
        // Take before calling: an error (or unwinding closure) cannot cause a
        // second attempt from Drop. Native owners must not panic in release.
        let result = self.release.take().expect("one release obligation")(self.frames, flags);
        self.record.set(Some(ReleaseRecord {
            frames: self.frames,
            flags,
            result,
        }));
        result
    }
}
impl<F, E> crate::output_sink::StereoSink for RenderLease<'_, '_, F, E>
where
    F: FnOnce(u32, u32) -> Result<(), E>,
    E: Copy,
{
    fn frames(&self) -> Result<usize, crate::output_sink::InvalidOutputBuffer> {
        Ok(self.frames())
    }
    fn clear(&mut self) {
        self.clear();
    }
    fn write_stereo(
        &mut self,
        start: usize,
        left: &[f32],
        right: &[f32],
    ) -> Result<(), crate::output_sink::InvalidOutputBuffer> {
        self.write_stereo(start, left, right)
            .map_err(|_| crate::output_sink::InvalidOutputBuffer)
    }
}
impl<F, E> Drop for RenderLease<'_, '_, F, E>
where
    F: FnOnce(u32, u32) -> Result<(), E>,
    E: Copy,
{
    fn drop(&mut self) {
        if self.release.is_some() {
            let _ = self.release_once(SILENT);
        }
    }
}

#[cfg(test)]
#[path = "wasapi_render_packet_tests.rs"]
mod tests;
