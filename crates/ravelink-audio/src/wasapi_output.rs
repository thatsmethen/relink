//! Original explicit-endpoint WASAPI render owner. Every COM operation and
//! acquired render lease stays on one MTA thread; public handles only use atomics.
use crate::{
    backend,
    output_sink::RenderOutcome,
    wasapi_output_state::{
        Gates, WasapiOutputOperation as Op, WasapiOutputPhase as Phase, WasapiOutputTiming,
        OWNER_STACK_BYTES, RESERVATIONS,
    },
    wasapi_packet::{Encoding, Layout, SILENT},
    wasapi_render_packet::{Error as PacketError, RenderLease},
    AudioError, AudioFault, FaultOrigin, OutputDevice, OutputPermit, OutputRenderer,
    COMMAND_TIMEOUT, MAX_CALLBACK_FRAMES, MAX_DEVICE_CHANNELS,
};
use std::{
    cell::Cell,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use windows::{
    core::{w, Interface, HRESULT, PCWSTR},
    Win32::{
        Foundation::{
            CloseHandle, E_ACCESSDENIED, E_FAIL, E_OUTOFMEMORY, HANDLE, S_OK, WAIT_FAILED,
            WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Media::{
            Audio::*,
            KernelStreaming::{
                KSDATAFORMAT_SUBTYPE_PCM, SPEAKER_FRONT_LEFT, SPEAKER_FRONT_RIGHT,
                WAVE_FORMAT_EXTENSIBLE,
            },
            Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        },
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
                COINIT_MULTITHREADED,
            },
            Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
            Threading::{
                AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
                ResetEvent, WaitForSingleObject,
            },
        },
    },
};

pub(crate) struct Selection {
    id: Box<str>,
    endpoint: Box<[u16]>,
    rate: u32,
    layout: Layout,
    format: cpal::SampleFormat,
}
pub(crate) fn select(
    device: &OutputDevice,
    config: &cpal::SupportedStreamConfig,
) -> Result<Selection, AudioError> {
    let parsed: cpal::DeviceId = device
        .id
        .0
        .parse()
        .map_err(|_| AudioError::InvalidOutputId(device.id.0.clone()))?;
    if parsed.host() != cpal::HostId::Wasapi
        || parsed.id().is_empty()
        || parsed.id().contains('\0')
        || parsed.id().len() > 32_768
    {
        return Err(AudioError::InvalidOutputId(device.id.0.clone()));
    }
    let unsupported = || AudioError::UnsupportedConfiguration {
        device: device.name.clone(),
        sample_rate: config.sample_rate(),
    };
    let encoding = Encoding::from_cpal(config.sample_format()).ok_or_else(unsupported)?;
    if !(2..=MAX_DEVICE_CHANNELS).contains(&config.channels())
        || !(8000..=384000).contains(&config.sample_rate())
    {
        return Err(unsupported());
    }
    let selection = Selection {
        id: parsed.id().into(),
        endpoint: parsed.id().encode_utf16().chain(Some(0)).collect(),
        rate: config.sample_rate(),
        layout: Layout {
            channels: config.channels(),
            encoding,
        },
        format: config.sample_format(),
    };
    wave_format(selection.rate, selection.layout).map_err(|_| unsupported())?;
    Ok(selection)
}
impl Selection {
    pub(crate) fn channels(&self) -> u16 {
        self.layout.channels
    }
    pub(crate) fn rate(&self) -> u32 {
        self.rate
    }
    pub(crate) fn description(&self) -> String {
        format!("{:?}", self.format)
    }
    // Rust-owned payloads only: explicit stack, Arcs, selection/registry strings,
    // native wrapper and fixed registry. COM/audio engine allocations are OS-owned.
    pub(crate) fn owner_bytes(&self) -> Result<usize, AudioError> {
        [
            std::mem::size_of::<Self>(),
            self.id.len(),
            self.id.len(),
            self.endpoint.len() * 2,
            std::mem::size_of_val(&RESERVATIONS),
            std::mem::size_of::<WasapiOutput>(),
            std::mem::size_of::<Native>(),
            std::mem::size_of::<Gates>(),
            std::mem::size_of::<WasapiOutputTiming>(),
            4 * std::mem::size_of::<usize>(),
            OWNER_STACK_BYTES,
        ]
        .into_iter()
        .try_fold(0usize, usize::checked_add)
        .ok_or_else(|| AudioError::InvalidSession("WASAPI owner budget overflow".into()))
    }
}

pub(crate) struct WasapiOutput {
    gates: Arc<Gates>,
    timing: Arc<WasapiOutputTiming>,
    permit: OutputPermit,
}
impl WasapiOutput {
    pub(crate) fn build(
        selection: Selection,
        render: OutputRenderer,
        timing: Arc<WasapiOutputTiming>,
    ) -> Result<Self, AudioError> {
        let permit = render.permit();
        timing.configure(permit.epoch(), 0, 0);
        if !permit.authorized() {
            return Err(AudioError::Cancelled);
        }
        let reservation = Reservation::acquire(&selection.id)?;
        let gates = Arc::new(Gates::default());
        let handle = Self {
            gates: gates.clone(),
            timing: timing.clone(),
            permit,
        };
        thread::Builder::new()
            .name("ravelink-wasapi-output".into())
            .stack_size(OWNER_STACK_BYTES)
            .spawn(move || owner(selection, render, gates, timing, reservation))
            .map_err(|error| backend("WASAPI output owner creation", None, error))?;
        handle.wait(COMMAND_TIMEOUT, "output creation", || {
            handle.gates.is_ready()
        })?;
        Ok(handle)
    }
    pub(crate) fn start(&self) -> Result<(), AudioError> {
        if !self.permit.authorized() || !self.gates.request_start() {
            self.cancel();
            return Err(AudioError::Cancelled);
        }
        self.wait(COMMAND_TIMEOUT, "output activation", || {
            self.gates.is_started()
        })
    }
    pub(crate) fn wait_for_progress(&self, timeout: Duration) -> Result<(), AudioError> {
        self.wait(timeout, "output startup progress", || {
            self.gates.is_started() && self.progress_count() > 0
        })
    }
    pub(crate) fn activate(&self) -> Result<(), AudioError> {
        if !self.permit.authorized() || !self.gates.activate(self.progress_count()) {
            self.cancel();
            return Err(AudioError::Cancelled);
        }
        // The callback also checks both gates at final release; this recheck
        // avoids reporting activation success after already observed revocation.
        if !self.permit.authorized() || !self.gates.alive() {
            self.cancel();
            return Err(AudioError::Cancelled);
        }
        Ok(())
    }
    pub(crate) fn cancel(&self) {
        self.gates.cancel();
    }
    pub(crate) fn progress_count(&self) -> u64 {
        self.timing.progress_count()
    }
    #[allow(dead_code)] // Also useful to callers which did not retain build diagnostics.
    pub(crate) fn diagnostics(&self) -> Arc<WasapiOutputTiming> {
        self.timing.clone()
    }
    fn wait(
        &self,
        timeout: Duration,
        operation: &'static str,
        done: impl Fn() -> bool,
    ) -> Result<(), AudioError> {
        let start = Instant::now();
        loop {
            if let Some(failure) = self.timing.failure() {
                self.cancel();
                return Err(backend(
                    operation,
                    None,
                    match failure.status {
                        Some(status) => format!(
                            "native {:?}: HRESULT {:#010x}",
                            failure.operation, status as u32
                        ),
                        None => format!(
                            "adapter {:?} failed without a native HRESULT",
                            failure.operation
                        ),
                    },
                ));
            }
            if !self.gates.alive() || !self.permit.authorized() {
                self.cancel();
                return Err(AudioError::Cancelled);
            }
            if start.elapsed() >= timeout {
                self.cancel();
                self.permit.fail_at(
                    AudioFault::Backend,
                    FaultOrigin::NativeOutputStartup,
                    self.permit.metadata(),
                );
                return Err(AudioError::Timeout { operation });
            }
            if done() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for WasapiOutput {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Reservation {
    slot: usize,
    proven: bool,
}
impl Reservation {
    fn acquire(id: &str) -> Result<Self, AudioError> {
        let mut registry = RESERVATIONS
            .lock()
            .map_err(|_| AudioError::RuntimeUnavailable)?;
        let slot = registry.reserve(id).ok_or_else(|| {
            backend(
                "WASAPI output reservation",
                None,
                "endpoint already owned or bounded reservation capacity exhausted",
            )
        })?;
        Ok(Self { slot, proven: true })
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if self.proven {
            if let Ok(mut registry) = RESERVATIONS.lock() {
                registry.release(self.slot);
            }
        }
        // Otherwise the fixed registry entry remains. Neither a handle timeout
        // nor an unproven native teardown authorizes a new owner of this ID.
    }
}

fn owner(
    selection: Selection,
    mut render: OutputRenderer,
    gates: Arc<Gates>,
    timing: Arc<WasapiOutputTiming>,
    mut reservation: Reservation,
) {
    let permit = render.permit();
    if !live(&gates, &permit) {
        return;
    }
    reservation.proven = false;
    timing.event(Phase::OpenRequested, ticks(&timing), None);
    timing.entered(Op::Open);
    let opened = Native::open(&selection, &timing);
    let status = opened
        .as_ref()
        .map_or_else(|error| error.status.map(|value| value.0), |_| Some(0));
    if let Some(status) = status {
        timing.returned(Op::Open, status);
        if opened.is_err() {
            timing.failed(Op::Open, status);
        }
    } else {
        timing.validation_failed();
    }
    timing.event(Phase::OpenReturned, ticks(&timing), status);
    let mut native = match opened {
        Ok(native) => native,
        Err(error) => {
            fail(&permit, &gates, error, FaultOrigin::NativeOutputStartup);
            // open's native locals have all been released before its Err return.
            let proven = timing.cleanup_proven();
            reservation.proven = proven;
            drop(reservation);
            timing.event(
                Phase::NativeReleaseReturned,
                ticks(&timing),
                proven.then_some(0),
            );
            timing.released(proven);
            timing.event(
                if proven {
                    Phase::OwnerCompleted
                } else {
                    Phase::ReservationRetained
                },
                ticks(&timing),
                None,
            );
            return;
        }
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(
            &selection,
            &mut native,
            &mut render,
            &gates,
            &permit,
            &timing,
        )
    }))
    .unwrap_or(Err(OwnerFailure::validation(AudioFault::Render)));
    if let Err(error) = result {
        if error.status.is_none() {
            timing.validation_failed();
        }
        fail(&permit, &gates, error, FaultOrigin::WasapiOutput);
    }
    gates.cancel();
    let mut proven = native.release_proven && timing.cleanup_proven();
    if native.started {
        timing.event(Phase::StopRequested, ticks(&timing), None);
        timing.entered(Op::Stop);
        // Preserve S_FALSE as well as failures; generated Stop() erases success codes.
        let status = unsafe { (native.client.vtable().Stop)(native.client.as_raw()) };
        native.started = false;
        timing.returned(Op::Stop, status.0);
        timing.event(Phase::StopReturned, ticks(&timing), Some(status.0));
        if status.is_err() {
            proven = false;
            fail(
                &permit,
                &gates,
                status.into(),
                FaultOrigin::NativeOutputLifecycle,
            );
        }
    }
    timing.event(Phase::NativeReleaseRequested, ticks(&timing), None);
    timing.entered(Op::NativeRelease);
    let release_status = native.release();
    timing.returned(Op::NativeRelease, release_status.0);
    timing.event(
        Phase::NativeReleaseReturned,
        ticks(&timing),
        Some(release_status.0),
    );
    if release_status.is_err() {
        proven = false;
        fail(
            &permit,
            &gates,
            release_status.into(),
            FaultOrigin::NativeOutputLifecycle,
        );
    }
    reservation.proven = proven;
    drop(reservation); // Registry lock is outside packet processing and native calls.
    timing.released(proven);
    timing.event(
        if proven {
            Phase::OwnerCompleted
        } else {
            Phase::ReservationRetained
        },
        ticks(&timing),
        None,
    );
}
fn live(gates: &Gates, permit: &OutputPermit) -> bool {
    gates.alive() && permit.authorized()
}
#[derive(Clone, Copy, Debug)]
struct OwnerFailure {
    fault: AudioFault,
    status: Option<HRESULT>,
}
impl OwnerFailure {
    fn validation(fault: AudioFault) -> Self {
        Self {
            fault,
            status: None,
        }
    }
}
impl From<HRESULT> for OwnerFailure {
    fn from(status: HRESULT) -> Self {
        Self {
            fault: hresult_fault(status),
            status: Some(status),
        }
    }
}
impl From<windows::core::Error> for OwnerFailure {
    fn from(error: windows::core::Error) -> Self {
        error.code().into()
    }
}
fn fail(permit: &OutputPermit, gates: &Gates, error: OwnerFailure, origin: FaultOrigin) {
    gates.cancel();
    let mut metadata = permit.metadata();
    metadata.status = error.status.map(|code| code.0);
    permit.fail_at(error.fault, origin, metadata);
}

fn run(
    selection: &Selection,
    native: &mut Native,
    render: &mut OutputRenderer,
    gates: &Gates,
    permit: &OutputPermit,
    timing: &WasapiOutputTiming,
) -> Result<(), OwnerFailure> {
    if !live(gates, permit) {
        return Ok(());
    }
    timing.event(Phase::PrefillRequested, ticks(timing), None);
    let filled = native.packet(
        native.maximum_frames,
        selection.layout,
        render,
        gates,
        permit,
        timing,
        true,
    );
    timing.event(
        Phase::PrefillReturned,
        ticks(timing),
        filled
            .as_ref()
            .map_or_else(|e| e.status.map(|code| code.0), |_| Some(0)),
    );
    filled?;
    if !live(gates, permit) {
        return Ok(());
    }
    gates.ready();
    while !gates.start_requested() {
        if !live(gates, permit) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(2));
    }
    if !live(gates, permit) {
        return Ok(());
    }
    timing.entered(Op::ResetEvent);
    let reset = unsafe { ResetEvent(native.event.0) };
    timing.returned(
        Op::ResetEvent,
        reset.as_ref().err().map_or(0, |e| e.code().0),
    );
    reset.map_err(|e| e.code())?;
    if !live(gates, permit) {
        return Ok(());
    }
    timing.event(Phase::StartRequested, ticks(timing), None);
    timing.entered(Op::Start);
    let status = unsafe { (native.client.vtable().Start)(native.client.as_raw()) };
    timing.returned(Op::Start, status.0);
    timing.event(Phase::StartReturned, ticks(timing), Some(status.0));
    if status.is_err() {
        return Err(status.into());
    }
    native.started = true;
    gates.started();
    while live(gates, permit) {
        if let Some(failure) = timing.failure() {
            return Err(failure.status.map_or_else(
                || OwnerFailure::validation(AudioFault::InvalidBuffer),
                |status| HRESULT(status).into(),
            ));
        }
        timing.entered(Op::Wait);
        let waited = unsafe { WaitForSingleObject(native.event.0, 20) };
        if waited != WAIT_OBJECT_0 && waited != WAIT_TIMEOUT {
            if waited == WAIT_FAILED {
                let status = windows::core::Error::from_thread().code();
                if status.is_err() {
                    timing.returned(Op::Wait, status.0);
                    return Err(status.into());
                }
            }
            return Err(OwnerFailure::validation(AudioFault::Backend));
        }
        timing.returned(Op::Wait, 0);
        if !live(gates, permit) {
            break;
        }
        // Event waits are outside every acquired lease. Timeout probes padding,
        // allowing disconnected/service errors to surface without an event.
        timing.entered(Op::Padding);
        let padding = unsafe { native.client.GetCurrentPadding() };
        timing.returned(
            Op::Padding,
            padding.as_ref().err().map_or(0, |e| e.code().0),
        );
        let padding = padding.map_err(|e| e.code())?;
        let available = native
            .maximum_frames
            .checked_sub(padding)
            .ok_or_else(|| OwnerFailure::validation(AudioFault::InvalidBuffer))?;
        if available == 0 {
            continue;
        }
        native.packet(
            available,
            selection.layout,
            render,
            gates,
            permit,
            timing,
            false,
        )?;
    }
    Ok(())
}

struct Apartment;
impl Apartment {
    fn open() -> windows::core::Result<Self> {
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
struct Event(HANDLE, Arc<WasapiOutputTiming>);
impl Drop for Event {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            if let Err(error) = unsafe { CloseHandle(self.0) } {
                self.1.cleanup_error(error.code().0);
            }
        }
    }
}
struct Realtime(HANDLE, Arc<WasapiOutputTiming>);
impl Drop for Realtime {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            if let Err(error) = unsafe { AvRevertMmThreadCharacteristics(self.0) } {
                self.1.cleanup_error(error.code().0);
            }
        }
    }
}
struct Native {
    // Field order protects event/MMCSS/apartment through COM interface release.
    render: IAudioRenderClient,
    client: IAudioClient,
    event: Event,
    realtime: Realtime,
    apartment: Apartment,
    started: bool,
    maximum_frames: u32,
    release_proven: bool,
}
impl Native {
    fn open(selection: &Selection, timing: &Arc<WasapiOutputTiming>) -> Result<Self, OwnerFailure> {
        // Reverse local drop order on every early error releases COM before apartment.
        let apartment = Apartment::open()?;
        let mut frequency = 0;
        unsafe {
            QueryPerformanceFrequency(&mut frequency)?;
        }
        if frequency <= 0 {
            return Err(OwnerFailure::validation(AudioFault::Backend));
        }
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        let device = unsafe { enumerator.GetDevice(PCWSTR(selection.endpoint.as_ptr()))? };
        let endpoint: IMMEndpoint = device.cast()?;
        if unsafe { endpoint.GetDataFlow()? } != eRender {
            return Err(OwnerFailure::validation(AudioFault::DeviceChanged));
        }
        if unsafe { device.GetState()? } != DEVICE_STATE_ACTIVE {
            return Err(OwnerFailure::validation(AudioFault::DeviceUnavailable));
        }
        // Reject alternate strings resolving to the same device; the reservation
        // uses the exact native identity originally exposed by discovery.
        let actual = unsafe { device.GetId()? };
        let matches = if actual.is_null() {
            false
        } else {
            selection
                .endpoint
                .iter()
                .enumerate()
                .all(|(index, value)| unsafe { *actual.0.add(index) == *value })
        };
        unsafe {
            CoTaskMemFree(Some(actual.0.cast()));
        }
        if !matches {
            return Err(OwnerFailure::validation(AudioFault::DeviceUnavailable));
        }
        // Event must outlive the client, including errors after SetEventHandle.
        let event = Event(
            unsafe { CreateEventW(None, false, false, None)? },
            timing.clone(),
        );
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None)? };
        let format = wave_format(selection.rate, selection.layout)
            .map_err(|_| OwnerFailure::validation(AudioFault::InvalidBuffer))?;
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
            return Err(if supported.is_err() {
                supported.into()
            } else {
                OwnerFailure {
                    fault: AudioFault::DeviceChanged,
                    status: Some(supported),
                }
            });
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
            client.SetEventHandle(event.0)?;
        }
        let maximum_frames = unsafe { client.GetBufferSize()? };
        if maximum_frames == 0 || maximum_frames as usize > MAX_CALLBACK_FRAMES {
            return Err(OwnerFailure::validation(AudioFault::InvalidBuffer));
        }
        let render: IAudioRenderClient = unsafe { client.GetService()? };
        let mut task_index = 0;
        let realtime = Realtime(
            unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index)? },
            timing.clone(),
        );
        timing.configure(timing.snapshot().epoch, frequency as u64, maximum_frames);
        Ok(Self {
            render,
            client,
            event,
            realtime,
            apartment,
            started: false,
            maximum_frames,
            release_proven: true,
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn packet(
        &mut self,
        frames: u32,
        layout: Layout,
        render: &mut OutputRenderer,
        gates: &Gates,
        permit: &OutputPermit,
        timing: &WasapiOutputTiming,
        prefill: bool,
    ) -> Result<(), OwnerFailure> {
        timing.begin_work(ticks(timing));
        timing.entered(Op::Acquire);
        let mut pointer = std::ptr::null_mut();
        let acquired =
            unsafe { (self.render.vtable().GetBuffer)(self.render.as_raw(), frames, &mut pointer) };
        timing.returned(Op::Acquire, acquired.0);
        if acquired.is_err() {
            timing.finish_work(ticks(timing), frames, true, None);
            return Err(acquired.into());
        }
        // A successful GetBuffer creates an obligation even if its pointer is
        // malformed. Never form a reference before validating pointer/extent.
        let extent = layout.bytes(frames as usize).ok();
        let bytes = match extent {
            Some(length)
                if length > 0
                    && !pointer.is_null()
                    && (pointer as usize).checked_add(length).is_some()
                    && length <= isize::MAX as usize =>
            {
                // WASAPI owns this unique byte extent until the matching release.
                Ok(unsafe { std::slice::from_raw_parts_mut(pointer, length) })
            }
            _ => Err(PacketError::InvalidBuffer),
        };
        let record = Cell::new(None);
        let released_status = Cell::new(E_FAIL);
        let mut version = None;
        let prepared: Result<(), OwnerFailure> = (|| {
            let mut lease = RenderLease::new(frames, layout, bytes, &record, |count, flags| {
                timing.entered(Op::Release);
                let status = unsafe {
                    (self.render.vtable().ReleaseBuffer)(self.render.as_raw(), count, flags)
                };
                released_status.set(status);
                timing.returned(Op::Release, status.0);
                if status.is_ok() {
                    Ok(())
                } else {
                    Err(status)
                }
            })
            .map_err(|_| OwnerFailure::validation(AudioFault::InvalidBuffer))?;
            if prefill || !gates.program() || !live(gates, permit) || !permit.program_authorized() {
                return lease.finish_silent().map_err(Into::into);
            }
            let outcome = render.write_to(&mut lease);
            match outcome {
                RenderOutcome::Rendered {
                    frames: rendered,
                    accepted_version,
                } if rendered == frames as usize => {
                    // This is the final software authorization point. Cancellation
                    // after native submission cannot retract the engine's buffer.
                    let authorized =
                        live(gates, permit) && gates.program() && permit.program_authorized();
                    let result = lease.commit(authorized);
                    if result.is_ok() {
                        version = accepted_version;
                    }
                    match result {
                        Ok(()) => Ok(()),
                        Err(crate::wasapi_render_packet::CommitError::Preparation(
                            PacketError::Cancelled,
                        )) => Ok(()),
                        Err(crate::wasapi_render_packet::CommitError::Preparation(_)) => {
                            Err(OwnerFailure::validation(AudioFault::InvalidBuffer))
                        }
                        Err(crate::wasapi_render_packet::CommitError::Release(status)) => {
                            Err(status.into())
                        }
                    }
                }
                RenderOutcome::Silence => lease.finish_silent().map_err(Into::into),
                _ => {
                    let _ = lease.finish_silent();
                    Err(OwnerFailure::validation(AudioFault::InvalidBuffer))
                }
            }
        })();
        let released = record
            .get()
            .expect("successful acquisition always releases exactly once");
        let status = released_status.get();
        timing.finish_work(
            ticks(timing),
            frames,
            released.flags == SILENT,
            Some(status.0),
        );
        if let Err(status) = released.result {
            self.release_proven = false;
            return Err(status.into());
        }
        if !prefill && released.flags == 0 && status.is_ok() {
            permit.submitted(frames as usize, version);
        }
        timing.submitted_pcm(
            frames,
            released.flags == 0,
            if released.flags == 0 { version } else { None },
            prefill,
            status.0,
        );
        prepared
    }
    fn release(self) -> HRESULT {
        // This method is the only normal teardown. If COM Release blocks, its
        // reservation stays held and NativeReleaseReturned remains absent.
        let Self {
            render,
            client,
            mut event,
            mut realtime,
            apartment,
            ..
        } = self;
        drop(render);
        drop(client);
        let mut first = S_OK;
        let closed = unsafe { CloseHandle(event.0) };
        if let Err(error) = closed {
            first = error.code();
        } else {
            event.0 = HANDLE::default();
        }
        let reverted = unsafe { AvRevertMmThreadCharacteristics(realtime.0) };
        if let Err(error) = reverted {
            if first.is_ok() {
                first = error.code();
            }
        } else {
            realtime.0 = HANDLE::default();
        }
        // Failed handle release is not retried; preserve the failure and retain
        // the endpoint reservation. The handles remain an explicit bounded leak.
        event.0 = HANDLE::default();
        realtime.0 = HANDLE::default();
        drop(event);
        drop(realtime);
        drop(apartment);
        first
    }
}
fn ticks(timing: &WasapiOutputTiming) -> u64 {
    let mut value = 0;
    match unsafe { QueryPerformanceCounter(&mut value) } {
        Ok(()) if value >= 0 => value as u64,
        Err(error) => {
            timing.failed(Op::Clock, error.code().0);
            0
        }
        Ok(()) => {
            timing.validation_failed();
            0
        }
    }
}
fn hresult_fault(status: HRESULT) -> AudioFault {
    match status {
        AUDCLNT_E_DEVICE_INVALIDATED | AUDCLNT_E_ENDPOINT_CREATE_FAILED => {
            AudioFault::DeviceUnavailable
        }
        AUDCLNT_E_RESOURCES_INVALIDATED | AUDCLNT_E_SERVICE_NOT_RUNNING => {
            AudioFault::StreamInvalidated
        }
        AUDCLNT_E_BUFFER_ERROR | AUDCLNT_E_BUFFER_SIZE_ERROR => AudioFault::InvalidBuffer,
        E_ACCESSDENIED => AudioFault::PermissionDenied,
        E_OUTOFMEMORY => AudioFault::ResourceExhausted,
        _ => AudioFault::Backend,
    }
}
fn wave_format(rate: u32, layout: Layout) -> windows::core::Result<WAVEFORMATEXTENSIBLE> {
    let invalid = || windows::core::Error::from_hresult(AUDCLNT_E_UNSUPPORTED_FORMAT);
    if layout.channels < 2 || !(8000..=384000).contains(&rate) {
        return Err(invalid());
    }
    let block = u16::try_from(layout.bytes(1).map_err(|_| invalid())?).map_err(|_| invalid())?;
    let bits = (layout.encoding.bytes() * 8) as u16;
    Ok(WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: layout.channels,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate.checked_mul(u32::from(block)).ok_or_else(invalid)?,
            nBlockAlign: block,
            wBitsPerSample: bits,
            cbSize: (std::mem::size_of::<WAVEFORMATEXTENSIBLE>()
                - std::mem::size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: if matches!(layout.encoding, Encoding::I24In32) {
                24
            } else {
                bits
            },
        },
        dwChannelMask: if layout.channels == 2 {
            SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT
        } else {
            0
        },
        SubFormat: if matches!(layout.encoding, Encoding::F32 | Encoding::F64) {
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
    fn exact_output_formats_match_sdk_fields_without_opening_devices() {
        for (encoding, bits, valid, align, floating) in [
            (Encoding::F32, 32, 32, 8, true),
            (Encoding::F64, 64, 64, 16, true),
            (Encoding::I16, 16, 16, 4, false),
            (Encoding::I24In32, 32, 24, 8, false),
            (Encoding::I32, 32, 32, 8, false),
            (Encoding::U8, 8, 8, 2, false),
        ] {
            let wave = wave_format(
                48_000,
                Layout {
                    channels: 2,
                    encoding,
                },
            )
            .unwrap();
            let format = wave.Format;
            let actual = (
                format.wBitsPerSample,
                unsafe { wave.Samples.wValidBitsPerSample },
                format.nBlockAlign,
            );
            assert_eq!(actual, (bits, valid, align));
            let average = format.nAvgBytesPerSec;
            assert_eq!(average, 48_000 * u32::from(align));
            let mask = wave.dwChannelMask;
            assert_eq!(mask, SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT);
            let subtype = wave.SubFormat;
            assert_eq!(
                subtype,
                if floating {
                    KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
                } else {
                    KSDATAFORMAT_SUBTYPE_PCM
                }
            );
        }
        let wave = wave_format(
            48_000,
            Layout {
                channels: 8,
                encoding: Encoding::F32,
            },
        )
        .unwrap();
        let mask = wave.dwChannelMask;
        assert_eq!(mask, 0, "direct channel order for multichannel layouts");
        for channels in [0, 1, 257, u16::MAX] {
            assert!(wave_format(
                48_000,
                Layout {
                    channels,
                    encoding: Encoding::F32
                }
            )
            .is_err());
        }
        assert_eq!(SILENT, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32);
    }
}
