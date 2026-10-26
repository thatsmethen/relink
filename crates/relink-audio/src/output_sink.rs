//! A validated, callback-scoped stereo destination. Native submission is separate.
#[cfg(test)]
use crate::{MAX_CALLBACK_FRAMES, MAX_DEVICE_CHANNELS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InvalidOutputBuffer;

/// Implementations own or exclusively borrow their complete callback buffer.
/// Methods cannot allocate, block, query devices or submit native buffers.
/// `clear` writes format-specific equilibrium to every enabled channel. It must
/// remain valid after an encoding failure, and must not authorize submission.
pub(crate) trait StereoSink {
    fn frames(&self) -> Result<usize, InvalidOutputBuffer>;
    fn clear(&mut self);
    fn write_stereo(
        &mut self,
        start: usize,
        left: &[f32],
        right: &[f32],
    ) -> Result<(), InvalidOutputBuffer>;
}

/// Graph rendering has completed; a native owner still needs its own final
/// authorization check and successful submission before reporting device work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderOutcome {
    Silence,
    Rendered {
        frames: usize,
        accepted_version: Option<u64>,
    },
}

#[cfg(test)]
pub(crate) struct InterleavedSink<'a, T> {
    pub(crate) samples: &'a mut [T],
    pub(crate) channels: usize,
}
#[cfg(test)]
impl<T: cpal::SizedSample + cpal::FromSample<f32>> StereoSink for InterleavedSink<'_, T> {
    fn frames(&self) -> Result<usize, InvalidOutputBuffer> {
        if !(2..=usize::from(MAX_DEVICE_CHANNELS)).contains(&self.channels)
            || !self.samples.len().is_multiple_of(self.channels)
            || self.samples.len() / self.channels > MAX_CALLBACK_FRAMES
        {
            return Err(InvalidOutputBuffer);
        }
        Ok(self.samples.len() / self.channels)
    }

    fn clear(&mut self) {
        self.samples.fill(T::EQUILIBRIUM);
    }

    fn write_stereo(
        &mut self,
        start: usize,
        left: &[f32],
        right: &[f32],
    ) -> Result<(), InvalidOutputBuffer> {
        let frames = self.frames()?;
        if left.len() != right.len() || start.checked_add(left.len()).is_none_or(|end| end > frames)
        {
            return Err(InvalidOutputBuffer);
        }
        for (index, (&left, &right)) in left.iter().zip(right).enumerate() {
            let offset = (start + index) * self.channels;
            self.samples[offset] = T::from_sample(left.clamp(-1.0, 1.0));
            self.samples[offset + 1] = T::from_sample(right.clamp(-1.0, 1.0));
        }
        Ok(())
    }
}
