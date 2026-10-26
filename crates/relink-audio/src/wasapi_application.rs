//! Original process-loopback owner. Only the selected INCLUDE process tree is
//! activated. Completion callbacks signal atomics; PCM stays on one MTA owner.
use super::{
    application_windows::Selection,
    backend,
    capture::{CaptureState, OWNER_STACK_BYTES},
    wasapi_capture::{hresult_fault, Apartment, NativeCapture},
    wasapi_event::{
        release_native, CleanupReservation, CloseRecord, Registry, Reserved, WasapiEventClose,
    },
    wasapi_packet::{Encoding, Layout, Tracker},
    AudioError, AudioFault, COMMAND_TIMEOUT,
};
use std::{
    mem::{size_of, ManuallyDrop},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};
use windows::{
    core::{implement, Interface, Ref, HRESULT},
    Win32::{
        Foundation::E_FAIL,
        Media::Audio::*,
        System::{
            Com::{
                StructuredStorage::{
                    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
                },
                BLOB,
            },
            Variant::VT_BLOB,
        },
    },
};

const POLL: Duration = Duration::from_millis(20);
const MONITOR_POLLS: usize = 12;
type Reply = mpsc::SyncSender<Result<(), AudioError>>;
enum Command {
    Configure(CaptureState),
    Start(Reply),
}

pub(crate) fn fixed_owner_bytes() -> usize {
    size_of::<ApplicationCapture>()
        + size_of::<NativeCapture>()
        + size_of::<CaptureState>()
        + size_of::<Activation>()
        + size_of::<Completion>()
        + size_of::<Tracker>()
        + size_of::<Reservation>()
        + 2 * size_of::<Reserved>()
        + size_of::<Owner>()
        // Also charge diagnostics to retained owners after the graph is gone.
        + CloseRecord::bytes()
        + size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>()
        + size_of::<PROPVARIANT>()
        + size_of::<AtomicBool>()
        + 20 * size_of::<usize>()
}
pub(crate) struct ApplicationCapture {
    commands: mpsc::SyncSender<Command>,
    active: Arc<AtomicBool>,
}
impl ApplicationCapture {
    pub(crate) fn build(selection: &Selection, state: CaptureState) -> Result<Self, AudioError> {
        if !state.authorized() {
            return Err(AudioError::Cancelled);
        }
        let close = state.event_close();
        let reservation = Reservation::acquire(selection, close.clone())?;
        let selection = selection.clone();
        let owner_close = close.clone();
        let (ready, received) = mpsc::sync_channel(1);
        let (commands, incoming) = mpsc::sync_channel(1);
        let active = Arc::new(AtomicBool::new(true));
        let running = active.clone();
        thread::Builder::new()
            .name("relink-application-wasapi".into())
            .stack_size(OWNER_STACK_BYTES)
            .spawn(move || {
                owner_loop(
                    selection,
                    reservation,
                    running,
                    incoming,
                    ready,
                    owner_close,
                )
            })
            .map_err(|e| backend("Windows application owner creation", None, e))?;
        let stream = Self { commands, active };
        if let Err(error) = receive(&received, "Windows process-loopback creation") {
            if let WasapiEventClose::Failed { hresult } = close.snapshot() {
                state.fail_status(hresult_fault(HRESULT(hresult)), hresult);
            }
            return Err(error);
        }
        // An activation that completes late never retains a graph/PCM ring: the
        // state remains here until the native owner has completed activation.
        if !state.authorized() || !stream.active.load(Ordering::Acquire) {
            return Err(AudioError::Cancelled);
        }
        stream
            .commands
            .try_send(Command::Configure(state))
            .map_err(|_| AudioError::RuntimeUnavailable)?;
        receive(&received, "Windows application monitor creation")?;
        Ok(stream)
    }
    pub(crate) fn play(&self) -> Result<(), AudioError> {
        let (reply, received) = mpsc::sync_channel(1);
        self.commands
            .try_send(Command::Start(reply))
            .map_err(|_| AudioError::RuntimeUnavailable)?;
        let result = receive(&received, "Windows process-loopback start");
        if result.is_err() {
            self.active.store(false, Ordering::Release);
        }
        result
    }
}
impl Drop for ApplicationCapture {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        // Never join a COM/driver call on the UI/control thread. Its reservation
        // remains charged until owner teardown completes, including late results.
    }
}
fn receive(
    receiver: &mpsc::Receiver<Result<(), AudioError>>,
    operation: &'static str,
) -> Result<(), AudioError> {
    match receiver.recv_timeout(COMMAND_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(AudioError::Timeout { operation }),
        Err(_) => Err(AudioError::RuntimeUnavailable),
    }
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Completion {
    complete: Arc<AtomicBool>,
}
impl IActivateAudioInterfaceCompletionHandler_Impl for Completion_Impl {
    fn ActivateCompleted(
        &self,
        _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        // windows::core::implement supplies IAgileObject/IMarshal by default.
        // No lock, channel send, allocation, native result access or PCM here.
        self.complete.store(true, Ordering::Release);
        Ok(())
    }
}
struct Activation {
    // Release the operation first; the system retains its handler until complete.
    operation: IActivateAudioInterfaceAsyncOperation,
    _handler: IActivateAudioInterfaceCompletionHandler,
    _variant: Box<PROPVARIANT>,
    _parameters: Box<AUDIOCLIENT_ACTIVATION_PARAMS>,
    complete: Arc<AtomicBool>,
}
impl Activation {
    fn begin(pid: u32) -> windows::core::Result<Self> {
        let mut parameters = Box::new(AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        });
        // PROPVARIANT is a borrowed VT_BLOB here. Do not PropVariantClear Rust's
        // Box allocation. Both objects remain owned through asynchronous completion.
        let variant = Box::new(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                            pBlobData: (&mut *parameters as *mut AUDIOCLIENT_ACTIVATION_PARAMS)
                                .cast(),
                        },
                    },
                    ..Default::default()
                }),
            },
        });
        let complete = Arc::new(AtomicBool::new(false));
        let handler: IActivateAudioInterfaceCompletionHandler = Completion {
            complete: complete.clone(),
        }
        .into();
        let operation = unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&*variant),
                &handler,
            )?
        };
        Ok(Self {
            operation,
            _handler: handler,
            _variant: variant,
            _parameters: parameters,
            complete,
        })
    }
    fn finish(&self) -> windows::core::Result<IAudioClient> {
        let mut status = HRESULT(E_FAIL.0);
        let mut unknown = None;
        // The completion signal is acquired before reading either result. Both
        // the method HRESULT and the activation HRESULT must succeed.
        unsafe {
            self.operation
                .GetActivateResult(&mut status, &mut unknown)?;
        }
        status.ok()?;
        unknown
            .ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?
            .cast()
    }
}
fn activation_error(name: String, error: windows::core::Error) -> AudioError {
    backend("Windows process-loopback activation", Some(name), format!(
        "INCLUDE_TARGET_PROCESS_TREE activation failed ({error}); process-loopback support and access are required. No system-audio fallback was attempted"))
}

struct Owner {
    native: Option<NativeCapture>,
    cleanup: CleanupReservation<Selection, Reservation>,
    close: Arc<CloseRecord>,
    state: Option<CaptureState>,
    active: Arc<AtomicBool>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        if let Err(status) = release_native(&mut self.native, &self.close, |native| {
            native.stop().map_err(|error| error.code().0)
        }) {
            if let Some(state) = &self.state {
                state.fail_status(hresult_fault(HRESULT(status)), status);
            }
            // Failed Stop retains native ownership too; failed event close has
            // already released COM and is never retried. Both keep the selected
            // process identity and reservation charged until process exit.
            self.cleanup.retain();
        }
    }
}
fn owner_loop(
    selection: Selection,
    reservation: Arc<Reservation>,
    active: Arc<AtomicBool>,
    incoming: mpsc::Receiver<Command>,
    ready: Reply,
    close: Arc<CloseRecord>,
) {
    struct Cancel(Arc<AtomicBool>);
    impl Drop for Cancel {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _cancel = Cancel(active.clone());
    // Established before any event can exist; also covers constructor unwind.
    // This Arc contains only close evidence, never a graph, permit or PCM ring.
    let cleanup = CleanupReservation::new(selection.clone(), reservation.clone(), close.clone());
    if !active.load(Ordering::Acquire) {
        let _ = ready.send(Err(AudioError::Cancelled));
        return;
    }
    if let Err(error) = selection.validate() {
        let _ = ready.send(Err(error));
        return;
    }
    let apartment = match Apartment::open() {
        Ok(value) => value,
        Err(error) => {
            let _ = ready.send(Err(activation_error(selection.name(), error)));
            return;
        }
    };
    let activation = match Activation::begin(selection.pid()) {
        Ok(value) => value,
        Err(error) => {
            let _ = ready.send(Err(activation_error(selection.name(), error)));
            return;
        }
    };
    // Windows exposes no cancellation API for this operation. A cancelled Start
    // returns to the caller at its deadline, while this bounded reserved owner
    // retains parameters/handler/operation until a possible late completion.
    while !activation.complete.load(Ordering::Acquire) {
        thread::sleep(POLL);
    }
    let client = match activation.finish() {
        Ok(client) => client,
        Err(error) => {
            let _ = ready.try_send(Err(activation_error(selection.name(), error)));
            return;
        }
    };
    // Drop the completed async operation/handler while this MTA apartment is
    // still local. NativeCapture later owns and uninitializes the apartment.
    drop(activation);
    if !active.load(Ordering::Acquire) {
        return;
    } // late interface drops on this MTA
    if let Err(error) = selection.validate() {
        let _ = ready.send(Err(error));
        return;
    }
    let config = selection.configuration();
    let layout = Layout {
        channels: 2,
        encoding: Encoding::F32,
    };
    let native = match NativeCapture::from_process_client(
        client,
        apartment,
        config.sample_rate(),
        layout,
        close.clone(),
    ) {
        Ok(native) => native,
        Err(error) => {
            let _ = ready.send(Err(activation_error(selection.name(), error)));
            return;
        }
    };
    let mut owner = Owner {
        native: Some(native),
        cleanup,
        close,
        state: None,
        active: active.clone(),
    };
    if !active.load(Ordering::Acquire) || ready.send(Ok(())).is_err() {
        return;
    }
    let state = loop {
        if !active.load(Ordering::Acquire) {
            return;
        }
        match incoming.recv_timeout(POLL) {
            Ok(Command::Configure(state)) => break state,
            Ok(Command::Start(reply)) => {
                let _ = reply.send(Err(AudioError::RuntimeUnavailable));
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return,
        }
    };
    if !state.authorized() {
        let _ = ready.send(Err(AudioError::Cancelled));
        return;
    }
    let permit = state.permit();
    owner.state = Some(state);
    let monitoring = active.clone();
    let monitor_selection = selection.clone();
    let monitor_reservation = reservation.clone();
    if let Err(error) = thread::Builder::new()
        .name("relink-application-membership".into())
        .stack_size(OWNER_STACK_BYTES)
        .spawn(move || {
            let _reservation = monitor_reservation;
            while monitoring.load(Ordering::Acquire) && permit.authorized() {
                // All OS discovery, path allocation and topology work is outside the
                // packet owner and the completion/audio callback paths.
                if monitor_selection.validate().is_err() {
                    permit.fail_at(
                        AudioFault::DeviceChanged,
                        super::FaultOrigin::WasapiInput,
                        permit.metadata(),
                    );
                    monitoring.store(false, Ordering::Release);
                    return;
                }
                for _ in 0..MONITOR_POLLS {
                    if !monitoring.load(Ordering::Acquire) || !permit.authorized() {
                        return;
                    }
                    thread::sleep(POLL);
                }
            }
        })
    {
        let _ = ready.send(Err(backend(
            "Windows application membership monitor",
            None,
            error,
        )));
        return;
    }
    if ready.send(Ok(())).is_err() {
        return;
    }
    loop {
        if !active.load(Ordering::Acquire) || !owner.state.as_ref().unwrap().authorized() {
            return;
        }
        match incoming.recv_timeout(POLL) {
            Ok(Command::Start(reply)) => {
                // Recheck on this control phase immediately before Start. The
                // background monitor then faults any subsequent membership drift.
                if let Err(error) = selection.validate() {
                    let _ = reply.send(Err(error));
                    return;
                }
                if !active.load(Ordering::Acquire) || !owner.state.as_ref().unwrap().authorized() {
                    let _ = reply.send(Err(AudioError::Cancelled));
                    return;
                }
                if let Err(error) = owner.native.as_mut().unwrap().start() {
                    owner
                        .state
                        .as_ref()
                        .unwrap()
                        .fail_status(hresult_fault(error.code()), error.code().0);
                    let _ = reply.send(Err(activation_error(selection.name(), error)));
                    return;
                }
                if !active.load(Ordering::Acquire) || !owner.state.as_ref().unwrap().authorized() {
                    let _ = reply.send(Err(AudioError::Cancelled));
                    return;
                }
                if reply.send(Ok(())).is_err() {
                    return;
                }
                break;
            }
            Ok(Command::Configure(_)) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return,
        }
    }
    let mut tracker = Tracker::default();
    while active.load(Ordering::Acquire) && owner.state.as_ref().unwrap().authorized() {
        let state = owner.state.as_mut().unwrap();
        if let Err(fault) =
            owner
                .native
                .as_mut()
                .unwrap()
                .wait_and_drain(state, &mut tracker, layout, &active)
        {
            state.fail(fault);
            return;
        }
    }
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());
struct Reservation {
    key: String,
}
impl Reservation {
    fn acquire(selection: &Selection, close: Arc<CloseRecord>) -> Result<Arc<Self>, AudioError> {
        let key = selection.key();
        REGISTRY
            .lock()
            .map_err(|_| {
                backend(
                    "application owner reservation",
                    None,
                    "owner registry is unavailable",
                )
            })?
            .acquire(
                key.clone(),
                selection.scope(),
                selection.owner_bytes()?,
                close,
            )?;
        Ok(Arc::new(Self { key }))
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if let Ok(mut registry) = REGISTRY.lock() {
            registry.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_is_preallocated_atomic_and_never_reads_native_results() {
        let flag = Arc::new(AtomicBool::new(false));
        let handler: IActivateAudioInterfaceCompletionHandler = Completion {
            complete: flag.clone(),
        }
        .into();
        // A null operation is deliberately sufficient: the callback only signals.
        unsafe {
            handler.ActivateCompleted(None).unwrap();
        }
        assert!(flag.load(Ordering::Acquire));
    }
}
