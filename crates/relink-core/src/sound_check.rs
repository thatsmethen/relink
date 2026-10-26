//! Memory-only, selected-source Sound Check storage and replay. This module does
//! not open devices, request permission, record files or prove native teardown.
use crate::{StereoInput, Uuid};
use std::{
    error::Error,
    fmt,
    mem::size_of,
    sync::atomic::{AtomicU64, Ordering},
};

pub const SOUND_CHECK_MAX_SECONDS: usize = 30;
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
fn next_session(counter: &AtomicU64) -> Result<u64, SoundCheckError> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map_err(|_| SoundCheckError::SessionIdsExhausted)
}

/// Opaque identity for one owner/session and one operation generation. No UUID
/// allocation, global counter or synchronization is needed for transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundCheckToken {
    session: u64,
    generation: u64,
}
impl SoundCheckToken {
    pub fn session_id(self) -> u64 {
        self.session
    }
    pub fn generation(self) -> u64 {
        self.generation
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundCheckState {
    Idle,
    Recording,
    Ready,
    Playing,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundCheckSnapshot {
    pub token: SoundCheckToken,
    pub state: SoundCheckState,
    pub source: Uuid,
    pub sample_rate: u32,
    pub capacity_frames: usize,
    pub recorded_frames: usize,
    pub playhead_frames: usize,
    pub looping: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundCheckCaptureProgress {
    pub accepted_frames: usize,
    pub reached_limit: bool,
    pub snapshot: SoundCheckSnapshot,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundCheckPlaybackProgress {
    pub rendered_frames: usize,
    pub wraps: usize,
    pub finished: bool,
    pub snapshot: SoundCheckSnapshot,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundCheckError {
    InvalidSampleRate,
    InvalidCapacity {
        requested: usize,
        maximum: usize,
    },
    ResourceBudget {
        required: usize,
        allowed: usize,
    },
    AllocationFailed,
    SessionIdsExhausted,
    GenerationExhausted,
    StaleToken,
    SourceMismatch {
        expected: Uuid,
        received: Uuid,
    },
    InvalidState(SoundCheckState),
    EmptyClip,
    InputTooShort {
        requested: usize,
        left: usize,
        right: usize,
    },
    OutputLengthMismatch,
}
impl fmt::Display for SoundCheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate => f.write_str("Sound Check sample rate must be between 8000 and 384000 Hz"),
            Self::InvalidCapacity { requested, maximum } => write!(f, "Sound Check capacity {requested} frames must be positive and at most {maximum} frames (30 seconds)"),
            Self::ResourceBudget { required, allowed } => write!(f, "Sound Check needs {required} bytes; the allowed budget is {allowed}"),
            Self::AllocationFailed => f.write_str("Sound Check storage allocation failed"),
            Self::SessionIdsExhausted => f.write_str("Sound Check session identities are exhausted"),
            Self::GenerationExhausted => f.write_str("Sound Check operation generations are exhausted; this owner is closed"),
            Self::StaleToken => f.write_str("Sound Check operation belongs to an old session or generation"),
            Self::SourceMismatch { expected, received } => write!(f, "Sound Check selected source {expected}, not {received}"),
            Self::InvalidState(state) => write!(f, "Sound Check operation is unavailable in state {state:?}"),
            Self::EmptyClip => f.write_str("Sound Check has no captured frames to play"),
            Self::InputTooShort { requested, left, right } => write!(f, "Sound Check input needs {requested} frames; left has {left} and right has {right}"),
            Self::OutputLengthMismatch => f.write_str("Sound Check playback channels must have equal lengths"),
        }
    }
}
impl Error for SoundCheckError {}

/// A borrowed, immutable dry clip available only from Ready. The borrow prevents
/// changing/clearing its owner until the caller releases this view.
#[derive(Clone, Copy, Debug)]
pub struct SoundCheckClipView<'a> {
    pub token: SoundCheckToken,
    pub source: Uuid,
    pub sample_rate: u32,
    pub frames: &'a [[f32; 2]],
}

/// Unique owned dry clip. Transfer only after the host establishes callback and
/// native-owner quiescence. This type is deliberately not Clone and has no
/// mutable PCM API. Drop/erase/reclaim must run off the audio callback.
pub struct SoundCheckClip {
    token: SoundCheckToken,
    source: Uuid,
    sample_rate: u32,
    recorded: usize,
    storage: Vec<[f32; 2]>,
}
impl SoundCheckClip {
    pub fn view(&self) -> SoundCheckClipView<'_> {
        SoundCheckClipView {
            token: self.token,
            source: self.source,
            sample_rate: self.sample_rate,
            frames: &self.storage[..self.recorded],
        }
    }
    pub fn capacity_frames(&self) -> usize {
        self.storage.len()
    }
    pub fn allocated_bytes(&self) -> usize {
        self.storage.capacity() * size_of::<[f32; 2]>()
    }
    pub fn working_bytes(&self) -> usize {
        size_of::<Self>() + self.allocated_bytes()
    }
    /// Off callback, after all borrowers/owners are quiescent. Overwrites the
    /// initialized slots before releasing storage, including old captured tails.
    /// Allocator slack beyond logical capacity has never held captured PCM.
    pub fn erase_and_reclaim(mut self) {
        erase(&mut self.storage);
    }
}

/// A bounded owner moved between host control/record/playback contexts only
/// after callback quiescence. Prepare, physical erasure and destruction run off
/// callback. Other methods use fixed state and the existing allocation only.
///
/// The host supplies selected-source PCM before ReLink effects. Stored samples
/// are copied bit-for-bit, including nonfinite payloads; replay is dry and the
/// host applies its current effects exactly once. This cannot undo hardware DSP.
/// Tokens prevent stale work from mutating/replaying this owner; they do not
/// establish native Stop, output isolation, or recording permission.
pub struct SoundCheck {
    token: SoundCheckToken,
    source: Uuid,
    sample_rate: u32,
    state: SoundCheckState,
    recorded: usize,
    playhead: usize,
    looping: bool,
    storage: Vec<[f32; 2]>,
}
impl SoundCheck {
    pub fn maximum_frames(sample_rate: u32) -> Result<usize, SoundCheckError> {
        if !(8000..=384000).contains(&sample_rate) {
            return Err(SoundCheckError::InvalidSampleRate);
        }
        (sample_rate as usize)
            .checked_mul(SOUND_CHECK_MAX_SECONDS)
            .ok_or(SoundCheckError::InvalidCapacity {
                requested: usize::MAX,
                maximum: usize::MAX,
            })
    }
    /// Requested storage plus owner metadata; allocator bookkeeping is excluded.
    /// Capacity is validated against the independent ReLink 30-second limit.
    pub fn required_bytes(
        sample_rate: u32,
        capacity_frames: usize,
    ) -> Result<usize, SoundCheckError> {
        let maximum = Self::maximum_frames(sample_rate)?;
        if capacity_frames == 0 || capacity_frames > maximum {
            return Err(SoundCheckError::InvalidCapacity {
                requested: capacity_frames,
                maximum,
            });
        }
        capacity_frames
            .checked_mul(size_of::<[f32; 2]>())
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()))
            .ok_or(SoundCheckError::ResourceBudget {
                required: usize::MAX,
                allowed: usize::MAX,
            })
    }
    /// Control-thread preparation. Budget and duration checks precede allocation;
    /// opening creates Idle state and never starts capture or replay.
    pub fn prepare(
        source: Uuid,
        sample_rate: u32,
        capacity_frames: usize,
        maximum_bytes: usize,
    ) -> Result<Self, SoundCheckError> {
        let required = Self::required_bytes(sample_rate, capacity_frames)?;
        if required > maximum_bytes {
            return Err(SoundCheckError::ResourceBudget {
                required,
                allowed: maximum_bytes,
            });
        }
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(capacity_frames)
            .map_err(|_| SoundCheckError::AllocationFailed)?;
        // try_reserve_exact may supply excess capacity. The requested byte
        // bound was checked before allocation; charge actual capacity before
        // handing the allocation to a recording/playback owner.
        let actual_required = storage
            .capacity()
            .checked_mul(size_of::<[f32; 2]>())
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()))
            .unwrap_or(usize::MAX);
        if actual_required > maximum_bytes {
            return Err(SoundCheckError::ResourceBudget {
                required: actual_required,
                allowed: maximum_bytes,
            });
        }
        storage.resize(capacity_frames, [0.0; 2]);
        let token = SoundCheckToken {
            session: next_session(&NEXT_SESSION)?,
            generation: 1,
        };
        Ok(Self {
            token,
            source,
            sample_rate,
            state: SoundCheckState::Idle,
            recorded: 0,
            playhead: 0,
            looping: false,
            storage,
        })
    }
    /// Move a unique completed dry clip into a playback-capable owner without
    /// allocation. The host must first stop all native capture and establish
    /// callback/native-owner completion; this constructor cannot prove that.
    pub fn from_clip(clip: SoundCheckClip) -> Self {
        Self {
            token: clip.token,
            source: clip.source,
            sample_rate: clip.sample_rate,
            state: SoundCheckState::Ready,
            recorded: clip.recorded,
            playhead: 0,
            looping: false,
            storage: clip.storage,
        }
    }
    pub fn token(&self) -> SoundCheckToken {
        self.token
    }
    pub fn snapshot(&self) -> SoundCheckSnapshot {
        SoundCheckSnapshot {
            token: self.token,
            state: self.state,
            source: self.source,
            sample_rate: self.sample_rate,
            capacity_frames: self.storage.len(),
            recorded_frames: self.recorded,
            playhead_frames: self.playhead,
            looping: self.looping,
        }
    }
    pub fn working_bytes(&self) -> usize {
        size_of::<Self>() + self.allocated_bytes()
    }
    pub fn allocated_bytes(&self) -> usize {
        self.storage.capacity() * size_of::<[f32; 2]>()
    }
    fn check(&self, token: SoundCheckToken) -> Result<(), SoundCheckError> {
        if token != self.token {
            Err(SoundCheckError::StaleToken)
        } else {
            Ok(())
        }
    }
    fn live(&self, token: SoundCheckToken) -> Result<(), SoundCheckError> {
        self.check(token)?;
        if self.state == SoundCheckState::Closed {
            Err(SoundCheckError::InvalidState(self.state))
        } else {
            Ok(())
        }
    }
    fn advance(&mut self) -> Result<SoundCheckToken, SoundCheckError> {
        if let Some(next) = self.token.generation.checked_add(1) {
            self.token.generation = next;
            Ok(self.token)
        } else {
            self.state = SoundCheckState::Closed;
            self.recorded = 0;
            self.playhead = 0;
            self.looping = false;
            Err(SoundCheckError::GenerationExhausted)
        }
    }
    pub fn begin_recording(
        &mut self,
        token: SoundCheckToken,
    ) -> Result<SoundCheckToken, SoundCheckError> {
        self.live(token)?;
        if !matches!(self.state, SoundCheckState::Idle | SoundCheckState::Ready) {
            return Err(SoundCheckError::InvalidState(self.state));
        }
        self.advance()?;
        self.state = SoundCheckState::Recording;
        self.recorded = 0;
        self.playhead = 0;
        self.looping = false;
        Ok(self.token)
    }
    /// Copies only the selected source, before effects. A partial final block is
    /// accepted exactly up to capacity; reaching the cap freezes Ready and
    /// invalidates this capture token. The host must then stop native capture.
    pub fn capture(
        &mut self,
        token: SoundCheckToken,
        input: StereoInput<'_>,
        frames: usize,
    ) -> Result<SoundCheckCaptureProgress, SoundCheckError> {
        self.live(token)?;
        if self.state != SoundCheckState::Recording {
            return Err(SoundCheckError::InvalidState(self.state));
        }
        if input.id != self.source {
            return Err(SoundCheckError::SourceMismatch {
                expected: self.source,
                received: input.id,
            });
        }
        if input.left.len() < frames || input.right.len() < frames {
            return Err(SoundCheckError::InputTooShort {
                requested: frames,
                left: input.left.len(),
                right: input.right.len(),
            });
        }
        let accepted = frames.min(self.storage.len() - self.recorded);
        let reached_limit = self.recorded + accepted == self.storage.len();
        if reached_limit {
            self.advance()?;
        }
        for index in 0..accepted {
            self.storage[self.recorded + index] = [input.left[index], input.right[index]];
        }
        self.recorded += accepted;
        if reached_limit {
            self.state = SoundCheckState::Ready;
        }
        Ok(SoundCheckCaptureProgress {
            accepted_frames: accepted,
            reached_limit,
            snapshot: self.snapshot(),
        })
    }
    /// Revokes all earlier work, preserves a nonempty clip, and rewinds playback.
    /// This is a logical state transition, not an acknowledgement of native Stop.
    pub fn stop(&mut self, token: SoundCheckToken) -> Result<SoundCheckToken, SoundCheckError> {
        self.live(token)?;
        self.advance()?;
        self.state = if self.recorded == 0 {
            SoundCheckState::Idle
        } else {
            SoundCheckState::Ready
        };
        self.playhead = 0;
        self.looping = false;
        Ok(self.token)
    }
    /// Start only after the host has established native capture teardown. Play
    /// always starts at frame zero. Looping is an explicit choice, never implicit.
    pub fn begin_playback(
        &mut self,
        token: SoundCheckToken,
        looping: bool,
    ) -> Result<SoundCheckToken, SoundCheckError> {
        self.live(token)?;
        if self.state == SoundCheckState::Idle {
            return Err(SoundCheckError::EmptyClip);
        }
        if self.state != SoundCheckState::Ready {
            return Err(SoundCheckError::InvalidState(self.state));
        }
        if self.recorded == 0 {
            return Err(SoundCheckError::EmptyClip);
        }
        self.advance()?;
        self.state = SoundCheckState::Playing;
        self.playhead = 0;
        self.looping = looping;
        Ok(self.token)
    }
    /// Fill the entire destination, including silence after nonloop completion
    /// and on errors/stale tokens. Exact loop wraps apply no fades or resampling.
    /// Nonloop completion returns Ready at frame zero and revokes this play token.
    pub fn render_playback(
        &mut self,
        token: SoundCheckToken,
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<SoundCheckPlaybackProgress, SoundCheckError> {
        left.fill(0.0);
        right.fill(0.0);
        self.live(token)?;
        if left.len() != right.len() {
            return Err(SoundCheckError::OutputLengthMismatch);
        }
        if self.state != SoundCheckState::Playing {
            return Err(SoundCheckError::InvalidState(self.state));
        }
        let finished =
            !self.looping && !left.is_empty() && left.len() >= self.recorded - self.playhead;
        // Preflight invalidation before emitting the final block so generation
        // exhaustion cannot leave playable data under a reused token.
        if finished {
            self.advance()?;
        }
        let mut written = 0;
        let mut wraps = 0;
        while written < left.len() {
            let count = (self.recorded - self.playhead).min(left.len() - written);
            for index in 0..count {
                let frame = self.storage[self.playhead + index];
                left[written + index] = frame[0];
                right[written + index] = frame[1];
            }
            written += count;
            self.playhead += count;
            if self.playhead == self.recorded {
                self.playhead = 0;
                if self.looping {
                    wraps += 1;
                } else {
                    break;
                }
            }
        }
        if finished {
            self.state = SoundCheckState::Ready;
            self.looping = false;
        }
        Ok(SoundCheckPlaybackProgress {
            rendered_frames: written,
            wraps,
            finished,
            snapshot: self.snapshot(),
        })
    }
    /// Invalidate the clip immediately, including cached length/playhead. The
    /// allocation is retained for reuse; erase/reclaim physical storage off callback.
    pub fn clear(&mut self, token: SoundCheckToken) -> Result<SoundCheckToken, SoundCheckError> {
        self.live(token)?;
        self.advance()?;
        self.state = SoundCheckState::Idle;
        self.recorded = 0;
        self.playhead = 0;
        self.looping = false;
        Ok(self.token)
    }
    pub fn close(&mut self, token: SoundCheckToken) -> Result<SoundCheckToken, SoundCheckError> {
        self.live(token)?;
        self.advance()?;
        self.state = SoundCheckState::Closed;
        self.recorded = 0;
        self.playhead = 0;
        self.looping = false;
        Ok(self.token)
    }
    pub fn clip(&self, token: SoundCheckToken) -> Result<SoundCheckClipView<'_>, SoundCheckError> {
        self.live(token)?;
        if self.state != SoundCheckState::Ready {
            return Err(SoundCheckError::InvalidState(self.state));
        }
        Ok(SoundCheckClipView {
            token: self.token,
            source: self.source,
            sample_rate: self.sample_rate,
            frames: &self.storage[..self.recorded],
        })
    }
    /// Move the unique completed allocation after host-established quiescence.
    /// The old owner closes permanently; no clone, allocation or deallocation
    /// occurs. Keep the returned owner alive until control-thread reclamation.
    pub fn take_clip(&mut self, token: SoundCheckToken) -> Result<SoundCheckClip, SoundCheckError> {
        self.live(token)?;
        if self.state != SoundCheckState::Ready {
            return Err(SoundCheckError::InvalidState(self.state));
        }
        self.advance()?;
        let clip = SoundCheckClip {
            token: self.token,
            source: self.source,
            sample_rate: self.sample_rate,
            recorded: self.recorded,
            storage: std::mem::take(&mut self.storage),
        };
        self.state = SoundCheckState::Closed;
        self.recorded = 0;
        self.playhead = 0;
        self.looping = false;
        Ok(clip)
    }
    /// O(capacity), control-thread only. Clear/Close must already have removed
    /// logical access; erasure keeps the allocation for a later recording.
    pub fn erase_cleared_storage(&mut self, token: SoundCheckToken) -> Result<(), SoundCheckError> {
        self.check(token)?;
        if !matches!(self.state, SoundCheckState::Idle | SoundCheckState::Closed) {
            return Err(SoundCheckError::InvalidState(self.state));
        }
        erase(&mut self.storage);
        Ok(())
    }
    /// Consume only off callback after native/callback quiescence. Overwrites
    /// all initialized slots, including old tails, before releasing storage.
    /// Allocator slack beyond logical capacity has never held captured PCM.
    pub fn erase_and_reclaim(mut self) {
        erase(&mut self.storage);
    }
}
fn erase(storage: &mut [[f32; 2]]) {
    for frame in storage {
        // SAFETY: an exclusive mutable slice owns valid aligned frame slots.
        // Volatile writes keep the explicit erasure before deallocation from
        // being removed as a dead store. No callback calls this O(capacity) path.
        unsafe {
            std::ptr::write_volatile(frame, [0.0; 2]);
        }
    }
}

#[cfg(test)]
#[path = "sound_check_tests.rs"]
mod tests;
