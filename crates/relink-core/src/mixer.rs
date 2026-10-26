use crate::{
    dsp::{sanitize, Frame, Processor},
    session::db_to_gain,
    smoothing::ControlRamp,
    InputKind, Session, SessionError,
};
use std::{
    error::Error,
    fmt,
    mem::size_of,
    sync::atomic::{AtomicU64, Ordering},
};
use uuid::Uuid;

// Allocated only during off-callback compilation. Checked advancement prevents
// graph identities being reused after wraparound; moving a graph preserves it.
static NEXT_GRAPH_TOKEN: AtomicU64 = AtomicU64::new(1);

/// A host-selected memory and callback-size budget. Channel counts are derived
/// from available memory rather than a fixed input or mix count in the product.
/// The estimate covers owned graph arrays and their frame buffers, not the
/// caller's session, source/destination buffers, or allocator bookkeeping.
/// Inline live-control ramp state is included in the graph estimate.
#[derive(Clone, Copy, Debug)]
pub struct ResourceBudget {
    pub max_block_frames: usize,
    pub max_working_bytes: usize,
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self {
            max_block_frames: 2048,
            max_working_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy)]
pub struct StereoInput<'a> {
    pub id: Uuid,
    pub left: &'a [f32],
    pub right: &'a [f32],
}

pub struct StereoOutput<'a> {
    pub id: Uuid,
    pub left: &'a mut [f32],
    pub right: &'a mut [f32],
}

/// A bus resolved off callback for one compiled graph. Handles survive moving
/// that graph, but are not valid for a separately compiled replacement, even
/// when the replacement uses the same session and bus IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusHandle {
    graph_token: u64,
    index: usize,
}

/// Errors from reading already-rendered bus audio. They never advance or change
/// the graph. Resolve unknown bus IDs with [`CompiledMixer::resolve_bus`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusReadError {
    /// The handle does not belong to this graph or its index is invalid.
    InvalidHandle,
    /// No render has succeeded, or the most recent render attempt was rejected.
    NoSuccessfulRender,
}

impl fmt::Display for BusReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHandle => f.write_str("bus handle does not belong to this compiled graph"),
            Self::NoSuccessfulRender => {
                f.write_str("the latest render attempt has no readable bus block")
            }
        }
    }
}
impl Error for BusReadError {}

/// Post-control, post-DSP meters for the last rendered block. Hosts can copy
/// these fixed-size values into their own callback-safe meter transport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterReading {
    pub id: Uuid,
    pub peak: [f32; 2],
    pub rms: [f32; 2],
    pub clipped_samples: [u64; 2],
}

impl MeterReading {
    fn empty(id: Uuid) -> Self {
        Self {
            id,
            peak: [0.0; 2],
            rms: [0.0; 2],
            clipped_samples: [0; 2],
        }
    }
    fn measure(&mut self, frames: &[Frame]) {
        self.peak = [0.0; 2];
        self.rms = [0.0; 2];
        self.clipped_samples = [0; 2];
        let mut squares = [0.0_f64; 2];
        for frame in frames {
            for channel in 0..2 {
                let sample = frame[channel];
                self.peak[channel] = self.peak[channel].max(sample.abs());
                squares[channel] += f64::from(sample) * f64::from(sample);
                if sample.abs() > 1.0 {
                    self.clipped_samples[channel] += 1;
                }
            }
        }
        if !frames.is_empty() {
            for (channel, sum) in squares.iter().enumerate() {
                self.rms[channel] = (sum / frames.len() as f64).sqrt() as f32;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderError {
    BlockTooLarge {
        requested: usize,
        maximum: usize,
    },
    BufferTooShort {
        id: Uuid,
        required: usize,
        available: usize,
    },
    UnknownInput(Uuid),
    UnknownBus(Uuid),
    DuplicateInput(Uuid),
    DuplicateBus(Uuid),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlockTooLarge { requested, maximum } => write!(
                f,
                "render block has {requested} frames; configured maximum is {maximum}"
            ),
            Self::BufferTooShort {
                id,
                required,
                available,
            } => write!(f, "buffer {id} has {available} frames; {required} required"),
            Self::UnknownInput(id) => write!(f, "unknown input buffer {id}"),
            Self::UnknownBus(id) => write!(f, "unknown output bus {id}"),
            Self::DuplicateInput(id) => write!(f, "duplicate input buffer {id}"),
            Self::DuplicateBus(id) => write!(f, "duplicate output buffer {id}"),
        }
    }
}
impl Error for RenderError {}

struct Tone {
    phase: f64,
    increment: f64,
    amplitude: f32,
}

impl Tone {
    fn next(&mut self) -> f32 {
        let sample = self.phase.sin() as f32 * self.amplitude;
        self.phase += self.increment;
        if self.phase >= std::f64::consts::TAU {
            self.phase -= std::f64::consts::TAU;
        }
        sample
    }
}

struct CompiledChannel {
    id: Uuid,
    frames: Vec<Frame>,
    processor: Processor,
    gains: ControlRamp<2>,
    activity: ControlRamp<1>,
    tone: Option<Tone>,
}

struct CompiledSend {
    id: Uuid,
    input_index: usize,
    bus_index: usize,
    gains: ControlRamp<2>,
}

pub struct CompiledMixer {
    graph_token: u64,
    rendered_frames: Option<usize>,
    sample_rate: u32,
    control_ramp_frames: u32,
    max_block_frames: usize,
    working_bytes: usize,
    inputs: Vec<CompiledChannel>,
    buses: Vec<CompiledChannel>,
    sends: Vec<CompiledSend>,
    input_meters: Vec<MeterReading>,
    bus_meters: Vec<MeterReading>,
}

impl CompiledMixer {
    /// Must run off the audio thread. The session is copied into an immutable
    /// topology; subsequent session edits do not change this graph.
    pub fn compile(session: &Session, budget: ResourceBudget) -> Result<Self, SessionError> {
        session.validate()?;
        if budget.max_block_frames == 0 {
            return Err(SessionError::Invalid(
                "max_block_frames must be greater than zero".into(),
            ));
        }
        let overflow = || SessionError::ResourceBudget {
            resource: "working bytes",
            required: usize::MAX,
            allowed: budget.max_working_bytes,
        };
        let channel_count = session
            .inputs
            .len()
            .checked_add(session.buses.len())
            .ok_or_else(overflow)?;
        let frame_bytes = budget
            .max_block_frames
            .checked_mul(size_of::<Frame>())
            .ok_or_else(overflow)?;
        let channel_bytes = size_of::<CompiledChannel>()
            .checked_add(size_of::<MeterReading>())
            .and_then(|n| n.checked_add(frame_bytes))
            .ok_or_else(overflow)?;
        let working_bytes = channel_count
            .checked_mul(channel_bytes)
            .and_then(|n| {
                session
                    .sends
                    .len()
                    .checked_mul(size_of::<CompiledSend>())
                    .and_then(|s| n.checked_add(s))
            })
            .and_then(|n| n.checked_add(size_of::<Self>()))
            .ok_or_else(overflow)?;
        if working_bytes > budget.max_working_bytes {
            return Err(SessionError::ResourceBudget {
                resource: "working bytes",
                required: working_bytes,
                allowed: budget.max_working_bytes,
            });
        }
        let input_solo = session.inputs.iter().any(|input| input.controls.solo);
        let bus_solo = session.buses.iter().any(|bus| bus.controls.solo);
        let mut inputs = reserved_vec(session.inputs.len())?;
        let mut buses = reserved_vec(session.buses.len())?;
        let mut sends = reserved_vec(session.sends.len())?;
        let mut input_meters = reserved_vec(session.inputs.len())?;
        let mut bus_meters = reserved_vec(session.buses.len())?;
        for input in &session.inputs {
            let tone = match input.kind {
                InputKind::TestTone {
                    frequency_hz,
                    amplitude_db,
                } => Some(Tone {
                    phase: 0.0,
                    increment: std::f64::consts::TAU * f64::from(frequency_hz)
                        / f64::from(session.sample_rate),
                    amplitude: db_to_gain(amplitude_db),
                }),
                _ => None,
            };
            inputs.push(CompiledChannel {
                id: input.id,
                frames: frame_buffer(budget.max_block_frames)?,
                processor: Processor::new(&input.dsp, session.sample_rate),
                gains: ControlRamp::new(input.controls.gains()),
                activity: ControlRamp::new(activity_gain(
                    !input.controls.muted && (!input_solo || input.controls.solo),
                )),
                tone,
            });
            input_meters.push(MeterReading::empty(input.id));
        }
        for bus in &session.buses {
            buses.push(CompiledChannel {
                id: bus.id,
                frames: frame_buffer(budget.max_block_frames)?,
                processor: Processor::new(&bus.dsp, session.sample_rate),
                gains: ControlRamp::new(bus.controls.gains()),
                activity: ControlRamp::new(activity_gain(
                    !bus.controls.muted && (!bus_solo || bus.controls.solo),
                )),
                tone: None,
            });
            bus_meters.push(MeterReading::empty(bus.id));
        }
        for send in &session.sends {
            let input_index = session
                .inputs
                .iter()
                .position(|input| input.id == send.input_id)
                .ok_or(SessionError::MissingInput(send.input_id))?;
            let bus_index = session
                .buses
                .iter()
                .position(|bus| bus.id == send.bus_id)
                .ok_or(SessionError::MissingBus(send.bus_id))?;
            let send_solo = session
                .sends
                .iter()
                .any(|other| other.bus_id == send.bus_id && other.controls.solo);
            sends.push(CompiledSend {
                id: send.id,
                input_index,
                bus_index,
                gains: ControlRamp::new(
                    if !send.controls.muted && (!send_solo || send.controls.solo) {
                        send.controls.gains()
                    } else {
                        [0.0; 2]
                    },
                ),
            });
        }
        Ok(Self {
            graph_token: NEXT_GRAPH_TOKEN
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |token| {
                    token.checked_add(1)
                })
                .map_err(|_| {
                    SessionError::Invalid("compiled graph identity space exhausted".into())
                })?,
            rendered_frames: None,
            sample_rate: session.sample_rate,
            control_ramp_frames: session.sample_rate.div_ceil(200),
            max_block_frames: budget.max_block_frames,
            working_bytes,
            inputs,
            buses,
            sends,
            input_meters,
            bus_meters,
        })
    }

    /// Apply one complete validated control bank without allocating, locking or
    /// rebuilding. All records are checked before any state changes; oscillator
    /// phase, DSP envelopes/filter history and route topology are preserved.
    /// Changed targets use a 5 ms linear amplitude ramp (rounded up to a whole
    /// frame). The next rendered sample advances one step; the last ramp sample
    /// reaches the exact target. Retargeting starts from the last rendered gain,
    /// and reapplying an unchanged target does not restart an existing ramp.
    /// Gain/pan ramp before input/bus DSP; mute/solo ramp after it. Sends ramp
    /// their combined resolved gain. Initial compiled controls apply exactly.
    pub fn apply_controls(
        &mut self,
        controls: &crate::PreparedControls,
    ) -> Result<(), crate::ControlError> {
        use crate::ControlError;
        if controls.inputs.len() != self.inputs.len()
            || controls.sends.len() != self.sends.len()
            || controls.buses.len() != self.buses.len()
            || controls
                .inputs
                .iter()
                .zip(&self.inputs)
                .any(|(c, target)| c.id != target.id)
            || controls
                .sends
                .iter()
                .zip(&self.sends)
                .any(|(c, target)| c.id != target.id)
            || controls
                .buses
                .iter()
                .zip(&self.buses)
                .any(|(c, target)| c.id != target.id)
        {
            return Err(ControlError::LayoutMismatch);
        }
        if controls
            .inputs
            .iter()
            .chain(&controls.sends)
            .chain(&controls.buses)
            .any(|c| !c.valid())
        {
            return Err(ControlError::InvalidGain);
        }
        for (target, c) in self.inputs.iter_mut().zip(&controls.inputs) {
            target.gains.retarget(c.gains, self.control_ramp_frames);
            target
                .activity
                .retarget(activity_gain(c.active), self.control_ramp_frames);
        }
        for (target, c) in self.sends.iter_mut().zip(&controls.sends) {
            target.gains.retarget(
                if c.active { c.gains } else { [0.0; 2] },
                self.control_ramp_frames,
            );
        }
        for (target, c) in self.buses.iter_mut().zip(&controls.buses) {
            target.gains.retarget(c.gains, self.control_ramp_frames);
            target
                .activity
                .retarget(activity_gain(c.active), self.control_ramp_frames);
        }
        Ok(())
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn max_block_frames(&self) -> usize {
        self.max_block_frames
    }
    /// Number of rendered frames in each live-control ramp: ceil(rate * 5 ms).
    /// Zero-frame renders and rejected renders do not advance the ramp.
    pub fn control_ramp_frames(&self) -> u32 {
        self.control_ramp_frames
    }
    pub fn working_bytes(&self) -> usize {
        self.working_bytes
    }
    pub fn input_meters(&self) -> &[MeterReading] {
        &self.input_meters
    }
    pub fn bus_meters(&self) -> &[MeterReading] {
        &self.bus_meters
    }

    /// Resolve a stable bus ID once on the control thread. This searches the
    /// compiled bus list; use the returned handle for constant-time callback
    /// access through [`Self::rendered_bus`]. No audio state advances here.
    pub fn resolve_bus(&self, id: Uuid) -> Result<BusHandle, RenderError> {
        self.buses
            .iter()
            .position(|bus| bus.id == id)
            .map(|index| BusHandle {
                graph_token: self.graph_token,
                index,
            })
            .ok_or(RenderError::UnknownBus(id))
    }

    /// Borrow one bus's post-control, post-DSP stereo frames in constant time.
    /// All buses are available, including those omitted from render outputs.
    /// Reusing a handle or reading several buses does not advance oscillators,
    /// ramps, processors or meters. This method allocates nothing and takes no
    /// locks; it checks only the graph token, index and valid frame extent.
    ///
    /// Before a successful render, or after any rejected render attempt, this
    /// returns [`BusReadError::NoSuccessfulRender`]. A successful zero-frame
    /// render returns an empty slice. A shorter render exposes only its own
    /// frames, never the previous block's tail. Applying controls does not alter
    /// the previously rendered block. The shared borrow prevents a new mutable
    /// render while these frames are still in use. Foreign handles are rejected
    /// before checking whether a readable block exists.
    pub fn rendered_bus(&self, handle: BusHandle) -> Result<&[[f32; 2]], BusReadError> {
        if handle.graph_token != self.graph_token {
            return Err(BusReadError::InvalidHandle);
        }
        let bus = self
            .buses
            .get(handle.index)
            .ok_or(BusReadError::InvalidHandle)?;
        let frames = self
            .rendered_frames
            .ok_or(BusReadError::NoSuccessfulRender)?;
        Ok(&bus.frames[..frames])
    }

    /// Render planar stereo buffers. Buffer order is arbitrary; IDs select the
    /// graph channels. Missing external inputs are silent, except test-tone
    /// inputs, which synthesize an internal sine. A provided buffer overrides a
    /// test tone. Output buses may be omitted; all buses still render and meter.
    /// Only the first `frames` elements are read/written.
    ///
    /// Input solo applies across inputs; send solo applies only within its
    /// destination bus; bus solo applies across buses. Mute always wins. Channel
    /// gain/pan precede DSP, and mute/solo follow it. Bus masters follow
    /// the sum of their sends. Live controls ramp as described by `apply_controls`.
    ///
    /// The callback makes no allocations, locks, I/O or graph changes. Buffer
    /// errors are detected before changing output or DSP state. The host should
    /// silence its device output if this method returns an error.
    pub fn render(
        &mut self,
        sources: &[StereoInput<'_>],
        outputs: &mut [StereoOutput<'_>],
        frames: usize,
    ) -> Result<(), RenderError> {
        self.render_sources(sources.iter().copied(), outputs, frames)
    }

    /// Render borrowed sources without assembling a temporary descriptor Vec.
    /// Use an immutable slice iterator or map over stable preallocated planes.
    /// Clones of `sources` must yield the same IDs and buffers throughout this
    /// call; the iterator is traversed for validation and channel lookup. The
    /// mixer itself does not allocate, but callers must also supply an iterator
    /// whose `clone` and `next` operations are suitable for an audio callback.
    /// Each attempt invalidates previous [`Self::rendered_bus`] access before
    /// validation. Only a complete successful render publishes a readable extent.
    pub fn render_sources<'a, I>(
        &mut self,
        sources: I,
        outputs: &mut [StereoOutput<'_>],
        frames: usize,
    ) -> Result<(), RenderError>
    where
        I: Iterator<Item = StereoInput<'a>> + Clone,
    {
        self.rendered_frames = None;
        self.validate_buffers(sources.clone(), outputs, frames)?;
        for (index, input) in self.inputs.iter_mut().enumerate() {
            let source = sources.clone().find(|source| source.id == input.id);
            for frame_index in 0..frames {
                let generated = input.tone.as_mut().map_or(0.0, Tone::next);
                let frame = source.map_or([generated; 2], |source| {
                    [source.left[frame_index], source.right[frame_index]]
                });
                let gains = input.gains.next();
                let activity = input.activity.next()[0];
                let processed = input
                    .processor
                    .process([sanitize(frame[0]) * gains[0], sanitize(frame[1]) * gains[1]]);
                input.frames[frame_index] = apply_activity(processed, activity);
            }
            self.input_meters[index].measure(&input.frames[..frames]);
        }
        for bus in &mut self.buses {
            bus.frames[..frames].fill([0.0; 2]);
        }
        for send in &mut self.sends {
            // Preserve the constant cost of a completed muted route. Testing
            // only its target would prematurely cut off an in-flight fade.
            if send.gains.is_stationary_zero() {
                continue;
            }
            let input = &self.inputs[send.input_index];
            let bus = &mut self.buses[send.bus_index];
            for frame_index in 0..frames {
                let gains = send.gains.next();
                bus.frames[frame_index][0] += input.frames[frame_index][0] * gains[0];
                bus.frames[frame_index][1] += input.frames[frame_index][1] * gains[1];
            }
        }
        for (index, bus) in self.buses.iter_mut().enumerate() {
            for frame in &mut bus.frames[..frames] {
                let gains = bus.gains.next();
                let activity = bus.activity.next()[0];
                let processed = bus
                    .processor
                    .process([sanitize(frame[0]) * gains[0], sanitize(frame[1]) * gains[1]]);
                *frame = apply_activity(processed, activity);
            }
            self.bus_meters[index].measure(&bus.frames[..frames]);
        }
        for output in outputs {
            // Already validated, and graph IDs cannot mutate during render.
            if let Some(bus) = self.buses.iter().find(|bus| bus.id == output.id) {
                for frame_index in 0..frames {
                    output.left[frame_index] = bus.frames[frame_index][0];
                    output.right[frame_index] = bus.frames[frame_index][1];
                }
            }
        }
        self.rendered_frames = Some(frames);
        Ok(())
    }

    fn validate_buffers<'a, I>(
        &self,
        sources: I,
        outputs: &[StereoOutput<'_>],
        frames: usize,
    ) -> Result<(), RenderError>
    where
        I: Iterator<Item = StereoInput<'a>> + Clone,
    {
        if frames > self.max_block_frames {
            return Err(RenderError::BlockTooLarge {
                requested: frames,
                maximum: self.max_block_frames,
            });
        }
        for (index, source) in sources.clone().enumerate() {
            if !self.inputs.iter().any(|input| input.id == source.id) {
                return Err(RenderError::UnknownInput(source.id));
            }
            if sources
                .clone()
                .take(index)
                .any(|other| other.id == source.id)
            {
                return Err(RenderError::DuplicateInput(source.id));
            }
            let available = source.left.len().min(source.right.len());
            if available < frames {
                return Err(RenderError::BufferTooShort {
                    id: source.id,
                    required: frames,
                    available,
                });
            }
        }
        for (index, output) in outputs.iter().enumerate() {
            if !self.buses.iter().any(|bus| bus.id == output.id) {
                return Err(RenderError::UnknownBus(output.id));
            }
            if outputs[..index].iter().any(|other| other.id == output.id) {
                return Err(RenderError::DuplicateBus(output.id));
            }
            let available = output.left.len().min(output.right.len());
            if available < frames {
                return Err(RenderError::BufferTooShort {
                    id: output.id,
                    required: frames,
                    available,
                });
            }
        }
        Ok(())
    }
}

fn activity_gain(active: bool) -> [f32; 1] {
    [if active { 1.0 } else { 0.0 }]
}

fn apply_activity(frame: Frame, gain: f32) -> Frame {
    if gain == 0.0 {
        [0.0; 2]
    } else {
        [frame[0] * gain, frame[1] * gain]
    }
}

fn reserved_vec<T>(capacity: usize) -> Result<Vec<T>, SessionError> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(capacity)
        .map_err(|_| SessionError::AllocationFailed)?;
    Ok(result)
}

fn frame_buffer(frames: usize) -> Result<Vec<Frame>, SessionError> {
    let mut result = reserved_vec(frames)?;
    result.resize(frames, [0.0; 2]);
    Ok(result)
}
