//! Original event-driven WASAPI capture adapter. COM objects and native packet
//! leases stay on one dedicated owner thread. No CPAL capture callback is used.
use super::{
    backend,
    capture::CaptureState,
    wasapi_event::{prepare_resources, CloseRecord, NativeResources, OwnedEvent},
    wasapi_hardware_owner::{self, Record, Registry, Reserved, StopRequirement},
    wasapi_packet::{Encoding, Layout, Metadata, ReleaseGuard, Tracker, SILENT},
    AudioError, AudioFault, RunningInput, COMMAND_TIMEOUT, MAX_CALLBACK_FRAMES,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};
use windows::{
    core::{w, Interface, PCWSTR},
    Win32::{
        Foundation::{
            CloseHandle, E_ACCESSDENIED, E_OUTOFMEMORY, HANDLE, S_OK, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Media::{
            Audio::*,
            KernelStreaming::{
                KSDATAFORMAT_SUBTYPE_PCM, SPEAKER_FRONT_CENTER, SPEAKER_FRONT_LEFT,
                SPEAKER_FRONT_RIGHT, WAVE_FORMAT_EXTENSIBLE,
            },
            Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        },
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
                COINIT_MULTITHREADED,
            },
            Threading::{
                AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
                ResetEvent, WaitForSingleObject,
            },
        },
    },
};

pub(crate) fn owner_bytes(configuration: &RunningInput) -> Result<usize, AudioError> {
    use super::capture::{checked_owner_bytes, OWNER_CHANNEL_BYTES, OWNER_STACK_BYTES};
    use std::mem::size_of;
    // Ring, ASRC and render scratch are charged separately by RenderState.
    // This covers application-owned worker storage; COM/driver-private memory
    // is opaque. Retained session-wide telemetry is not charged by this worker
    // estimate, so this is not a total retained-memory bound.
    let id_bytes = configuration.device.id.0.len();
    checked_owner_bytes([
        Some(OWNER_STACK_BYTES),
        Some(OWNER_CHANNEL_BYTES),
        Some(size_of::<WasapiCapture>()),
        Some(size_of::<NativeCapture>()),
        Some(size_of::<Owner>()),
        Some(size_of::<Reservation>()),
        Some(size_of::<Reserved>()),
        Some(Record::bytes()),
        Some(CloseRecord::bytes()),
        Some(size_of::<CaptureState>()),
        Some(size_of::<Tracker>()),
        Some(size_of::<Layout>()),
        Some(size_of::<AtomicBool>() + size_of::<[usize; 2]>()),
        Some(configuration.device.name.len()),
        // Parsed and owned ID strings plus the NUL-terminated UTF-16 endpoint.
        id_bytes.checked_mul(4),
        id_bytes
            .checked_add(1)
            .and_then(|n| n.checked_mul(size_of::<u16>())),
        Some(size_of::<WAVEFORMATEXTENSIBLE>()),
    ])
}

pub(crate) struct WasapiCapture {
    commands: mpsc::SyncSender<mpsc::SyncSender<Result<(), AudioError>>>,
    keep_running: Arc<AtomicBool>,
}
impl WasapiCapture {
    pub(crate) fn build(
        configuration: &RunningInput,
        config: &cpal::SupportedStreamConfig,
        state: CaptureState,
    ) -> Result<Self, AudioError> {
        let parsed: cpal::DeviceId = configuration
            .device
            .id
            .0
            .parse()
            .map_err(|_| AudioError::InvalidInputId(configuration.device.id.0.clone()))?;
        if parsed.host() != cpal::HostId::Wasapi
            || parsed.id().is_empty()
            || parsed.id().contains('\0')
        {
            return Err(AudioError::InvalidInputId(
                configuration.device.id.0.clone(),
            ));
        }
        let encoding = Encoding::from_cpal(config.sample_format()).ok_or_else(|| {
            AudioError::UnsupportedInputConfiguration {
                device: configuration.device.name.clone(),
                sample_rate: config.sample_rate(),
            }
        })?;
        let layout = Layout {
            channels: config.channels(),
            encoding,
        };
        let native_id = parsed.id().to_owned();
        let record = Arc::new(Record::new(state.event_close()));
        let reservation =
            Reservation::acquire(&native_id, owner_bytes(configuration)?, record.clone())?;
        let name = configuration.device.name.clone();
        let sample_rate = config.sample_rate();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (commands, receiver) = mpsc::sync_channel(1);
        let keep_running = Arc::new(AtomicBool::new(true));
        let active = keep_running.clone();
        thread::Builder::new()
            .name("relink-wasapi-capture".into())
            .stack_size(super::capture::OWNER_STACK_BYTES)
            .spawn(move || {
                owner_loop(
                    native_id,
                    name,
                    sample_rate,
                    layout,
                    state,
                    active,
                    receiver,
                    ready_tx,
                    reservation,
                    record,
                );
            })
            .map_err(|e| backend("WASAPI capture thread creation", None, e))?;
        let stream = Self {
            commands,
            keep_running,
        };
        match ready_rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(Ok(())) => Ok(stream),
            Ok(Err(error)) => Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(AudioError::Timeout {
                operation: "input stream creation",
            }),
            Err(_) => Err(AudioError::RuntimeUnavailable),
        }
    }
    pub(crate) fn play(&self) -> Result<(), AudioError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.commands
            .try_send(tx)
            .map_err(|_| AudioError::RuntimeUnavailable)?;
        match rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.keep_running.store(false, Ordering::Release);
                Err(AudioError::Timeout {
                    operation: "input activation",
                })
            }
            Err(_) => Err(AudioError::RuntimeUnavailable),
        }
    }
}
impl Drop for WasapiCapture {
    fn drop(&mut self) {
        self.keep_running.store(false, Ordering::Release);
        // Never join a worker stuck inside a driver/COM call. Session gates have
        // already revoked publication; normal event waits observe this in 20ms.
    }
}

type StartReply = mpsc::SyncSender<Result<(), AudioError>>;
static HARDWARE_REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());
struct Reservation {
    index: usize,
}
impl Reservation {
    fn acquire(id: &str, bytes: usize, record: Arc<Record>) -> Result<Self, AudioError> {
        let index = HARDWARE_REGISTRY
            .lock()
            .map_err(|_| {
                backend(
                    "WASAPI hardware owner reservation",
                    None,
                    "owner registry is unavailable",
                )
            })?
            .acquire(id, bytes, record)?;
        Ok(Self { index })
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if let Ok(mut registry) = HARDWARE_REGISTRY.lock() {
            registry.release(self.index);
        }
    }
}
struct Owner {
    native: Option<NativeCapture>,
    state: CaptureState,
    active: Arc<AtomicBool>,
    record: Arc<Record>,
    // Drops after native cleanup and fault reporting, including constructor errors.
    _reservation: Reservation,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        if let Err(status) =
            wasapi_hardware_owner::release(&mut self.native, &self.record, |native| {
                native.stop_status().map_err(|error| error.code().0)
            })
        {
            self.state
                .fail_status(hresult_fault(windows::core::HRESULT(status)), status);
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn owner_loop(
    id: String,
    name: String,
    sample_rate: u32,
    layout: Layout,
    state: CaptureState,
    active: Arc<AtomicBool>,
    commands: mpsc::Receiver<StartReply>,
    ready: StartReply,
    reservation: Reservation,
    record: Arc<Record>,
) {
    let mut owner = Owner {
        native: None,
        state,
        active: active.clone(),
        record,
        _reservation: reservation,
    };
    if !active.load(Ordering::Acquire) || !owner.state.authorized() {
        let _ = ready.send(Err(AudioError::Cancelled));
        return;
    }
    let native = match NativeCapture::open(&id, sample_rate, layout, owner.record.close.clone()) {
        Ok(native) => native,
        Err(error) => {
            let _ = ready.send(Err(backend("WASAPI input creation", Some(name), error)));
            return;
        }
    };
    owner.native = Some(native);
    if !active.load(Ordering::Acquire) || !owner.state.authorized() {
        let _ = ready.send(Err(AudioError::Cancelled));
        return;
    }
    if ready.send(Ok(())).is_err() {
        return;
    }
    loop {
        if !active.load(Ordering::Acquire) || !owner.state.authorized() {
            return;
        }
        match commands.recv_timeout(Duration::from_millis(20)) {
            Ok(reply) => {
                if !active.load(Ordering::Acquire) || !owner.state.authorized() {
                    let _ = reply.send(Err(AudioError::Cancelled));
                    return;
                }
                if let Err(error) = owner.native.as_mut().unwrap().start() {
                    owner
                        .state
                        .fail_status(hresult_fault(error.code()), error.code().0);
                    let _ = reply.send(Err(backend("WASAPI input activation", Some(name), error)));
                    return;
                }
                if !active.load(Ordering::Acquire) || !owner.state.authorized() {
                    let _ = reply.send(Err(AudioError::Cancelled));
                    return;
                }
                if reply.send(Ok(())).is_err() {
                    return;
                }
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return,
        }
    }
    let mut tracker = Tracker::default();
    while active.load(Ordering::Acquire) && owner.state.authorized() {
        // Only this dedicated capture thread waits. The engine's output callback
        // reads the atomic ring and never waits for a native device or this event.
        let native = owner.native.as_mut().unwrap();
        let waited = unsafe { WaitForSingleObject(*native.resources.event.handle(), 20) };
        // A timeout still probes the packet count so invalidation can be
        // reported even when a disconnected endpoint stops signaling events.
        if waited != WAIT_OBJECT_0 && waited != WAIT_TIMEOUT {
            owner.state.fail(AudioFault::Backend);
            return;
        }
        if let Err(fault) = native.drain(&mut owner.state, &mut tracker, layout, &active) {
            owner.state.fail(fault);
            return;
        }
    }
}

pub(crate) struct Apartment;
impl Apartment {
    pub(crate) fn open() -> windows::core::Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
type Event = OwnedEvent<HANDLE>;
fn close_event(handle: HANDLE) -> Result<(), i32> {
    unsafe { CloseHandle(handle) }.map_err(|error| error.code().0)
}
struct Realtime(HANDLE);
impl Drop for Realtime {
    fn drop(&mut self) {
        let _ = unsafe { AvRevertMmThreadCharacteristics(self.0) };
    }
}
pub(crate) struct NativeCapture {
    // Release capture before client and both COM objects before the event and
    // COM apartment. The client may still reference its event until destruction.
    resources: NativeResources<IAudioCaptureClient, IAudioClient, Event, Realtime, Apartment>,
    // Start failure can leave native state uncertain; Stop is owed after any
    // Start call that was entered, until a successful Stop return.
    stop_required: StopRequirement,
    maximum_frames: u32,
}
impl NativeCapture {
    fn open(
        id: &str,
        sample_rate: u32,
        layout: Layout,
        close: Arc<CloseRecord>,
    ) -> windows::core::Result<Self> {
        let apartment = Apartment::open()?;
        let endpoint: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
        // SAFETY: the worker owns its COM apartment and all interfaces. The ID
        // is explicitly selected, NUL-terminated, and lives through GetDevice.
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        let device = unsafe { enumerator.GetDevice(PCWSTR(endpoint.as_ptr()))? };
        let endpoint_kind: IMMEndpoint = device.cast()?;
        if unsafe { endpoint_kind.GetDataFlow()? } != eCapture {
            return Err(windows::core::Error::from_hresult(
                AUDCLNT_E_WRONG_ENDPOINT_TYPE,
            ));
        }
        if unsafe { device.GetState()? } != DEVICE_STATE_ACTIVE {
            return Err(windows::core::Error::from_hresult(
                AUDCLNT_E_DEVICE_INVALIDATED,
            ));
        }
        // Admission uses the exact native ID returned by discovery. Reject
        // alternate strings resolving to this endpoint before creating an event.
        let actual = unsafe { device.GetId()? };
        let matches = if actual.is_null() {
            false
        } else {
            // GetId returns a NUL-terminated allocation. Comparing both whole
            // slices avoids reading past a shorter ID while rejecting aliases.
            (unsafe { actual.as_wide() }) == &endpoint[..endpoint.len() - 1]
        };
        unsafe { CoTaskMemFree(Some(actual.0.cast())) };
        if !matches {
            return Err(windows::core::Error::from_hresult(
                AUDCLNT_E_DEVICE_INVALIDATED,
            ));
        }
        // Locals drop in reverse declaration order on every early return. Keep
        // this event alive until the client is released even if buffer/service
        // discovery or MMCSS registration fails after SetEventHandle succeeds.
        let event = Event::new(
            unsafe { CreateEventW(None, false, false, None)? },
            close_event,
            close,
        );
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None)? };
        let format = wave_format(sample_rate, layout)?;
        let mut closest = std::ptr::null_mut();
        let supported = unsafe {
            client.IsFormatSupported(AUDCLNT_SHAREMODE_SHARED, &format.Format, Some(&mut closest))
        };
        if !closest.is_null() {
            unsafe {
                CoTaskMemFree(Some(closest.cast()));
            }
        }
        if supported != S_OK {
            return Err(windows::core::Error::from_hresult(if supported.is_err() {
                supported
            } else {
                AUDCLNT_E_UNSUPPORTED_FORMAT
            }));
        }
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                0,
                0,
                &format.Format,
                None,
            )?;
            client.SetEventHandle(*event.handle())?;
        }
        let maximum_frames = unsafe { client.GetBufferSize()? };
        if maximum_frames == 0 || maximum_frames as usize > MAX_CALLBACK_FRAMES {
            return Err(windows::core::Error::from_hresult(
                AUDCLNT_E_BUFFER_SIZE_ERROR,
            ));
        }
        let capture: IAudioCaptureClient = unsafe { client.GetService()? };
        let mut task_index = 0;
        let realtime =
            Realtime(unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index)? });
        Ok(Self {
            resources: NativeResources {
                capture,
                client,
                event,
                realtime,
                apartment,
            },
            stop_required: StopRequirement::default(),
            maximum_frames,
        })
    }
    /// Process loopback is not an endpoint and may not implement format discovery.
    /// The caller has activated only the explicit INCLUDE_TARGET_PROCESS_TREE.
    pub(crate) fn from_process_client(
        client: IAudioClient,
        apartment: Apartment,
        sample_rate: u32,
        layout: Layout,
        close: Arc<CloseRecord>,
    ) -> windows::core::Result<Self> {
        let (resources, maximum_frames) = prepare_resources(
            client,
            apartment,
            || {
                Ok(Event::new(
                    unsafe { CreateEventW(None, false, false, None)? },
                    close_event,
                    close,
                ))
            },
            |resources| -> windows::core::Result<u32> {
                let format = wave_format(sample_rate, layout)?;
                unsafe {
                    resources.client.Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_LOOPBACK
                            | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                        0,
                        0,
                        &format.Format,
                        None,
                    )?;
                    resources.client.SetEventHandle(*resources.event.handle())?;
                }
                let maximum_frames = unsafe { resources.client.GetBufferSize()? };
                if maximum_frames == 0 || maximum_frames as usize > MAX_CALLBACK_FRAMES {
                    return Err(windows::core::Error::from_hresult(
                        AUDCLNT_E_BUFFER_SIZE_ERROR,
                    ));
                }
                resources.capture =
                    Some(unsafe { resources.client.GetService::<IAudioCaptureClient>()? });
                let mut task_index = 0;
                resources.realtime = Some(Realtime(unsafe {
                    AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index)?
                }));
                Ok(maximum_frames)
            },
        )?;
        let NativeResources {
            capture,
            client,
            event,
            realtime,
            apartment,
        } = resources;
        Ok(Self {
            resources: NativeResources {
                capture: capture.expect("configured capture service"),
                client,
                event,
                realtime: realtime.expect("configured realtime task"),
                apartment,
            },
            stop_required: StopRequirement::default(),
            maximum_frames,
        })
    }

    fn stop_status(&mut self) -> windows::core::Result<Option<i32>> {
        // Preserve S_FALSE as well as S_OK. A failed Stop cannot establish that
        // the producer is quiescent or permit event/client destruction.
        self.stop_required
            .stop(|| {
                let status = unsafe {
                    (self.resources.client.vtable().Stop)(self.resources.client.as_raw())
                };
                status
                    .ok()
                    .map(|()| status.0)
                    .map_err(|error| error.code().0)
            })
            .map_err(|status| windows::core::Error::from_hresult(windows::core::HRESULT(status)))
    }
    pub(crate) fn stop(&mut self) -> windows::core::Result<()> {
        self.stop_status().map(|_| ())
    }
    pub(crate) fn wait_and_drain(
        &mut self,
        state: &mut CaptureState,
        tracker: &mut Tracker,
        layout: Layout,
        active: &AtomicBool,
    ) -> Result<(), AudioFault> {
        let waited = unsafe { WaitForSingleObject(*self.resources.event.handle(), 20) };
        if waited != WAIT_OBJECT_0 && waited != WAIT_TIMEOUT {
            return Err(AudioFault::Backend);
        }
        self.drain(state, tracker, layout, active)
    }
    pub(crate) fn start(&mut self) -> windows::core::Result<()> {
        unsafe {
            ResetEvent(*self.resources.event.handle())?;
            self.stop_required.start_entered();
            self.resources.client.Start()?;
        }
        Ok(())
    }
    fn drain(
        &mut self,
        state: &mut CaptureState,
        tracker: &mut Tracker,
        layout: Layout,
        active: &AtomicBool,
    ) -> Result<(), AudioFault> {
        // A faulty producer cannot keep us in an unbounded draining loop. More
        // than 64 queued engine packets is already well beyond our working set.
        for _ in 0..64 {
            if !active.load(Ordering::Acquire) || !state.authorized() {
                return Ok(());
            }
            let available = unsafe { self.resources.capture.GetNextPacketSize() }.map_err(|e| {
                let fault = hresult_fault(e.code());
                state.fail_status(fault, e.code().0);
                fault
            })?;
            if available == 0 {
                return Ok(());
            }
            if available > self.maximum_frames {
                return Err(AudioFault::InvalidBuffer);
            }
            let mut data = std::ptr::null_mut();
            let mut frames = 0;
            let mut flags = 0;
            let mut position = 0;
            let mut qpc_100ns = 0;
            unsafe {
                self.resources.capture.GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    Some(&mut position),
                    Some(&mut qpc_100ns),
                )
            }
            .map_err(|e| {
                let fault = hresult_fault(e.code());
                state.fail_status(fault, e.code().0);
                fault
            })?;
            if frames == 0 {
                return Ok(());
            } // AUDCLNT_S_BUFFER_EMPTY: no lease.
            let release_status = std::cell::Cell::new(None);
            let release = ReleaseGuard::new(frames, |count| {
                unsafe { self.resources.capture.ReleaseBuffer(count) }.map_err(|e| {
                    release_status.set(Some(e.code().0));
                    hresult_fault(e.code())
                })
            });
            let processed = (|| {
                if frames > self.maximum_frames {
                    return Err(AudioFault::InvalidBuffer);
                }
                let length = layout.bytes(frames as usize)?;
                let bytes = if flags & SILENT != 0 {
                    None
                } else {
                    if data.is_null() {
                        return Err(AudioFault::InvalidBuffer);
                    }
                    // SAFETY: successful nonempty GetBuffer owns this packet's
                    // frames*nBlockAlign bytes until release below. The exact
                    // negotiated layout is validated; decoding is unaligned-safe.
                    Some(unsafe { std::slice::from_raw_parts(data.cast_const(), length) })
                };
                let packet = tracker.accept(
                    Metadata {
                        frames: frames as usize,
                        flags,
                        position,
                        qpc_100ns,
                    },
                    layout,
                    bytes,
                )?;
                if active.load(Ordering::Acquire) && state.authorized() {
                    state.push_frames(packet.frames, |index| packet.frame(index));
                }
                Ok(())
            })();
            let released = release.finish();
            processed?;
            if let Err(fault) = released {
                if let Some(status) = release_status.get() {
                    state.fail_status(fault, status);
                }
                return Err(fault);
            }
        }
        if !active.load(Ordering::Acquire) || !state.authorized() {
            return Ok(());
        }
        // Exactly 64 packets can exhaust the queue without exhausting resources.
        // One final count query distinguishes that case without another lease.
        match unsafe { self.resources.capture.GetNextPacketSize() }.map_err(|e| {
            let fault = hresult_fault(e.code());
            state.fail_status(fault, e.code().0);
            fault
        })? {
            0 => Ok(()),
            _ => Err(AudioFault::ResourceExhausted),
        }
    }
}
impl Drop for NativeCapture {
    fn drop(&mut self) {
        if self.stop_required.required() {
            let _ = unsafe { self.resources.client.Stop() };
        }
    }
}
pub(crate) fn hresult_fault(code: windows::core::HRESULT) -> AudioFault {
    match code {
        AUDCLNT_E_DEVICE_INVALIDATED | AUDCLNT_E_ENDPOINT_CREATE_FAILED => {
            AudioFault::DeviceUnavailable
        }
        AUDCLNT_E_RESOURCES_INVALIDATED | AUDCLNT_E_SERVICE_NOT_RUNNING => {
            AudioFault::StreamInvalidated
        }
        E_ACCESSDENIED => AudioFault::PermissionDenied,
        E_OUTOFMEMORY => AudioFault::ResourceExhausted,
        _ => AudioFault::Backend,
    }
}
fn wave_format(sample_rate: u32, layout: Layout) -> windows::core::Result<WAVEFORMATEXTENSIBLE> {
    let block = layout
        .bytes(1)
        .map_err(|_| windows::core::Error::from_hresult(AUDCLNT_E_UNSUPPORTED_FORMAT))?;
    let block = u16::try_from(block)
        .map_err(|_| windows::core::Error::from_hresult(AUDCLNT_E_UNSUPPORTED_FORMAT))?;
    let average = sample_rate
        .checked_mul(u32::from(block))
        .ok_or_else(|| windows::core::Error::from_hresult(AUDCLNT_E_UNSUPPORTED_FORMAT))?;
    let bits = (layout.encoding.bytes() * 8) as u16;
    let valid_bits = if matches!(layout.encoding, Encoding::I24In32) {
        24
    } else {
        bits
    };
    let float = matches!(layout.encoding, Encoding::F32 | Encoding::F64);
    Ok(WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: layout.channels,
            nSamplesPerSec: sample_rate,
            nAvgBytesPerSec: average,
            nBlockAlign: block,
            wBitsPerSample: bits,
            cbSize: (std::mem::size_of::<WAVEFORMATEXTENSIBLE>()
                - std::mem::size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: valid_bits,
        },
        dwChannelMask: match layout.channels {
            1 => SPEAKER_FRONT_CENTER,
            2 => SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT,
            _ => 0,
        },
        SubFormat: if float {
            KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        } else {
            KSDATAFORMAT_SUBTYPE_PCM
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_flag_values_and_wave_format_match_packet_contract() {
        assert_eq!(
            super::super::wasapi_packet::SILENT,
            AUDCLNT_BUFFERFLAGS_SILENT.0 as u32
        );
        assert_eq!(
            super::super::wasapi_packet::DISCONTINUITY,
            AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32
        );
        assert_eq!(
            super::super::wasapi_packet::TIMESTAMP_ERROR,
            AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32
        );
        let wave = wave_format(
            48000,
            Layout {
                channels: 2,
                encoding: Encoding::I24In32,
            },
        )
        .unwrap();
        let align = wave.Format.nBlockAlign;
        let bits = wave.Format.wBitsPerSample;
        let valid = unsafe { wave.Samples.wValidBitsPerSample };
        let average = wave.Format.nAvgBytesPerSec;
        assert_eq!((align, bits, valid, average), (8, 32, 24, 384000));
    }
    #[test]
    fn native_device_loss_errors_are_specific_and_do_not_reopen_defaults() {
        assert_eq!(
            hresult_fault(AUDCLNT_E_DEVICE_INVALIDATED),
            AudioFault::DeviceUnavailable
        );
        assert_eq!(
            hresult_fault(AUDCLNT_E_RESOURCES_INVALIDATED),
            AudioFault::StreamInvalidated
        );
        assert_eq!(hresult_fault(E_ACCESSDENIED), AudioFault::PermissionDenied);
    }
}
