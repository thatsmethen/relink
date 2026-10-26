//! Explicit-device, stopped-by-default audio runtime for macOS and Windows.
//!
//! Blocking control methods must run away from the GUI thread. One dedicated
//! thread discovers explicit devices; original CoreAudio and WASAPI owners
//! manage capture/output with bounded control waits and separate progress
//! supervision. Callbacks use a preallocated mixer,
//! planar buffers and atomic telemetry, with no application locks or allocation.
//! A command deadline cancels late output; it cannot kill a stuck OS call. A
//! supervising process is still required for hard recovery from driver hangs.

mod capture;
mod capture_transport;
mod control_bank;
mod epoch_gate;
mod output_render;
use epoch_gate::EpochGate;
use output_render::{FanoutRender, OutputRenderer, OutputTelemetry};
mod output_sink;
#[cfg(test)]
use output_sink::InterleavedSink;
use output_sink::{RenderOutcome, StereoSink};
mod output;
pub use output::OutputTimingSnapshot;
mod output_plan;
mod output_progress;
mod output_startup;
pub use output_plan::{OutputPlan, OutputPlanError, OutputRoute};
#[allow(dead_code)]
mod fanout;
mod fault;
pub use fault::{FaultAttribution, FaultDirection, FaultMetadata, FaultOrigin, FaultProperty};
use fault::{FaultLatch, FaultSource};
#[cfg(target_os = "macos")]
mod native_capture;
#[cfg(target_os = "macos")]
mod native_output;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod native_output_timing;
#[cfg(target_os = "macos")]
pub use native_output_timing::{
    NativeOutputCompletion, NativeOutputFault, NativeOutputFaultReason, NativeOutputLifecycleEvent,
    NativeOutputLifecyclePhase, NativeOutputOutcome, NativeOutputPacket, NativeOutputStamp,
    NativeOutputTimingSnapshot, NATIVE_OUTPUT_LIFECYCLE_PHASES,
};
#[cfg(any(target_os = "macos", test))]
mod native_packet;
// Portable PCM encoder used by the original CoreAudio output owner.
#[cfg(any(target_os = "macos", test))]
#[allow(dead_code)]
mod native_output_packet;
mod native_timing;
pub use native_timing::{
    NativeCallbackCompletion, NativeCallbackOutcome, NativeCaptureFault, NativeCapturePacket,
    NativeLifecycleEvent, NativeLifecyclePhase, NativeTimingSnapshot, NATIVE_LIFECYCLE_PHASES,
};
#[cfg(target_os = "windows")]
mod wasapi_capture;
#[cfg(target_os = "windows")]
mod wasapi_output;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod wasapi_output_state;
#[cfg(target_os = "windows")]
pub use wasapi_output_state::{
    WasapiOutputCall, WasapiOutputCompletion, WasapiOutputEvent, WasapiOutputOperation,
    WasapiOutputPhase, WasapiOutputSnapshot, WasapiOutputWork, WASAPI_OUTPUT_PHASES,
};
#[cfg(any(target_os = "windows", test))]
mod wasapi_packet;
// Portable PCM encoder/lease used by the original WASAPI output owner.
#[cfg(any(target_os = "windows", test))]
#[allow(dead_code)]
mod wasapi_render_packet;
use capture::{CaptureRender, CaptureState};
use control_bank::ControlBank;

use cpal::traits::{DeviceTrait, HostTrait};
use ravelink_core::{
    CompiledMixer, InputKind, MeterReading, PreparedControls, ResourceBudget, Session, StereoInput,
    StereoOutput, Uuid,
};
use std::{
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const RENDER_CHUNK: usize = 2048;
const MAX_CALLBACK_FRAMES: usize = 16384;
// Physical PCM layouts are bounded independently of logical channel count.
const MAX_DEVICE_CHANNELS: u16 = 256;
const GRAPH_BYTES: usize = 64 * 1024 * 1024;

/// CPAL's host-qualified native ID. Stable where the OS/backend permits; never a
/// display name or a request to follow the system default device.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OutputId(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputDevice {
    pub id: OutputId,
    pub name: String,
}

/// Host-qualified native input identity, never a display name/default alias.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InputId(pub String);
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDevice {
    pub id: InputId,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningInput {
    pub device: InputDevice,
    /// Several logical channels can share one physical capture stream.
    pub source_ids: Vec<Uuid>,
    pub device_channels: u16,
    /// Mono is duplicated; multichannel input uses channels 1 and 2.
    pub sample_format: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureSnapshot {
    pub device: InputId,
    pub captured_frames: u64,
    pub buffered_frames: u64,
    pub clock_correction_ppm: f64,
    /// Native macOS packet evidence, retained after a fault. Other backends
    /// currently return None. Reading this never waits for an audio callback.
    pub native_timing: Option<NativeTimingSnapshot>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningOutput {
    pub output: OutputDevice,
    pub bus_id: Uuid,
    pub sample_rate: u32,
    pub device_channels: u16,
    pub sample_format: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningConfiguration {
    pub outputs: Vec<RunningOutput>,
    pub clock_output_index: usize,
    pub output: OutputDevice,
    pub inputs: Vec<RunningInput>,
    pub bus_id: Uuid,
    pub sample_rate: u32,
    pub device_channels: u16,
    /// The chosen mix is sent to channels 1–2; remaining device channels are silent.
    pub sample_format: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    UnsupportedPlatform,
    RuntimeUnavailable,
    Timeout {
        operation: &'static str,
    },
    AlreadyRunning,
    NotRunning,
    InvalidSession(String),
    InvalidOutputId(String),
    InvalidInputId(String),
    InputUnavailable(InputId),
    UnsupportedInputConfiguration {
        device: String,
        sample_rate: u32,
    },
    DeviceUnavailable(OutputId),
    UnsupportedConfiguration {
        device: String,
        sample_rate: u32,
    },
    Backend {
        operation: &'static str,
        device: Option<String>,
        detail: String,
    },
    Cancelled,
}
impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => f.write_str("Audio is supported on macOS/CoreAudio and Windows/WASAPI."),
            Self::RuntimeUnavailable => f.write_str("The audio control thread is unavailable. Restart the audio runtime."),
            Self::Timeout { operation: "device discovery" } => f.write_str("Audio device discovery exceeded its deadline. The device list was not refreshed; existing playback was not changed."),
            Self::Timeout { operation } => write!(f, "Audio {operation} exceeded its deadline. Output was cancelled; the device may be unresponsive."),
            Self::NotRunning => f.write_str("Start audio before updating live controls."),
            Self::AlreadyRunning => f.write_str("Stop audio before starting a different session or output."),
            Self::InvalidSession(reason) => write!(f, "Cannot start this mix: {reason}"),
            Self::InvalidInputId(id) => write!(f, "The selected input identity is invalid for this platform: {id}"),
            Self::InputUnavailable(id) => write!(f, "The selected input is unavailable: {}. Reconnect it or explicitly select another input.", id.0),
            Self::UnsupportedInputConfiguration { device, sample_rate } => write!(f, "{device} does not expose supported PCM input at {sample_rate} Hz. Choose a compatible input or session sample rate."),
            Self::InvalidOutputId(id) => write!(f, "The selected output identity is invalid for this platform: {id}"),
            Self::DeviceUnavailable(id) => write!(f, "The selected output is unavailable: {}. Reconnect it or explicitly select another output.", id.0),
            Self::UnsupportedConfiguration { device, sample_rate } => write!(f, "{device} does not expose a supported stereo PCM output at {sample_rate} Hz. Choose a compatible output or session sample rate."),
            Self::Backend { operation, device, detail } => {
                write!(f, "Audio {operation} failed")?;
                if let Some(device) = device { write!(f, " for {device}")?; }
                write!(f, ": {detail}")
            }
            Self::Cancelled => f.write_str("Audio start was cancelled before activation."),
        }
    }
}
impl std::error::Error for AudioError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum AudioFault {
    None = 0,
    DeviceUnavailable = 1,
    DeviceChanged = 2,
    PermissionDenied = 3,
    StreamInvalidated = 4,
    Xrun = 5,
    Backend = 6,
    InvalidBuffer = 7,
    Render = 8,
    RealtimeDenied = 9,
    ResourceExhausted = 10,
    CaptureUnderflow = 11,
    CaptureOverflow = 12,
    CaptureStartup = 13,
    CaptureTimestamp = 14,
}
impl AudioFault {
    fn from_code(code: u32) -> Self {
        match code {
            0 => Self::None,
            1 => Self::DeviceUnavailable,
            2 => Self::DeviceChanged,
            3 => Self::PermissionDenied,
            4 => Self::StreamInvalidated,
            5 => Self::Xrun,
            7 => Self::InvalidBuffer,
            8 => Self::Render,
            9 => Self::RealtimeDenied,
            10 => Self::ResourceExhausted,
            11 => Self::CaptureUnderflow,
            12 => Self::CaptureOverflow,
            13 => Self::CaptureStartup,
            14 => Self::CaptureTimestamp,
            _ => Self::Backend,
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::None => "",
            Self::CaptureUnderflow => "An input could not keep up with output. Audio stopped rather than dropping captured samples; check device clocks and load.",
            Self::CaptureOverflow => "An input exceeded its capture buffer. Audio stopped rather than overwriting captured samples; check device clocks and load.",
            Self::CaptureTimestamp => "An input reported invalid capture timing. Audio stopped; check the input device and restart audio.",
            Self::CaptureStartup => "A selected input did not deliver audio before its startup deadline. Check input permissions and the selected device.",
            Self::DeviceUnavailable => "A selected audio device disconnected. Reconnect it and start explicitly.",
            Self::DeviceChanged => "An audio device route changed unexpectedly. Output stopped; select the intended device again.",
            Self::PermissionDenied => "The operating system denied audio access. Check the app's audio permissions.",
            Self::StreamInvalidated => "An audio device format or stream became invalid. Check the device settings and start again.",
            Self::Xrun => "The device reported a buffer underrun or overrun. Output stopped; reduce audio load before restarting.",
            Self::Backend => "The audio backend reported a stream failure. Check the selected device before restarting.",
            Self::InvalidBuffer => "The device supplied an unsupported callback buffer. Output stopped.",
            Self::Render => "The mixer could not render the requested buffer. Output stopped.",
            Self::RealtimeDenied => "Real-time audio scheduling was denied. Output stopped before continuing with degraded scheduling.",
            Self::ResourceExhausted => "The operating system exhausted audio resources. Close other audio streams before restarting.",
        }
    }
}

/// Per-route software preparation and native submission observations. These
/// counters do not establish acoustic playback or alignment between devices.
#[derive(Clone, Debug)]
pub struct OutputSnapshot {
    pub route_index: usize,
    pub output: OutputId,
    pub bus_id: Uuid,
    pub is_clock: bool,
    pub ready: bool,
    pub rendered_frames: u64,
    pub submitted_frames: u64,
    pub rendered_control_version: Option<u64>,
    pub submitted_control_version: Option<u64>,
    pub source_position: u64,
    pub buffered_frames: u64,
    pub target_frames: usize,
    pub clock_correction_ppm: f64,
    pub timing: OutputTimingSnapshot,
}

#[derive(Clone, Debug)]
pub struct AudioSnapshot {
    pub outputs: Vec<OutputSnapshot>,
    pub running: bool,
    pub configuration: Option<RunningConfiguration>,
    pub rendered_frames: u64,
    pub callback_count: u64,
    /// Accepted target version used by a complete, nonempty, authorized render.
    /// This is not hardware playback, ramp completion, or proof that every
    /// intermediate (possibly coalesced) control request was rendered.
    pub applied_control_version: u64,
    pub captures: Vec<CaptureSnapshot>,
    /// Original output-owner observations, retained after stop or failed startup.
    pub output_timing: Option<OutputTimingSnapshot>,
    pub input_meters: Vec<MeterReading>,
    pub bus_meters: Vec<MeterReading>,
    pub fault: AudioFault,
    /// Write-once source attribution; metadata can briefly be pending.
    pub first_fault: Option<FaultAttribution>,
    pub error: Option<AudioError>,
}

struct AtomicMeter {
    id: Uuid,
    peak: [AtomicU32; 2],
    rms: [AtomicU32; 2],
    clipped: [AtomicU64; 2],
}
impl AtomicMeter {
    fn new(id: Uuid) -> Self {
        Self {
            id,
            peak: std::array::from_fn(|_| AtomicU32::new(0)),
            rms: std::array::from_fn(|_| AtomicU32::new(0)),
            clipped: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
    fn publish(&self, meter: &MeterReading) {
        for side in 0..2 {
            self.peak[side].store(meter.peak[side].to_bits(), Ordering::Relaxed);
            self.rms[side].store(meter.rms[side].to_bits(), Ordering::Relaxed);
            self.clipped[side].fetch_add(meter.clipped_samples[side], Ordering::Relaxed);
        }
    }
    fn read(&self, audible: bool) -> MeterReading {
        MeterReading {
            id: self.id,
            peak: std::array::from_fn(|i| {
                if audible {
                    f32::from_bits(self.peak[i].load(Ordering::Relaxed))
                } else {
                    0.0
                }
            }),
            rms: std::array::from_fn(|i| {
                if audible {
                    f32::from_bits(self.rms[i].load(Ordering::Relaxed))
                } else {
                    0.0
                }
            }),
            clipped_samples: std::array::from_fn(|i| self.clipped[i].load(Ordering::Relaxed)),
        }
    }
}
struct Telemetry {
    gate: Arc<EpochGate>,
    outputs: Vec<Arc<OutputTelemetry>>,
    fault: FaultLatch,
    frames: AtomicU64,
    callbacks: AtomicU64,
    applied_control_version: AtomicU64,
    captures: Vec<(InputId, Arc<capture_transport::Ring>)>,
    inputs: Vec<AtomicMeter>,
    buses: Vec<AtomicMeter>,
}
impl Telemetry {
    fn new(session: &Session) -> Self {
        Self::with_gate(
            session,
            Arc::new(EpochGate::new(Arc::new(AtomicBool::new(true)))),
        )
    }
    fn with_gate(session: &Session, gate: Arc<EpochGate>) -> Self {
        Self {
            gate,
            outputs: Vec::new(),
            fault: FaultLatch::default(),
            frames: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
            applied_control_version: AtomicU64::new(0),
            captures: Vec::new(),
            inputs: session
                .inputs
                .iter()
                .map(|i| AtomicMeter::new(i.id))
                .collect(),
            buses: session
                .buses
                .iter()
                .map(|b| AtomicMeter::new(b.id))
                .collect(),
        }
    }
    fn record_fault(
        &self,
        fault: AudioFault,
        origin: FaultOrigin,
        source: FaultSource,
        metadata: FaultMetadata,
    ) {
        self.fault.record(fault, origin, source, metadata);
        self.gate.cancel();
    }
    #[cfg(test)]
    fn fail(&self, fault: AudioFault) {
        self.fault.record(
            fault,
            FaultOrigin::Runtime,
            FaultSource::RUNTIME,
            FaultMetadata::now(0),
        );
    }
}
#[derive(Default)]
struct ControlState {
    running: bool,
    configuration: Option<RunningConfiguration>,
    telemetry: Option<Arc<Telemetry>>,
    error: Option<AudioError>,
    authorization: Option<(Arc<AtomicBool>, u64)>,
    output_diagnostics: Vec<output::Diagnostics>,
}
struct Shared {
    active_gate: arc_swap::ArcSwapOption<EpochGate>,
    state: Mutex<ControlState>,
    epoch: AtomicU64,
}
impl Shared {
    fn state(&self) -> std::sync::MutexGuard<'_, ControlState> {
        // No callback takes this lock. Recovering control metadata cannot enable audio.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
type Reply<T> = mpsc::SyncSender<Result<T, AudioError>>;
enum Command {
    Enumerate(Reply<Vec<OutputDevice>>),
    EnumerateInputs(Reply<Vec<InputDevice>>),
    Start {
        session: Session,
        plan: OutputPlan,
        epoch: u64,
        gate: Arc<EpochGate>,
        reply: Reply<RunningConfiguration>,
    },
    Update {
        session: Session,
        epoch: u64,
        wanted: Arc<AtomicBool>,
        reply: Reply<u64>,
    },
    Stop(Reply<()>),
    Shutdown,
}

/// A control handle; creation never enumerates devices or starts audio.
/// Methods wait at most five seconds and should be called from a UI worker.
pub struct AudioRuntime {
    commands: mpsc::SyncSender<Command>,
    shared: Arc<Shared>,
}

/// Cancellation for one start request, including time spent in a UI queue.
/// Create a fresh token for each start. Cancellation is permanent and also
/// silences future callbacks if that start has already become active.
#[derive(Clone)]
pub struct StartCancellation(Arc<EpochGate>);
impl Default for StartCancellation {
    fn default() -> Self {
        Self(Arc::new(EpochGate::new(Arc::new(AtomicBool::new(true)))))
    }
}
impl StartCancellation {
    pub fn cancel(&self) {
        self.0.cancel();
    }
    pub fn is_cancelled(&self) -> bool {
        !self.0.wanted.load(Ordering::Acquire)
    }
}
impl AudioRuntime {
    pub fn new() -> Result<Self, AudioError> {
        let (commands, receiver) = mpsc::sync_channel(8);
        let shared = Arc::new(Shared {
            active_gate: arc_swap::ArcSwapOption::empty(),
            state: Mutex::new(ControlState::default()),
            epoch: AtomicU64::new(0),
        });
        let owner = shared.clone();
        thread::Builder::new()
            .name("ravelink-audio-control".into())
            .spawn(move || runtime_loop(receiver, owner))
            .map_err(|error| AudioError::Backend {
                operation: "thread creation",
                device: None,
                detail: error.to_string(),
            })?;
        Ok(Self { commands, shared })
    }
    pub fn enumerate_output_devices(&self) -> Result<Vec<OutputDevice>, AudioError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Command::Enumerate(tx))?;
        receive(rx, "device discovery")
    }
    pub fn enumerate_input_devices(&self) -> Result<Vec<InputDevice>, AudioError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Command::EnumerateInputs(tx))?;
        receive(rx, "device discovery")
    }
    pub fn start(
        &self,
        session: Session,
        bus_id: Uuid,
        output_id: OutputId,
    ) -> Result<RunningConfiguration, AudioError> {
        self.start_cancellable(session, bus_id, output_id, StartCancellation::default())
    }
    pub fn start_cancellable(
        &self,
        session: Session,
        bus_id: Uuid,
        output_id: OutputId,
        cancellation: StartCancellation,
    ) -> Result<RunningConfiguration, AudioError> {
        if cancellation.is_cancelled() {
            return Err(AudioError::Cancelled);
        }
        validate_session(&session, bus_id)?;
        self.start_outputs_cancellable(session, OutputPlan::single(output_id, bus_id), cancellation)
    }
    pub fn start_outputs(
        &self,
        session: Session,
        plan: OutputPlan,
    ) -> Result<RunningConfiguration, AudioError> {
        self.start_outputs_cancellable(session, plan, StartCancellation::default())
    }
    pub fn start_outputs_cancellable(
        &self,
        session: Session,
        plan: OutputPlan,
        cancellation: StartCancellation,
    ) -> Result<RunningConfiguration, AudioError> {
        if cancellation.is_cancelled() {
            return Err(AudioError::Cancelled);
        }
        output_render::validate_plan_budget(&plan)?;
        plan.validate(&session)
            .map_err(|e| AudioError::InvalidSession(e.to_string()))?;
        let gate = cancellation.0;
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Command::Start {
            session,
            plan,
            epoch: self.shared.epoch.load(Ordering::Acquire),
            gate: gate.clone(),
            reply: tx,
        })?;
        let result = receive(rx, "start");
        if result.is_err() {
            gate.cancel();
        }
        result
    }
    /// Publish gain/pan/link/mute/solo changes for the current graph. Returns a
    /// bank version; `snapshot().applied_control_version` acknowledges callback
    /// application. A timeout cancels output rather than applying a queued edit
    /// later. Sources, route identities/order, DSP and sample rate must match.
    pub fn update_controls(&self, session: Session) -> Result<u64, AudioError> {
        let wanted = Arc::new(AtomicBool::new(true));
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Command::Update {
            session,
            epoch: self.shared.epoch.load(Ordering::Acquire),
            wanted: wanted.clone(),
            reply: tx,
        })?;
        let result = receive(rx, "control update");
        if result.is_err() {
            wanted.store(false, Ordering::Release);
        }
        if matches!(
            result,
            Err(AudioError::Timeout { .. } | AudioError::RuntimeUnavailable)
        ) {
            let revoked = self.shared.epoch.fetch_add(1, Ordering::AcqRel);
            if let Some(gate) = &*self.shared.active_gate.load() {
                if gate.epoch.load(Ordering::Acquire) <= revoked {
                    gate.cancel();
                }
            }
            self.shared.state().running = false;
        }
        result
    }
    /// Revoke output immediately without waiting on any device call, lock or
    /// command queue. Use this for emergency stop and UI teardown, then request
    /// `stop` off the UI thread to release native resources. Samples already
    /// handed to the device cannot be recalled.
    pub fn cancel_output(&self) {
        let revoked = self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        if let Some(gate) = &*self.shared.active_gate.load() {
            if gate.epoch.load(Ordering::Acquire) <= revoked {
                gate.cancel();
            }
        }
    }
    /// Immediately prevents subsequent callback output, then asks the owner to
    /// release the stream. Already handed-off hardware samples cannot be recalled.
    /// Success acknowledges cancellation and handle disposal, not completed native
    /// teardown. Retained `output_timing` reports completion or unresolved ownership.
    pub fn stop(&self) -> Result<(), AudioError> {
        let revoked = self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        if let Some(gate) = &*self.shared.active_gate.load() {
            if gate.epoch.load(Ordering::Acquire) <= revoked {
                gate.cancel();
            }
        }
        self.shared.state().running = false;
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Command::Stop(tx))?;
        receive(rx, "stop")
    }
    pub fn snapshot(&self) -> AudioSnapshot {
        let state = self.shared.state();
        let mut first_fault = state.telemetry.as_ref().and_then(|t| t.fault.snapshot());
        if let Some(record) = &mut first_fault {
            record.endpoint_id = match record.direction {
                FaultDirection::Input => state.telemetry.as_ref().and_then(|t| {
                    t.captures
                        .get(record.endpoint_index? as usize)
                        .map(|(id, _)| id.0.clone())
                }),
                FaultDirection::Output => state
                    .configuration
                    .as_ref()
                    .and_then(|c| c.outputs.get(record.endpoint_index? as usize))
                    .map(|o| o.output.id.0.clone()),
                FaultDirection::Runtime => None,
            };
        }
        let fault = first_fault
            .as_ref()
            .map_or(AudioFault::None, |record| record.fault);
        let authorized = state.authorization.as_ref().is_some_and(|(wanted, epoch)| {
            wanted.load(Ordering::Acquire) && self.shared.epoch.load(Ordering::Acquire) == *epoch
        });
        let running = state.running && authorized && fault == AudioFault::None;
        let (frames, callbacks, inputs, buses) =
            state
                .telemetry
                .as_ref()
                .map_or((0, 0, Vec::new(), Vec::new()), |t| {
                    (
                        t.frames.load(Ordering::Relaxed),
                        t.callbacks.load(Ordering::Relaxed),
                        t.inputs.iter().map(|m| m.read(running)).collect(),
                        t.buses.iter().map(|m| m.read(running)).collect(),
                    )
                });
        let outputs = state
            .configuration
            .as_ref()
            .zip(state.telemetry.as_ref())
            .map_or_else(Vec::new, |(configuration, telemetry)| {
                configuration
                    .outputs
                    .iter()
                    .zip(&telemetry.outputs)
                    .zip(&state.output_diagnostics)
                    .enumerate()
                    .map(|(index, ((route, counters), timing))| {
                        counters.snapshot(
                            index,
                            route,
                            index == configuration.clock_output_index,
                            timing.snapshot(),
                        )
                    })
                    .collect()
            });
        AudioSnapshot {
            outputs,
            running,
            configuration: state.configuration.clone(),
            rendered_frames: frames,
            callback_count: callbacks,
            applied_control_version: state
                .telemetry
                .as_ref()
                .map_or(0, |t| t.applied_control_version.load(Ordering::Acquire)),
            captures: state.telemetry.as_ref().map_or_else(Vec::new, |t| {
                t.captures
                    .iter()
                    .map(|(id, ring)| CaptureSnapshot {
                        device: id.clone(),
                        captured_frames: ring.captured.load(Ordering::Relaxed),
                        buffered_frames: ring.buffered.load(Ordering::Relaxed),
                        clock_correction_ppm: f64::from_bits(
                            ring.correction_ppm.load(Ordering::Relaxed),
                        ),
                        native_timing: {
                            #[cfg(target_os = "macos")]
                            {
                                ring.native_timing.snapshot()
                            }
                            #[cfg(not(target_os = "macos"))]
                            {
                                None
                            }
                        },
                    })
                    .collect()
            }),
            output_timing: state
                .configuration
                .as_ref()
                .and_then(|c| state.output_diagnostics.get(c.clock_output_index))
                .map(output::Diagnostics::snapshot),
            input_meters: inputs,
            bus_meters: buses,
            fault,
            first_fault,
            error: state.error.clone(),
        }
    }
    fn send(&self, command: Command) -> Result<(), AudioError> {
        self.commands
            .try_send(command)
            .map_err(|_| AudioError::RuntimeUnavailable)
    }
}
impl Drop for AudioRuntime {
    fn drop(&mut self) {
        let revoked = self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        if let Some(gate) = &*self.shared.active_gate.load() {
            if gate.epoch.load(Ordering::Acquire) <= revoked {
                gate.cancel();
            }
        }
        let _ = self.commands.try_send(Command::Shutdown);
        // Do not join a thread that may be blocked in an OS driver call. Callback
        // gates are revoked immediately; the thread releases resources when able.
    }
}
fn receive<T>(
    receiver: mpsc::Receiver<Result<T, AudioError>>,
    operation: &'static str,
) -> Result<T, AudioError> {
    match receiver.recv_timeout(COMMAND_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(AudioError::Timeout { operation }),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(AudioError::RuntimeUnavailable),
    }
}

fn native_host() -> Result<cpal::Host, AudioError> {
    #[cfg(target_os = "macos")]
    let host = cpal::host_from_id(cpal::HostId::CoreAudio);
    #[cfg(target_os = "windows")]
    let host = cpal::host_from_id(cpal::HostId::Wasapi);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Err(AudioError::UnsupportedPlatform);
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    host.map_err(|e| backend("host initialization", None, e))
}
fn backend(
    operation: &'static str,
    device: Option<String>,
    error: impl fmt::Display,
) -> AudioError {
    AudioError::Backend {
        operation,
        device,
        detail: error.to_string(),
    }
}
fn enumerate() -> Result<Vec<OutputDevice>, AudioError> {
    let host = native_host()?;
    let devices = host
        .output_devices()
        .map_err(|e| backend("device discovery", None, e))?;
    let mut outputs = Vec::new();
    for device in devices {
        let id = device
            .id()
            .map_err(|e| backend("device identity", None, e))?;
        let name = device
            .description()
            .map_err(|e| backend("device description", Some(id.to_string()), e))?
            .name()
            .to_owned();
        outputs.push(OutputDevice {
            id: OutputId(id.to_string()),
            name,
        });
    }
    outputs.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.0.cmp(&b.id.0)));
    Ok(outputs)
}
fn validate_session(session: &Session, bus: Uuid) -> Result<(), AudioError> {
    session
        .validate()
        .map_err(|e| AudioError::InvalidSession(e.to_string()))?;
    if !session.buses.iter().any(|b| b.id == bus) {
        return Err(AudioError::InvalidSession(
            "the selected mix no longer exists".into(),
        ));
    }
    if session
        .inputs
        .iter()
        .any(|i| matches!(i.kind, InputKind::Application { .. }))
    {
        return Err(AudioError::InvalidSession("application capture requires a platform process tap and is not yet implemented; select a hardware or virtual input device".into()));
    }
    for id in capture::source_groups(session).keys() {
        if id.parse::<cpal::DeviceId>().is_err() {
            return Err(AudioError::InvalidInputId(id.clone()));
        }
    }
    Ok(())
}
fn prepare_live_update(current: &Session, next: &Session) -> Result<PreparedControls, AudioError> {
    let same = current.id == next.id
        && current.schema_version == next.schema_version
        && current.sample_rate == next.sample_rate
        && current.inputs.len() == next.inputs.len()
        && current.buses.len() == next.buses.len()
        && current.sends.len() == next.sends.len()
        && current
            .inputs
            .iter()
            .zip(&next.inputs)
            .all(|(a, b)| a.id == b.id && a.kind == b.kind && a.dsp == b.dsp)
        && current
            .buses
            .iter()
            .zip(&next.buses)
            .all(|(a, b)| a.id == b.id && a.dsp == b.dsp)
        && current
            .sends
            .iter()
            .zip(&next.sends)
            .all(|(a, b)| a.id == b.id && a.input_id == b.input_id && a.bus_id == b.bus_id);
    if !same {
        return Err(AudioError::InvalidSession("sources, effects, route topology or sample rate changed; stop audio before applying this edit".into()));
    }
    PreparedControls::compile(next, GRAPH_BYTES)
        .map_err(|e| AudioError::InvalidSession(e.to_string()))
}

#[cfg(any(target_os = "windows", test))]
fn select_config(
    configs: impl Iterator<Item = cpal::SupportedStreamConfigRange>,
    sample_rate: u32,
) -> Option<cpal::SupportedStreamConfig> {
    configs
        .filter(|c| {
            (2..=MAX_DEVICE_CHANNELS).contains(&c.channels())
                && wasapi_packet::Encoding::from_cpal(c.sample_format()).is_some()
        })
        .filter_map(|c| c.try_with_sample_rate(sample_rate))
        .min_by_key(|c| {
            (
                c.channels(),
                match c.sample_format() {
                    cpal::SampleFormat::F32 => 0,
                    cpal::SampleFormat::F64 => 1,
                    _ => 2,
                },
            )
        })
}
#[cfg(any(not(target_os = "macos"), test))]
fn supported_pcm(format: cpal::SampleFormat) -> bool {
    matches!(
        format,
        cpal::SampleFormat::F32
            | cpal::SampleFormat::F64
            | cpal::SampleFormat::I8
            | cpal::SampleFormat::I16
            | cpal::SampleFormat::I24
            | cpal::SampleFormat::I32
            | cpal::SampleFormat::I64
            | cpal::SampleFormat::U8
            | cpal::SampleFormat::U16
            | cpal::SampleFormat::U24
            | cpal::SampleFormat::U32
            | cpal::SampleFormat::U64
    )
}

struct Running {
    _streams: Vec<output::Stream>,
    _input_streams: InputStreams,
    session: Session,
    controls: Arc<ControlBank>,
    wanted: Arc<AtomicBool>,
    telemetry: Arc<Telemetry>,
    epoch: u64,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.telemetry.gate.cancel();
    }
}
/// Revoke publication before dropping native capture resources, including when
/// a later stream fails to build/start and a driver blocks during destruction.
struct InputStreams {
    streams: Vec<capture::CaptureStream>,
    wanted: Arc<AtomicBool>,
}
impl Drop for InputStreams {
    fn drop(&mut self) {
        self.wanted.store(false, Ordering::Release);
    }
}

fn runtime_loop(receiver: mpsc::Receiver<Command>, shared: Arc<Shared>) {
    let mut running: Option<Running> = None;
    loop {
        if running.as_ref().is_some_and(|r| {
            r.telemetry.fault.load(Ordering::Acquire) != 0
                || !r.wanted.load(Ordering::Acquire)
                || shared.epoch.load(Ordering::Acquire) != r.epoch
        }) {
            if let Some(old) = running.take() {
                old.telemetry.gate.cancel();
                drop(old);
            }
            shared.state().running = false;
        }
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(Command::Enumerate(reply)) => {
                let _ = reply.send(enumerate());
            }
            Ok(Command::EnumerateInputs(reply)) => {
                let _ = reply.send(capture::enumerate());
            }
            Ok(Command::Start {
                session,
                plan,
                epoch,
                gate,
                reply,
            }) => {
                let result = if running.is_some() {
                    Err(AudioError::AlreadyRunning)
                } else {
                    start_stream(session, plan, epoch, gate, shared.clone())
                };
                match result {
                    Ok((owner, configuration)) => {
                        {
                            let mut state = shared.state();
                            state.running = true;
                            state.configuration = Some(configuration.clone());
                            state.telemetry = Some(owner.telemetry.clone());
                            state.error = None;
                            state.authorization = Some((owner.wanted.clone(), owner.epoch));
                        }
                        if reply.send(Ok(configuration)).is_err() {
                            owner.telemetry.gate.cancel();
                        }
                        running = Some(owner);
                    }
                    Err(error) => {
                        if running.is_none() {
                            shared.state().error = Some(error.clone());
                        }
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Ok(Command::Update {
                session,
                epoch,
                wanted,
                reply,
            }) => {
                let result = match running.as_mut() {
                    Some(owner) => {
                        prepare_live_update(&owner.session, &session).and_then(|prepared| {
                            if !wanted.load(Ordering::Acquire)
                                || epoch != owner.epoch
                                || shared.epoch.load(Ordering::Acquire) != epoch
                            {
                                return Err(AudioError::Cancelled);
                            }
                            let version = owner.controls.publish(&prepared)?;
                            owner.session = session;
                            Ok(version)
                        })
                    }
                    None => Err(AudioError::NotRunning),
                };
                let _ = reply.send(result);
            }
            Ok(Command::Stop(reply)) => {
                if let Some(old) = running.take() {
                    old.telemetry.gate.cancel();
                    drop(old);
                }
                let mut state = shared.state();
                state.running = false;
                state.error = None;
                let _ = reply.send(Ok(()));
            }
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    if let Some(old) = running {
        old.telemetry.gate.cancel();
    }
}

fn start_stream(
    session: Session,
    plan: OutputPlan,
    epoch: u64,
    gate: Arc<EpochGate>,
    shared: Arc<Shared>,
) -> Result<(Running, RunningConfiguration), AudioError> {
    struct RevokeOnFailure(Option<Arc<EpochGate>>);
    impl Drop for RevokeOnFailure {
        fn drop(&mut self) {
            if let Some(gate) = &self.0 {
                gate.cancel();
            }
        }
    }
    let mut guard = RevokeOnFailure(Some(gate.clone()));
    let wanted = gate.wanted.clone();
    if !wanted.load(Ordering::Acquire) {
        return Err(AudioError::Cancelled);
    }
    let clock_index = plan
        .validate(&session)
        .map_err(|e| AudioError::InvalidSession(e.to_string()))?;
    let bus = plan.routes[clock_index].bus_id;
    validate_session(&session, bus)?;
    let next_epoch = epoch.checked_add(1).ok_or(AudioError::RuntimeUnavailable)?;
    shared
        .epoch
        .compare_exchange(epoch, next_epoch, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| AudioError::Cancelled)?;
    let epoch = next_epoch;
    gate.epoch.store(epoch, Ordering::Release);
    shared.active_gate.store(Some(gate.clone()));
    let host = native_host()?;
    let mut discovered = Vec::new();
    for device in host
        .devices()
        .map_err(|e| backend("device discovery", None, e))?
    {
        let id = device
            .id()
            .map_err(|e| backend("device identity", None, e))?;
        discovered.push((id, device));
    }
    let mut selected_outputs = Vec::with_capacity(plan.routes.len());
    for route in &plan.routes {
        if !wanted.load(Ordering::Acquire) || shared.epoch.load(Ordering::Acquire) != epoch {
            return Err(AudioError::Cancelled);
        }
        let id: cpal::DeviceId = route
            .output
            .0
            .parse()
            .map_err(|_| AudioError::InvalidOutputId(route.output.0.clone()))?;
        let (_, device) = discovered
            .iter()
            .find(|(candidate, _)| *candidate == id)
            .ok_or_else(|| AudioError::DeviceUnavailable(route.output.clone()))?;
        let name = device
            .description()
            .map_err(|e| backend("device description", Some(route.output.0.clone()), e))?
            .name()
            .to_owned();
        selected_outputs.push(output::select(
            device,
            OutputDevice {
                id: route.output.clone(),
                name,
            },
            session.sample_rate,
        )?);
    }
    let selected_inputs = capture::select(&session, &host)?;
    let mut configuration = selected_outputs[clock_index].configuration(
        bus,
        selected_inputs
            .iter()
            .map(|s| s.configuration.clone())
            .collect(),
    );
    configuration.clock_output_index = clock_index;
    configuration.outputs = selected_outputs
        .iter()
        .zip(&plan.routes)
        .map(|(selection, route)| selection.running_output(route.bus_id))
        .collect();
    let owner_bytes = selected_outputs
        .iter()
        .try_fold(0usize, |total, selection| {
            total
                .checked_add(selection.owner_bytes()?)
                .ok_or_else(|| AudioError::InvalidSession("output owner budget overflow".into()))
        })?;
    let follower_count = selected_outputs.len() - 1;
    let extra_bytes = output_render::extra_bytes(&configuration, follower_count)?;
    let output_bytes = owner_bytes
        .checked_add(extra_bytes)
        .ok_or_else(|| AudioError::InvalidSession("output memory overflow".into()))?;
    let mut render = RenderState::new_with_owner_budget(
        &session,
        bus,
        usize::from(selected_outputs[clock_index].channels()),
        epoch,
        wanted.clone(),
        shared.clone(),
        output_bytes,
        capture::selected_owner_bytes(&selected_inputs)?,
    )?;
    let mut followers =
        output_render::configure(&mut render, &plan, clock_index, gate.clone(), extra_bytes)?
            .into_iter();
    let permit = render.permit();
    let diagnostics: Vec<_> = selected_outputs
        .iter()
        .map(output::Selection::diagnostics)
        .collect();
    {
        let mut state = shared.state();
        state.running = false;
        state.configuration = Some(configuration.clone());
        state.telemetry = Some(render.telemetry.clone());
        state.authorization = Some((wanted.clone(), epoch));
        state.output_diagnostics = diagnostics.clone();
        state.error = None;
    }
    let mut input_streams = InputStreams {
        streams: Vec::with_capacity(selected_inputs.len()),
        wanted: wanted.clone(),
    };
    for (index, (selected, capture)) in selected_inputs.iter().zip(&render.captures).enumerate() {
        if !permit.authorized() {
            return Err(AudioError::Cancelled);
        }
        let state = CaptureState::new(
            capture.consumer.ring.clone(),
            index,
            usize::from(selected.config.channels()),
            epoch,
            wanted.clone(),
            shared.clone(),
            render.telemetry.clone(),
        );
        input_streams
            .streams
            .push(capture::make_stream(selected, state)?);
        if !permit.authorized() {
            return Err(AudioError::Cancelled);
        }
    }
    let telemetry = render.telemetry.clone();
    let controls = render.bank.clone();
    let mut primary = Some(render);
    let mut streams = Vec::with_capacity(selected_outputs.len());
    let mut permits = Vec::with_capacity(selected_outputs.len());
    for (index, (selected, diagnostic)) in
        selected_outputs.into_iter().zip(&diagnostics).enumerate()
    {
        if !permit.authorized() {
            return Err(AudioError::Cancelled);
        }
        let renderer = if index == clock_index {
            OutputRenderer::from(primary.take().expect("one clock output"))
        } else {
            OutputRenderer::Follower(followers.next().expect("one reader per follower"))
        };
        permits.push(renderer.permit());
        streams.push(selected.build(renderer, diagnostic)?);
    }
    output_startup::start_outputs(&streams, &permit, COMMAND_TIMEOUT, || {
        for (stream, input) in input_streams.streams.iter().zip(&configuration.inputs) {
            if !permit.authorized() {
                return Err(AudioError::Cancelled);
            }
            stream
                .play()
                .map_err(|e| backend("input activation", Some(input.device.name.clone()), e))?;
        }
        for (index, (route_permit, diagnostic)) in permits.iter().zip(&diagnostics).enumerate() {
            let diagnostic = diagnostic.clone();
            output_progress::supervise(
                route_permit.clone(),
                output_progress::deadline(configuration.sample_rate),
                output_progress::program_deadline(
                    configuration.sample_rate,
                    !configuration.inputs.is_empty(),
                    index != clock_index,
                ),
                move || diagnostic.progress_count(),
            )?;
        }
        Ok(())
    })?;
    if !permit.authorized() {
        return Err(AudioError::Cancelled);
    }
    guard.0 = None;
    Ok((
        Running {
            _streams: streams,
            _input_streams: input_streams,
            session,
            controls,
            wanted,
            telemetry,
            epoch,
        },
        configuration,
    ))
}
#[cfg(test)]
fn record_output_error(telemetry: &Telemetry, epoch: u64, kind: cpal::ErrorKind) {
    telemetry.fault.record(
        stream_fault(kind),
        output_error_origin(kind),
        FaultSource::OUTPUT,
        FaultMetadata::now(epoch),
    );
}

#[cfg(test)]
fn output_error_origin(kind: cpal::ErrorKind) -> FaultOrigin {
    if cfg!(target_os = "macos") && kind == cpal::ErrorKind::Xrun {
        FaultOrigin::CpalCoreAudioOutputOverload
    } else {
        FaultOrigin::CpalOutputError
    }
}

#[cfg(any(test, not(any(target_os = "macos", target_os = "windows"))))]
fn stream_fault(kind: cpal::ErrorKind) -> AudioFault {
    use cpal::ErrorKind;
    match kind {
        ErrorKind::DeviceNotAvailable => AudioFault::DeviceUnavailable,
        ErrorKind::DeviceChanged => AudioFault::DeviceChanged,
        ErrorKind::PermissionDenied => AudioFault::PermissionDenied,
        ErrorKind::StreamInvalidated => AudioFault::StreamInvalidated,
        ErrorKind::Xrun => AudioFault::Xrun,
        ErrorKind::RealtimeDenied => AudioFault::RealtimeDenied,
        ErrorKind::ResourceExhausted => AudioFault::ResourceExhausted,
        _ => AudioFault::Backend,
    }
}

struct RenderState {
    fanout: Option<FanoutRender>,
    output_permit: Option<OutputPermit>,
    output_index: usize,
    mixer: CompiledMixer,
    bank: Arc<ControlBank>,
    control_scratch: PreparedControls,
    applied_version: u64,
    bus: Uuid,
    #[cfg(test)]
    channels: usize,
    captures: Vec<CaptureRender>,
    priming_frames: u64,
    sample_rate: u32,
    left: Vec<f32>,
    right: Vec<f32>,
    epoch: u64,
    wanted: Arc<AtomicBool>,
    shared: Arc<Shared>,
    telemetry: Arc<Telemetry>,
}

/// Cloned only while constructing a native owner. A callback or listener uses
/// these atomics without borrowing the single-writer mixer or control mutex.
/// Program activation is a separate, owner-local gate.
#[derive(Clone)]
struct OutputPermit {
    route_index: usize,
    output_telemetry: Arc<OutputTelemetry>,
    epoch: u64,
    wanted: Arc<AtomicBool>,
    shared: Arc<Shared>,
    telemetry: Arc<Telemetry>,
}
impl OutputPermit {
    fn cancel(&self) {
        self.telemetry.gate.cancel();
    }
    fn program_authorized(&self) -> bool {
        self.authorized() && self.telemetry.gate.program()
    }
    fn submitted(&self, frames: usize, version: Option<u64>) {
        self.output_telemetry.submitted(frames, version);
    }
    fn authorized(&self) -> bool {
        self.wanted.load(Ordering::Acquire)
            && self.shared.epoch.load(Ordering::Acquire) == self.epoch
            && self.telemetry.fault.load(Ordering::Acquire) == 0
    }
    fn epoch(&self) -> u64 {
        self.epoch
    }
    fn metadata(&self) -> FaultMetadata {
        FaultMetadata::now(self.epoch)
    }
    fn fail_at(&self, fault: AudioFault, origin: FaultOrigin, metadata: FaultMetadata) {
        self.telemetry.record_fault(
            fault,
            origin,
            FaultSource::output(self.route_index),
            metadata,
        );
    }
}

impl RenderState {
    #[cfg(test)]
    fn new(
        session: &Session,
        bus: Uuid,
        channels: usize,
        epoch: u64,
        wanted: Arc<AtomicBool>,
        shared: Arc<Shared>,
    ) -> Result<Self, AudioError> {
        Self::new_with_output_budget(session, bus, channels, epoch, wanted, shared, 0)
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn new_with_output_budget(
        session: &Session,
        bus: Uuid,
        channels: usize,
        epoch: u64,
        wanted: Arc<AtomicBool>,
        shared: Arc<Shared>,
        output_bytes: usize,
    ) -> Result<Self, AudioError> {
        Self::new_with_owner_budget(
            session,
            bus,
            channels,
            epoch,
            wanted,
            shared,
            output_bytes,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_owner_budget(
        session: &Session,
        bus: Uuid,
        channels: usize,
        epoch: u64,
        wanted: Arc<AtomicBool>,
        shared: Arc<Shared>,
        output_bytes: usize,
        capture_owner_bytes: usize,
    ) -> Result<Self, AudioError> {
        if !(2..=usize::from(MAX_DEVICE_CHANNELS)).contains(&channels) {
            return Err(AudioError::InvalidSession(
                "output needs a supported stereo PCM layout".into(),
            ));
        }
        let control_records = session
            .inputs
            .len()
            .checked_add(session.sends.len())
            .and_then(|n| n.checked_add(session.buses.len()))
            .ok_or_else(|| AudioError::InvalidSession("control count overflow".into()))?;
        let control_bytes = ControlBank::estimated_bytes(control_records)
            .ok_or_else(|| AudioError::InvalidSession("control memory overflow".into()))?;
        let capture_ids = capture::source_groups(session);
        let capture_bytes = capture_ids
            .len()
            .checked_mul(capture::bytes_per_capture())
            .and_then(|n| {
                n.checked_add(
                    session
                        .inputs
                        .len()
                        .checked_mul(std::mem::size_of::<Uuid>())?,
                )
            })
            .and_then(|n| {
                n.checked_add(if capture_ids.is_empty() {
                    0
                } else {
                    capture_transport::Kernel::bytes()
                })
            })
            .ok_or_else(|| AudioError::InvalidSession("capture memory overflow".into()))?;
        let overhead = session
            .inputs
            .len()
            .checked_add(session.buses.len())
            .and_then(|n| n.checked_mul(std::mem::size_of::<AtomicMeter>()))
            .and_then(|n| n.checked_add(2 * RENDER_CHUNK * std::mem::size_of::<f32>()))
            .and_then(|n| n.checked_add(control_bytes))
            .and_then(|n| n.checked_add(capture_bytes))
            .and_then(|n| n.checked_add(output_bytes))
            .and_then(|n| n.checked_add(capture_owner_bytes))
            .and_then(|n| n.checked_add(std::mem::size_of::<Telemetry>()))
            .and_then(|n| GRAPH_BYTES.checked_sub(n))
            .ok_or_else(|| {
                AudioError::InvalidSession("the graph exceeds the runtime memory budget".into())
            })?;
        let mixer = CompiledMixer::compile(
            session,
            ResourceBudget {
                max_block_frames: RENDER_CHUNK,
                max_working_bytes: overhead,
            },
        )
        .map_err(|e| AudioError::InvalidSession(e.to_string()))?;
        let control_scratch = PreparedControls::compile(session, control_bytes)
            .map_err(|e| AudioError::InvalidSession(e.to_string()))?;
        let bank = Arc::new(ControlBank::new(&control_scratch));
        let kernel = (!capture_ids.is_empty()).then(capture_transport::Kernel::new);
        let mut telemetry = Telemetry::new(session);
        telemetry.gate = Arc::new(EpochGate::new(wanted.clone()));
        telemetry.outputs.push(Arc::new(OutputTelemetry::default()));
        let captures = capture_ids
            .into_iter()
            .map(|(device_id, source_ids)| {
                let capture = CaptureRender::new(
                    source_ids,
                    session.sample_rate,
                    kernel.as_ref().expect("capture kernel").clone(),
                );
                telemetry
                    .captures
                    .push((InputId(device_id), capture.consumer.ring.clone()));
                capture
            })
            .collect();
        let telemetry = Arc::new(telemetry);
        Ok(Self {
            fanout: None,
            output_permit: None,
            output_index: 0,
            mixer,
            bank,
            control_scratch,
            applied_version: 0,
            bus,
            #[cfg(test)]
            channels,
            captures,
            priming_frames: 0,
            sample_rate: session.sample_rate,
            left: vec![0.0; RENDER_CHUNK],
            right: vec![0.0; RENDER_CHUNK],
            epoch,
            wanted,
            shared,
            telemetry,
        })
    }
    fn fail(&self, fault: AudioFault, origin: FaultOrigin) {
        self.telemetry.record_fault(
            fault,
            origin,
            FaultSource::output(self.output_index),
            FaultMetadata::now(self.epoch),
        );
    }
    fn permit(&self) -> OutputPermit {
        OutputPermit {
            route_index: self.output_index,
            output_telemetry: self.telemetry.outputs[self.output_index].clone(),
            epoch: self.epoch,
            wanted: self.wanted.clone(),
            shared: self.shared.clone(),
            telemetry: self.telemetry.clone(),
        }
    }
    fn authorized(&self) -> bool {
        self.wanted.load(Ordering::Acquire)
            && self.shared.epoch.load(Ordering::Acquire) == self.epoch
            && self.telemetry.fault.load(Ordering::Acquire) == 0
    }
    #[cfg(test)]
    fn write<T: cpal::SizedSample + cpal::FromSample<f32>>(&mut self, output: &mut [T]) {
        let mut sink = InterleavedSink {
            samples: output,
            channels: self.channels,
        };
        let _ = self.write_to(&mut sink);
    }
    /// One whole callback transaction; native buffer acquisition/submission and
    /// their success/failure observables belong to the platform owner.
    fn write_to(&mut self, sink: &mut impl StereoSink) -> RenderOutcome {
        sink.clear();
        if !self.authorized() {
            return RenderOutcome::Silence;
        }
        let frames = match sink.frames() {
            Ok(frames) if frames <= MAX_CALLBACK_FRAMES => frames,
            _ => {
                self.fail(AudioFault::InvalidBuffer, FaultOrigin::OutputBuffer);
                return RenderOutcome::Silence;
            }
        };
        if let Some(version) = self
            .bank
            .copy_if_new(&mut self.control_scratch, self.applied_version)
        {
            if self.mixer.apply_controls(&self.control_scratch).is_err() {
                self.fail(AudioFault::Render, FaultOrigin::MixerControls);
                return RenderOutcome::Silence;
            }
            self.applied_version = version;
        }
        if frames == 0 {
            return RenderOutcome::Silence;
        }
        let telemetry = &self.telemetry;
        let epoch = self.epoch;
        let output_index = self.output_index;
        let fail = |fault, origin| {
            telemetry.record_fault(
                fault,
                origin,
                FaultSource::output(output_index),
                FaultMetadata::now(epoch),
            )
        };
        let mut transaction = if let Some(fanout) = &mut self.fanout {
            let transaction = match fanout.writer.begin(frames, self.applied_version) {
                Ok(transaction) => transaction,
                Err(error) => {
                    if error != fanout::Error::Cancelled {
                        let source = match error {
                            fanout::Error::RouteFull { route } => fanout.routes[route].0,
                            _ => output_index,
                        };
                        telemetry.record_fault(
                            AudioFault::StreamInvalidated,
                            FaultOrigin::OutputFanoutProducer,
                            FaultSource::output(source),
                            FaultMetadata::now(epoch),
                        );
                    }
                    return RenderOutcome::Silence;
                }
            };
            Some((transaction, &fanout.routes))
        } else {
            None
        };
        let mut ready = true;
        let mut first_unready = None;
        for (index, capture) in self.captures.iter_mut().enumerate() {
            match capture.consumer.prepare(frames) {
                Ok(primed) => {
                    ready &= primed;
                    if !primed && first_unready.is_none() {
                        first_unready = Some(index);
                    }
                }
                Err(error) => {
                    self.telemetry.record_fault(
                        capture::fault(error),
                        FaultOrigin::CaptureTransportConsumer,
                        FaultSource::input(index),
                        FaultMetadata::now(self.epoch),
                    );
                    return RenderOutcome::Silence;
                }
            }
        }
        if !ready {
            self.priming_frames += frames as u64;
            if self.priming_frames > output_progress::capture_priming_frames(self.sample_rate) {
                self.telemetry.record_fault(
                    AudioFault::CaptureStartup,
                    FaultOrigin::CaptureStartup,
                    first_unready.map_or(FaultSource::RUNTIME, FaultSource::input),
                    FaultMetadata::now(self.epoch),
                );
            }
            return RenderOutcome::Silence;
        }
        for start in (0..frames).step_by(RENDER_CHUNK) {
            let count = (frames - start).min(RENDER_CHUNK);
            let mut destinations = [StereoOutput {
                id: self.bus,
                left: &mut self.left[..count],
                right: &mut self.right[..count],
            }];
            for capture in &mut self.captures {
                capture
                    .consumer
                    .read(&mut capture.left[..count], &mut capture.right[..count]);
            }
            let sources = self.captures.iter().flat_map(|capture| {
                capture.source_ids.iter().map(move |&id| StereoInput {
                    id,
                    left: &capture.left[..count],
                    right: &capture.right[..count],
                })
            });
            if self
                .mixer
                .render_sources(sources, &mut destinations, count)
                .is_err()
            {
                fail(AudioFault::Render, FaultOrigin::MixerRender);
                sink.clear();
                return RenderOutcome::Silence;
            }
            if sink
                .write_stereo(start, &self.left[..count], &self.right[..count])
                .is_err()
            {
                fail(AudioFault::InvalidBuffer, FaultOrigin::OutputBuffer);
                sink.clear();
                return RenderOutcome::Silence;
            }
            if let Some((transaction, routes)) = &mut transaction {
                for (queue, (_, bus)) in routes.iter().enumerate() {
                    let pcm = match self.mixer.rendered_bus(*bus) {
                        Ok(pcm) => pcm,
                        Err(_) => {
                            fail(AudioFault::Render, FaultOrigin::MixerRender);
                            sink.clear();
                            return RenderOutcome::Silence;
                        }
                    };
                    if transaction.write(queue, pcm).is_err() {
                        fail(
                            AudioFault::StreamInvalidated,
                            FaultOrigin::OutputFanoutProducer,
                        );
                        sink.clear();
                        return RenderOutcome::Silence;
                    }
                }
            }
            for (target, meter) in self.telemetry.inputs.iter().zip(self.mixer.input_meters()) {
                target.publish(meter);
            }
            for (target, meter) in self.telemetry.buses.iter().zip(self.mixer.bus_meters()) {
                target.publish(meter);
            }
        }
        // Revoke output even if a stop/fault arrived while this callback rendered.
        if !self.wanted.load(Ordering::Acquire)
            || self.shared.epoch.load(Ordering::Acquire) != self.epoch
            || self.telemetry.fault.load(Ordering::Acquire) != 0
        {
            self.telemetry.gate.cancel();
            sink.clear();
            return RenderOutcome::Silence;
        }
        if let Some((transaction, _)) = transaction {
            if transaction.commit().is_err() {
                sink.clear();
                return RenderOutcome::Silence;
            }
        }
        self.telemetry
            .frames
            .fetch_add(frames as u64, Ordering::Relaxed);
        self.telemetry.callbacks.fetch_add(1, Ordering::Relaxed);
        // Preparing controls alone does not render them. Priming, rejected or
        // cancelled callbacks and empty buffers must not advance this watermark.
        if frames != 0 {
            self.telemetry
                .applied_control_version
                .store(self.applied_version, Ordering::Release);
            RenderOutcome::Rendered {
                frames,
                accepted_version: Some(self.applied_version),
            }
        } else {
            RenderOutcome::Silence
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod capture_tests;
#[cfg(test)]
mod native_packet_tests;
#[cfg(test)]
mod output_sink_tests;
