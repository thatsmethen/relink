//! Device-free, single-source Sound Check playback. Native hosts must establish
//! exclusive ownership before attaching this processor to an output callback.
use crate::{
    control_bank::ControlBank, AudioError, SoundCheckSelection, GRAPH_BYTES, MAX_CALLBACK_FRAMES,
    RENDER_CHUNK,
};
use relink_core::{
    CompiledMixer, ControlError, PreparedControls, RenderError, ResourceBudget, Session,
    SoundCheck, SoundCheckClip, SoundCheckError, SoundCheckSnapshot, SoundCheckState,
    SoundCheckToken, StereoInput, StereoOutput, Uuid,
};
use std::{mem::size_of, sync::Arc};

/// One control-thread writer. Updates select only the current input's controls
/// and effects; no studio bus/send or capture device can enter the audition.
pub struct SoundCheckAuditionControl {
    selection: SoundCheckSelection,
    bank: Arc<ControlBank>,
}
impl SoundCheckAuditionControl {
    /// Off callback. A rejected edit preserves both the previous bank and clip.
    pub fn update(&mut self, studio: &Session) -> Result<u64, AudioError> {
        let graph = self.selection.playback_graph(studio)?;
        let prepared = PreparedControls::compile(&graph, GRAPH_BYTES).map_err(invalid)?;
        self.bank.publish(&prepared)
    }
}

/// Failure retains the unique clip for the host's Ready state. Dropping or
/// erasing the returned clip is an off-callback operation.
pub struct SoundCheckAuditionPrepareError {
    pub error: AudioError,
    pub clip: Box<SoundCheckClip>,
}
impl std::fmt::Debug for SoundCheckAuditionPrepareError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Error diagnostics must never include the retained PCM.
        formatter
            .debug_struct("SoundCheckAuditionPrepareError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for SoundCheckAuditionPrepareError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}
impl std::error::Error for SoundCheckAuditionPrepareError {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SoundCheckAuditionError {
    Clip(SoundCheckError),
    Controls(ControlError),
    Render(RenderError),
    OutputLengthMismatch,
    CallbackTooLarge,
    AlreadyStarted,
}
impl std::fmt::Display for SoundCheckAuditionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Clip(error) => error.fmt(formatter),
            Self::Controls(error) => error.fmt(formatter),
            Self::Render(error) => error.fmt(formatter),
            Self::OutputLengthMismatch => {
                formatter.write_str("audition output planes have different lengths")
            }
            Self::CallbackTooLarge => {
                formatter.write_str("audition callback exceeds the prepared frame bound")
            }
            Self::AlreadyStarted => {
                formatter.write_str("replay requires a fresh audition processor")
            }
        }
    }
}
impl std::error::Error for SoundCheckAuditionError {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundCheckAuditionProgress {
    /// Valid clip frames, excluding zero padding after the exact end of clip.
    pub rendered_frames: usize,
    pub wraps: usize,
    pub finished: bool,
    pub snapshot: SoundCheckSnapshot,
    /// Target bank accepted by the renderer, not proof of settled DSP or output.
    pub applied_control_version: u64,
}

/// One playback epoch with an immutable dry clip and a private effect graph.
/// Preparation, release and destruction run off callback. Rendering, Stop and
/// Clear use only preallocated memory. This type does not open native devices,
/// authorize recording, prove native shutdown, or acknowledge audible output.
///
/// Each replay uses a new processor so DSP starts from its defined initial state.
/// Within an explicit loop, processor history continues across exact dry wraps;
/// no crossfade or implicit tail is added. Clear revokes access immediately and
/// zeroes source scratch; physical clip erasure waits for off-callback release.
pub struct SoundCheckAudition {
    check: SoundCheck,
    mixer: CompiledMixer,
    bank: Arc<ControlBank>,
    scratch: PreparedControls,
    applied_version: u64,
    source: Uuid,
    bus: Uuid,
    left: [f32; RENDER_CHUNK],
    right: [f32; RENDER_CHUNK],
    started: bool,
    working_bytes: usize,
}

impl SoundCheckAudition {
    /// Compile without any device discovery or capture selection. `budget` is
    /// the remaining runtime allowance AFTER native owners, telemetry and other
    /// host state are charged. It cannot increase the runtime's 64 MiB ceiling.
    /// Includes actual clip capacity, fixed scratch and graph/control accounting.
    pub fn prepare(
        clip: SoundCheckClip,
        selection: SoundCheckSelection,
        studio: &Session,
        budget: usize,
    ) -> Result<(SoundCheckAuditionControl, Self), SoundCheckAuditionPrepareError> {
        let prepared = (|| {
            let view = clip.view();
            if view.source != selection.source_id() || view.sample_rate != selection.sample_rate() {
                return Err(invalid(
                    "the dry clip does not match the selected source and sample rate",
                ));
            }
            if view.frames.is_empty() {
                return Err(invalid("the dry clip is empty"));
            }
            let graph = selection.playback_graph(studio)?;
            let control_bytes = ControlBank::estimated_bytes(3, 2)
                .ok_or_else(|| invalid("audition control memory overflow"))?;
            // SoundCheck is embedded in Self; charge its allocation separately.
            // The bank estimate includes both callback scratch and the transient
            // control-thread PreparedControls, plus Arc's shared counters.
            let fixed = size_of::<Self>()
                .checked_add(size_of::<SoundCheckAuditionControl>())
                .and_then(|n| n.checked_add(clip.allocated_bytes()))
                .and_then(|n| n.checked_add(selection.heap_bytes()))
                .and_then(|n| n.checked_add(control_bytes))
                .and_then(|n| n.checked_add(2 * size_of::<usize>()))
                .ok_or_else(|| invalid("audition memory overflow"))?;
            let remaining = budget.min(GRAPH_BYTES).checked_sub(fixed).ok_or_else(|| {
                invalid("the dry clip and audition state exceed the runtime memory budget")
            })?;
            let mixer = CompiledMixer::compile(
                &graph,
                ResourceBudget {
                    max_block_frames: RENDER_CHUNK,
                    max_working_bytes: remaining,
                },
            )
            .map_err(invalid)?;
            let scratch = PreparedControls::compile(&graph, control_bytes).map_err(invalid)?;
            let bank = Arc::new(ControlBank::new(&scratch));
            let working_bytes = fixed + mixer.working_bytes();
            Ok((mixer, scratch, bank, working_bytes))
        })();
        let (mixer, scratch, bank, working_bytes) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(SoundCheckAuditionPrepareError {
                    error,
                    clip: Box::new(clip),
                })
            }
        };
        let source = selection.source_id();
        let bus = selection.bus_id();
        let control = SoundCheckAuditionControl {
            selection,
            bank: bank.clone(),
        };
        let renderer = Self {
            check: SoundCheck::from_clip(clip),
            mixer,
            bank,
            scratch,
            applied_version: 0,
            source,
            bus,
            left: [0.0; RENDER_CHUNK],
            right: [0.0; RENDER_CHUNK],
            started: false,
            working_bytes,
        };
        Ok((control, renderer))
    }
    pub fn snapshot(&self) -> SoundCheckSnapshot {
        self.check.snapshot()
    }
    pub fn working_bytes(&self) -> usize {
        self.working_bytes
    }
    /// Explicit start; preparing an audition never starts it. Replaying after
    /// Stop/EOF requires release and a fresh compile with the current settings.
    pub fn begin_playback(
        &mut self,
        token: SoundCheckToken,
        looping: bool,
    ) -> Result<SoundCheckToken, SoundCheckAuditionError> {
        if self.started {
            return Err(SoundCheckAuditionError::AlreadyStarted);
        }
        let token = self
            .check
            .begin_playback(token, looping)
            .map_err(SoundCheckAuditionError::Clip)?;
        self.started = true;
        Ok(token)
    }
    pub fn stop(&mut self, token: SoundCheckToken) -> Result<SoundCheckToken, SoundCheckError> {
        self.check.stop(token)
    }
    pub fn clear(&mut self, token: SoundCheckToken) -> Result<SoundCheckToken, SoundCheckError> {
        let token = self.check.clear(token)?;
        self.left.fill(0.0);
        self.right.fill(0.0);
        Ok(token)
    }
    /// All destinations are silent on rejection. Only valid clip frames enter
    /// DSP; a final partial block has exact zero padding, with no cached tails.
    /// A native host must submit the final block BEFORE revoking its output epoch.
    pub fn render(
        &mut self,
        token: SoundCheckToken,
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<SoundCheckAuditionProgress, SoundCheckAuditionError> {
        left.fill(0.0);
        right.fill(0.0);
        if left.len() != right.len() {
            return Err(SoundCheckAuditionError::OutputLengthMismatch);
        }
        if left.len() > MAX_CALLBACK_FRAMES {
            return Err(SoundCheckAuditionError::CallbackTooLarge);
        }
        let snapshot = self.check.snapshot();
        if token != snapshot.token {
            return Err(SoundCheckAuditionError::Clip(SoundCheckError::StaleToken));
        }
        if snapshot.state != SoundCheckState::Playing {
            return Err(SoundCheckAuditionError::Clip(
                SoundCheckError::InvalidState(snapshot.state),
            ));
        }
        // Empty callbacks cannot acknowledge an edit or advance a clip/ramp.
        if !left.is_empty() {
            if let Some(version) = self
                .bank
                .copy_if_new(&mut self.scratch, self.applied_version)
            {
                self.mixer
                    .apply_controls(&self.scratch)
                    .map_err(SoundCheckAuditionError::Controls)?;
                self.applied_version = version;
            }
        }
        let result = self.render_chunks(token, left, right);
        if result.is_err() {
            left.fill(0.0);
            right.fill(0.0);
        }
        result
    }
    fn render_chunks(
        &mut self,
        token: SoundCheckToken,
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<SoundCheckAuditionProgress, SoundCheckAuditionError> {
        let mut result = SoundCheckAuditionProgress {
            rendered_frames: 0,
            wraps: 0,
            finished: false,
            snapshot: self.check.snapshot(),
            applied_control_version: self.applied_version,
        };
        for (out_left, out_right) in left
            .chunks_mut(RENDER_CHUNK)
            .zip(right.chunks_mut(RENDER_CHUNK))
        {
            let count = out_left.len();
            let progress = self
                .check
                .render_playback(token, &mut self.left[..count], &mut self.right[..count])
                .map_err(SoundCheckAuditionError::Clip)?;
            let frames = progress.rendered_frames;
            self.mixer
                .render(
                    &[StereoInput {
                        id: self.source,
                        left: &self.left[..frames],
                        right: &self.right[..frames],
                    }],
                    &mut [StereoOutput {
                        id: self.bus,
                        left: &mut out_left[..frames],
                        right: &mut out_right[..frames],
                    }],
                    frames,
                )
                .map_err(SoundCheckAuditionError::Render)?;
            result.rendered_frames += frames;
            result.wraps += progress.wraps;
            result.finished = progress.finished;
            result.snapshot = progress.snapshot;
            if progress.finished {
                break;
            }
        }
        Ok(result)
    }
    /// Off callback, AFTER native ownership and in-flight callbacks are proven
    /// quiescent. Returns the original dry clip for another explicit audition;
    /// a cleared clip is physically erased instead. Never resumes any source.
    pub fn release(mut self) -> Option<SoundCheckClip> {
        if self.check.snapshot().state == SoundCheckState::Playing {
            let _ = self.check.stop(self.check.token());
        }
        let clip = if self.check.snapshot().state == SoundCheckState::Ready {
            self.check.take_clip(self.check.token()).ok()
        } else {
            None
        };
        self.check.erase_and_reclaim();
        clip
    }
}
fn invalid(error: impl std::fmt::Display) -> AudioError {
    AudioError::InvalidSession(error.to_string())
}

#[cfg(test)]
#[path = "sound_check_audition_tests.rs"]
mod tests;
