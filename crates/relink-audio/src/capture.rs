use super::capture_groups::CaptureGroupPlan;
use super::capture_transport::{Consumer, Kernel, Producer, Ring, TransportError};
#[cfg(any(not(target_os = "macos"), test))]
use super::supported_pcm;
use super::{
    backend, native_host, AudioError, AudioFault, FaultMetadata, FaultOrigin, FaultSource,
    InputDevice, InputId, RunningInput, Shared, Telemetry, RENDER_CHUNK,
};
use cpal::traits::{DeviceTrait, HostTrait};
use relink_core::{ApplicationSelector, InputKind, Session, Uuid};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub(crate) fn enumerate() -> Result<Vec<InputDevice>, AudioError> {
    let host = native_host()?;
    let mut inputs = Vec::new();
    for device in host
        .input_devices()
        .map_err(|e| backend("input discovery", None, e))?
    {
        let id = device
            .id()
            .map_err(|e| backend("input identity", None, e))?
            .to_string();
        let name = device
            .description()
            .map_err(|e| backend("input description", Some(id.clone()), e))?
            .name()
            .to_owned();
        inputs.push(InputDevice {
            id: InputId(id),
            name,
        });
    }
    inputs.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.0.cmp(&b.id.0)));
    Ok(inputs)
}

pub(crate) fn source_groups(session: &Session) -> BTreeMap<String, Vec<Uuid>> {
    let mut groups: BTreeMap<String, Vec<Uuid>> = BTreeMap::new();
    for input in &session.inputs {
        let id = match &input.kind {
            InputKind::Hardware { device_id } => device_id,
            InputKind::Virtual { endpoint_id } => endpoint_id,
            _ => continue,
        };
        groups.entry(id.clone()).or_default().push(input.id);
    }
    groups
}
pub(crate) struct Selected {
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    device: cpal::Device,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    native: NativeSelection,
    pub(crate) config: cpal::SupportedStreamConfig,
    pub(crate) configuration: RunningInput,
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
enum NativeSelection {
    #[cfg(target_os = "macos")]
    Device(super::native_capture::Selection),
    #[cfg(target_os = "windows")]
    Device,
    Application(super::application::Selection),
}

pub(crate) struct CapturePlan {
    pub(crate) sources: Vec<Selected>,
    pub(crate) groups: Vec<CaptureGroupPlan>,
}

// Requested native owner stack size, shared by both implementations. The OS
// may add guard pages and platform-private audio allocations outside this budget.
pub(crate) const OWNER_STACK_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const OWNER_CHANNEL_BYTES: usize = 3 * 4096;
pub(crate) fn checked_owner_bytes(
    parts: impl IntoIterator<Item = Option<usize>>,
) -> Result<usize, AudioError> {
    parts
        .into_iter()
        .try_fold(0_usize, |sum, part| sum.checked_add(part?))
        .ok_or_else(|| AudioError::InvalidSession("capture owner memory estimate overflow".into()))
}
impl Selected {
    pub(crate) fn owner_bytes(&self) -> Result<usize, AudioError> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let native = match &self.native {
            #[cfg(target_os = "macos")]
            NativeSelection::Device(selection) => selection.owner_bytes()?,
            #[cfg(target_os = "windows")]
            NativeSelection::Device => super::wasapi_capture::owner_bytes(&self.configuration)?,
            NativeSelection::Application(selection) => selection.owner_bytes()?,
        };
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let native = 0;
        let metadata = checked_owner_bytes([
            Some(self.configuration.device.id.0.capacity()),
            Some(self.configuration.device.name.capacity()),
            Some(self.configuration.sample_format.capacity()),
            self.configuration
                .source_ids
                .capacity()
                .checked_mul(std::mem::size_of::<Uuid>()),
        ])?;
        // Selection metadata plus the two configuration copies held by startup
        // and ControlState. These are separate from CaptureRender's alias list.
        checked_owner_bytes([
            Some(native),
            Some(std::mem::size_of::<Self>()),
            Some(2 * std::mem::size_of::<RunningInput>()),
            metadata.checked_mul(3),
        ])
    }
}
pub(crate) fn selected_owner_bytes(selected: &[Selected]) -> Result<usize, AudioError> {
    selected.iter().try_fold(0_usize, |sum, item| {
        checked_owner_bytes([Some(sum), Some(item.owner_bytes()?)])
    })
}
pub(crate) fn select(session: &Session, host: &cpal::Host) -> Result<CapturePlan, AudioError> {
    let mut selected = Vec::new();
    let mut groups = Vec::new();
    for (id, source_ids) in source_groups(session) {
        let native_id: cpal::DeviceId = id
            .parse()
            .map_err(|_| AudioError::InvalidInputId(id.clone()))?;
        let mut matching = None;
        for device in host
            .devices()
            .map_err(|e| backend("input discovery", None, e))?
        {
            if device
                .id()
                .map_err(|e| backend("input identity", None, e))?
                == native_id
            {
                matching = Some(device);
                break;
            }
        }
        let device = matching.ok_or_else(|| AudioError::InputUnavailable(InputId(id.clone())))?;
        let name = device
            .description()
            .map_err(|e| backend("input description", Some(id.clone()), e))?
            .name()
            .to_owned();
        #[cfg(target_os = "macos")]
        let native = super::native_capture::select(&id, session.sample_rate)?;
        #[cfg(target_os = "macos")]
        let config = native.configuration();
        #[cfg(not(target_os = "macos"))]
        let config = select_config(
            device
                .supported_input_configs()
                .map_err(|e| backend("input format discovery", Some(name.clone()), e))?,
            session.sample_rate,
        )
        .ok_or_else(|| AudioError::UnsupportedInputConfiguration {
            device: name.clone(),
            sample_rate: session.sample_rate,
        })?;
        let configuration = RunningInput {
            device: InputDevice {
                id: InputId(id),
                name,
            },
            source_ids,
            device_channels: config.channels(),
            sample_format: {
                #[cfg(target_os = "macos")]
                {
                    native.description()
                }
                #[cfg(not(target_os = "macos"))]
                {
                    config.sample_format().to_string()
                }
            },
        };
        groups.push(CaptureGroupPlan {
            source_indices: vec![selected.len()],
            input_ids: configuration.source_ids.clone(),
        });
        selected.push(Selected {
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            device,
            #[cfg(target_os = "macos")]
            native: NativeSelection::Device(native),
            #[cfg(target_os = "windows")]
            native: NativeSelection::Device,
            config,
            configuration,
        });
    }
    let mut applications: BTreeMap<String, Vec<Uuid>> = BTreeMap::new();
    for input in &session.inputs {
        if let InputKind::Application { application_id } = &input.kind {
            // Canonical serialization shares one native source group even if
            // equivalent saved selectors originally used different JSON spacing.
            let selector = ApplicationSelector::decode(application_id)
                .and_then(|selector| selector.encode())
                .map_err(|error| AudioError::InvalidSession(error.to_string()))?;
            applications.entry(selector).or_default().push(input.id);
        }
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let mut claimed = std::collections::BTreeSet::new();
        for (id, input_ids) in applications {
            let selections = super::application::select(&id, session.sample_rate)?;
            if selections.is_empty() {
                return Err(backend(
                    "application selection",
                    Some(id),
                    "the selected application has no capture sources",
                ));
            }
            let mut source_indices = Vec::with_capacity(selections.len());
            for native in selections {
                for instance in native.scope() {
                    if !claimed.insert(*instance) {
                        return Err(backend("application selection", Some(id),
                            "selected applications have overlapping process scopes; choose one of these sources to prevent duplicate audio"));
                    }
                }
                let config = native.configuration();
                let configuration = RunningInput {
                    device: InputDevice {
                        id: InputId(native.key()),
                        name: native.name(),
                    },
                    source_ids: input_ids.clone(),
                    device_channels: config.channels(),
                    sample_format: native.description(),
                };
                source_indices.push(selected.len());
                selected.push(Selected {
                    native: NativeSelection::Application(native),
                    config,
                    configuration,
                });
            }
            groups.push(CaptureGroupPlan {
                source_indices,
                input_ids,
            });
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    if !applications.is_empty() {
        return Err(AudioError::UnsupportedPlatform);
    }
    Ok(CapturePlan {
        sources: selected,
        groups,
    })
}
#[cfg(any(not(target_os = "macos"), test))]
pub(crate) fn select_config(
    configs: impl Iterator<Item = cpal::SupportedStreamConfigRange>,
    sample_rate: u32,
) -> Option<cpal::SupportedStreamConfig> {
    configs
        .filter(|c| {
            (1..=super::MAX_DEVICE_CHANNELS).contains(&c.channels())
                && supported_pcm(c.sample_format())
                && native_encoding_supported(c.sample_format())
        })
        .filter_map(|c| c.try_with_sample_rate(sample_rate))
        .min_by_key(|c| {
            (
                match c.channels() {
                    2 => 0,
                    1 => 1,
                    _ => 2,
                },
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
fn native_encoding_supported(format: cpal::SampleFormat) -> bool {
    #[cfg(target_os = "windows")]
    {
        super::wasapi_packet::Encoding::from_cpal(format).is_some()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = format;
        true
    }
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) enum CaptureStream {
    #[cfg(target_os = "windows")]
    Device(super::wasapi_capture::WasapiCapture),
    #[cfg(target_os = "macos")]
    Device(super::native_capture::NativeCapture),
    Application(super::application::ApplicationCapture),
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
impl CaptureStream {
    pub(crate) fn play(&self) -> Result<(), AudioError> {
        match self {
            Self::Device(stream) => stream.play(),
            Self::Application(stream) => stream.play(),
        }
    }
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) type CaptureStream = cpal::Stream;

pub(crate) struct CaptureRender {
    pub(crate) consumer: Consumer,
    pub(crate) left: Vec<f32>,
    pub(crate) right: Vec<f32>,
}
impl CaptureRender {
    pub(crate) fn new(sample_rate: u32, kernel: Arc<Kernel>) -> Self {
        Self {
            consumer: Consumer::new(Ring::new(), kernel, sample_rate),
            left: vec![0.0; RENDER_CHUNK],
            right: vec![0.0; RENDER_CHUNK],
        }
    }
}
pub(crate) fn bytes_per_capture() -> usize {
    Ring::bytes()
        + 2 * RENDER_CHUNK * std::mem::size_of::<f32>()
        + std::mem::size_of::<CaptureRender>()
}

pub(crate) struct CaptureState {
    producer: Producer,
    #[cfg(any(not(any(target_os = "windows", target_os = "macos")), test))]
    channels: usize,
    permit: CapturePermit,
}
#[derive(Clone)]
pub(crate) struct CapturePermit {
    source: FaultSource,
    epoch: u64,
    wanted: Arc<AtomicBool>,
    shared: Arc<Shared>,
    telemetry: Arc<Telemetry>,
}
impl CapturePermit {
    pub(crate) fn authorized(&self) -> bool {
        self.wanted.load(Ordering::Acquire)
            && self.shared.epoch.load(Ordering::Acquire) == self.epoch
            && self.telemetry.fault.load(Ordering::Acquire) == 0
    }
    pub(crate) fn fail_at(&self, fault: AudioFault, origin: FaultOrigin, metadata: FaultMetadata) {
        self.telemetry
            .record_fault(fault, origin, self.source, metadata);
    }
    pub(crate) fn metadata(&self) -> FaultMetadata {
        FaultMetadata::now(self.epoch)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }
}
impl CaptureState {
    pub(crate) fn new(
        ring: Arc<Ring>,
        source_index: usize,
        channels: usize,
        epoch: u64,
        wanted: Arc<AtomicBool>,
        shared: Arc<Shared>,
        telemetry: Arc<Telemetry>,
    ) -> Self {
        #[cfg(all(any(target_os = "windows", target_os = "macos"), not(test)))]
        let _ = channels;
        Self {
            producer: Producer::new(ring),
            #[cfg(any(not(any(target_os = "windows", target_os = "macos")), test))]
            channels,
            permit: CapturePermit {
                source: FaultSource::input(source_index),
                epoch,
                wanted,
                shared,
                telemetry,
            },
        }
    }
    pub(crate) fn authorized(&self) -> bool {
        self.permit.authorized()
    }
    #[cfg(target_os = "windows")]
    pub(crate) fn event_close(&self) -> Arc<super::wasapi_event::CloseRecord> {
        self.producer.ring.event_close.clone()
    }
    #[cfg(not(target_os = "macos"))]
    pub(crate) fn fail(&self, fault: AudioFault) {
        self.permit
            .fail_at(fault, FaultOrigin::WasapiInput, self.permit.metadata());
    }
    #[cfg(target_os = "windows")]
    pub(crate) fn fail_status(&self, fault: AudioFault, status: i32) {
        self.permit.fail_at(
            fault,
            FaultOrigin::WasapiInput,
            FaultMetadata {
                status: Some(status),
                ..self.permit.metadata()
            },
        );
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(crate) fn permit(&self) -> CapturePermit {
        self.permit.clone()
    }
    #[cfg(target_os = "macos")]
    pub(crate) fn ring(&self) -> Arc<Ring> {
        self.producer.ring.clone()
    }
    #[cfg(any(not(any(target_os = "windows", target_os = "macos")), test))]
    pub(crate) fn push<T: cpal::Sample>(&mut self, samples: &[T])
    where
        f32: cpal::FromSample<T>,
    {
        if !self.authorized() {
            return;
        }
        if let Err(error) = self.producer.push(samples, self.channels) {
            self.permit.fail_at(
                fault(error),
                FaultOrigin::CaptureTransportProducer,
                self.permit.metadata(),
            );
        }
    }
    #[cfg(any(target_os = "windows", test))]
    pub(crate) fn push_frames(&mut self, frames: usize, read: impl FnMut(usize) -> [f32; 2]) {
        if !self.authorized() {
            return;
        }
        if let Err(error) = self.producer.push_frames(frames, read) {
            self.permit.fail_at(
                fault(error),
                FaultOrigin::CaptureTransportProducer,
                self.permit.metadata(),
            );
        }
    }
    #[cfg(target_os = "macos")]
    pub(crate) fn push_native_frames(
        &mut self,
        frames: usize,
        read: impl FnMut(usize) -> [f32; 2],
    ) -> Result<bool, AudioFault> {
        if !self.authorized() {
            return Ok(false);
        }
        self.producer.push_frames(frames, read).map_err(fault)?;
        Ok(true)
    }
}
pub(crate) fn fault(error: TransportError) -> AudioFault {
    match error {
        TransportError::Underflow => AudioFault::CaptureUnderflow,
        TransportError::Overflow => AudioFault::CaptureOverflow,
        TransportError::InvalidBuffer => AudioFault::InvalidBuffer,
    }
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn make_typed<T: cpal::SizedSample>(
    selected: &Selected,
    mut state: CaptureState,
) -> Result<cpal::Stream, cpal::Error>
where
    f32: cpal::FromSample<T>,
{
    let error_permit = state.permit.clone();
    selected.device.build_input_stream_raw(
        selected.config.config(),
        T::FORMAT,
        move |data, _| {
            if let Some(samples) = data.as_slice::<T>() {
                state.push(samples);
            } else {
                state.fail(AudioFault::InvalidBuffer);
            }
        },
        move |error| {
            error_permit.fail_at(
                super::stream_fault(error.kind()),
                FaultOrigin::CpalInputError,
                error_permit.metadata(),
            );
        },
        Some(super::COMMAND_TIMEOUT),
    )
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) fn make_stream(
    selected: &Selected,
    state: CaptureState,
) -> Result<cpal::Stream, AudioError> {
    macro_rules! build {
        ($t:ty) => {
            make_typed::<$t>(selected, state)
        };
    }
    let stream = match selected.config.sample_format() {
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::F64 => build!(f64),
        cpal::SampleFormat::I8 => build!(i8),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::I24 => build!(cpal::I24),
        cpal::SampleFormat::I32 => build!(i32),
        cpal::SampleFormat::I64 => build!(i64),
        cpal::SampleFormat::U8 => build!(u8),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::U24 => build!(cpal::U24),
        cpal::SampleFormat::U32 => build!(u32),
        cpal::SampleFormat::U64 => build!(u64),
        _ => {
            return Err(AudioError::UnsupportedInputConfiguration {
                device: selected.configuration.device.name.clone(),
                sample_rate: selected.config.sample_rate(),
            })
        }
    };
    stream.map_err(|e| {
        backend(
            "input stream creation",
            Some(selected.configuration.device.name.clone()),
            e,
        )
    })
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn make_stream(
    selected: &Selected,
    state: CaptureState,
) -> Result<CaptureStream, AudioError> {
    match &selected.native {
        #[cfg(target_os = "windows")]
        NativeSelection::Device => super::wasapi_capture::WasapiCapture::build(
            &selected.configuration,
            &selected.config,
            state,
        )
        .map(CaptureStream::Device),
        #[cfg(target_os = "macos")]
        NativeSelection::Device(native) => {
            super::native_capture::NativeCapture::build(native, state).map(CaptureStream::Device)
        }
        NativeSelection::Application(native) => {
            super::application::ApplicationCapture::build(native, state)
                .map(CaptureStream::Application)
        }
    }
}
