//! Original CoreAudio output IOProc owner. All native resources live on its
//! dedicated thread; callbacks only validate/encode bounded PCM and atomics.
use crate::{
    backend,
    native_output_packet::{OutputLayout, PlaneBuffer},
    native_output_timing::{
        NativeOutputCompletion, NativeOutputFault, NativeOutputFaultReason,
        NativeOutputLifecycleEvent, NativeOutputLifecyclePhase, NativeOutputOutcome,
        NativeOutputPacket, NativeOutputStamp, NativeOutputTiming, Tracker,
    },
    native_packet::Format,
    output_sink::RenderOutcome,
    AudioError, AudioFault, FaultMetadata, FaultOrigin, FaultProperty, OutputPermit,
    OutputRenderer, COMMAND_TIMEOUT, MAX_DEVICE_CHANNELS,
};
use block2::RcBlock;
use objc2_core_audio::*;
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp,
};
use objc2_core_foundation::{CFRetained, CFString};
use std::{
    cell::UnsafeCell,
    ffi::c_void,
    mem::{offset_of, size_of, MaybeUninit},
    ptr::{self, NonNull},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const MAX_PROPERTY_BYTES: usize = 64 * 1024;
const MAX_PLANES: usize = MAX_DEVICE_CHANNELS as usize;
const POLL: Duration = Duration::from_millis(20);
const QUIESCE: Duration = Duration::from_millis(250);
const OWNER_STACK: usize = 2 * 1024 * 1024;
const BASE_LISTENERS: usize = 9;
// Conservatively reserve the application block capture/header payload. Apple's
// allocator and native audio internals cannot be accounted from this API.
const BLOCK_BYTES: usize = 256;
const CHANNEL_STORAGE_RESERVE: usize = 3 * 4096;
const MAX_RESERVATIONS: usize = 64;

#[derive(Debug, PartialEq)]
struct NativeStream {
    id: AudioObjectID,
    first_channel: u32,
    format: Format,
}
#[derive(Debug, PartialEq)]
pub(crate) struct Selection {
    id: AudioObjectID,
    uid: String,
    rate: u32,
    streams: Vec<NativeStream>,
    layout: Arc<OutputLayout>,
    input_streams: usize,
}
impl Selection {
    pub(crate) fn channels(&self) -> u16 {
        self.layout.native_layout().channels
    }
    pub(crate) fn rate(&self) -> u32 {
        self.rate
    }
    pub(crate) fn description(&self) -> String {
        self.streams
            .iter()
            .map(|stream| {
                let f = stream.format;
                format!(
                    "{}ch {}-bit {}{}",
                    f.channels,
                    f.bits,
                    if f.flags & crate::native_packet::FLOAT != 0 {
                        "float"
                    } else {
                        "PCM"
                    },
                    if f.flags & crate::native_packet::NON_INTERLEAVED != 0 {
                        " planar"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }
    pub(crate) fn storage_bytes(&self) -> usize {
        size_of::<Self>()
            + self.uid.capacity()
            + self.streams.capacity() * size_of::<NativeStream>()
            + self.layout.storage_bytes()
            + 2 * size_of::<usize>()
    }
    pub(crate) fn owner_bytes(&self) -> Result<usize, AudioError> {
        // One selection/layout, one timing allocation, one context containing
        // OutputRenderer's inline fields (its PCM/graph allocation is charged by
        // OutputRenderer), gate, block/listener captures, registry strings and the
        // dedicated stack. Temporary property/reinspection storage is bounded.
        let listeners = BASE_LISTENERS + self.streams.len() * 2;
        let values = [
            OWNER_STACK,
            self.storage_bytes(),
            size_of::<Owner>(),
            size_of::<Context>(),
            size_of::<Gate>(),
            size_of::<NativeOutputTiming>(),
            size_of::<NativeOutput>(),
            4 * size_of::<[usize; 2]>(),
            listeners * (size_of::<Listener>() + BLOCK_BYTES),
            BLOCK_BYTES,
            2 * self.uid.len() + size_of::<Reservation>() + size_of::<String>(),
            CHANNEL_STORAGE_RESERVE + size_of::<Mutex<Reservations>>(),
            4 * MAX_PROPERTY_BYTES,
            self.storage_bytes(),
        ];
        values
            .into_iter()
            .try_fold(0_usize, |n, v| n.checked_add(v))
            .ok_or_else(|| {
                AudioError::InvalidSession("native output memory estimate overflow".into())
            })
    }
}

pub(crate) fn select(id: &str, rate: u32) -> Result<Selection, AudioError> {
    let parsed: cpal::DeviceId = id
        .parse()
        .map_err(|_| AudioError::InvalidOutputId(id.into()))?;
    if parsed.host() != cpal::HostId::CoreAudio
        || parsed.id().is_empty()
        || parsed.id().contains('\0')
    {
        return Err(AudioError::InvalidOutputId(id.into()));
    }
    inspect(parsed.id(), rate)
        .map_err(|e| backend("native output format discovery", Some(id.into()), e))
}
fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}
fn status(code: i32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("CoreAudio OSStatus {code}"))
    }
}
fn property<T: Copy>(
    object: AudioObjectID,
    address: AudioObjectPropertyAddress,
) -> Result<T, String> {
    let mut value = MaybeUninit::<T>::uninit();
    let mut bytes = size_of::<T>() as u32;
    // Only native integer, floating, pointer and repr(C) data types are used.
    status(unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut bytes),
            NonNull::new(value.as_mut_ptr().cast()).unwrap(),
        )
    })?;
    if bytes as usize != size_of::<T>() {
        return Err("native property size changed".into());
    }
    Ok(unsafe { value.assume_init() })
}
fn property_bytes(
    object: AudioObjectID,
    address: AudioObjectPropertyAddress,
) -> Result<(Vec<u64>, usize), String> {
    let mut bytes = 0;
    status(unsafe {
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut bytes),
        )
    })?;
    if bytes as usize > MAX_PROPERTY_BYTES {
        return Err("native property exceeds bounded storage".into());
    }
    let capacity = bytes as usize;
    let mut data = vec![0_u64; capacity.div_ceil(8).max(1)];
    status(unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut bytes),
            NonNull::new(data.as_mut_ptr().cast()).unwrap(),
        )
    })?;
    if bytes as usize > capacity {
        return Err("native property grew during discovery".into());
    }
    Ok((data, bytes as usize))
}
fn objects(object: AudioObjectID, selector: u32, scope: u32) -> Result<Vec<AudioObjectID>, String> {
    let (data, bytes) = property_bytes(object, address(selector, scope))?;
    if !bytes.is_multiple_of(size_of::<AudioObjectID>()) {
        return Err("invalid native object list".into());
    }
    // Aligned owned storage covers the returned byte count, with no trailing ID.
    Ok(
        unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<AudioObjectID>(), bytes / 4) }
            .to_vec(),
    )
}
fn uid(object: AudioObjectID) -> Result<String, String> {
    let raw: *mut CFString = property(
        object,
        address(
            kAudioDevicePropertyDeviceUID,
            kAudioObjectPropertyScopeGlobal,
        ),
    )?;
    let raw = NonNull::new(raw).ok_or("native device has no UID")?;
    // AudioDevicePropertyDeviceUID returns a retained CFString to the caller.
    Ok(unsafe { CFRetained::from_raw(raw) }.to_string())
}

fn inspect(wanted: &str, rate: u32) -> Result<Selection, String> {
    let mut matching = None;
    for object in objects(
        kAudioObjectSystemObject as u32,
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    )? {
        if uid(object).is_ok_and(|candidate| candidate == wanted)
            && matching.replace(object).is_some()
        {
            return Err("duplicate native output UID".into());
        }
    }
    let id = matching.ok_or("selected native output is unavailable")?;
    let alive: u32 = property(
        id,
        address(
            kAudioDevicePropertyDeviceIsAlive,
            kAudioObjectPropertyScopeGlobal,
        ),
    )?;
    let nominal: f64 = property(
        id,
        address(
            kAudioDevicePropertyNominalSampleRate,
            kAudioObjectPropertyScopeGlobal,
        ),
    )?;
    if alive == 0 || nominal != f64::from(rate) {
        return Err("native output is not alive at the exact requested rate".into());
    }
    let ids = objects(
        id,
        kAudioDevicePropertyStreams,
        kAudioObjectPropertyScopeOutput,
    )?;
    if ids.is_empty() || ids.len() > MAX_PLANES {
        return Err("unsupported native output stream count".into());
    }
    let mut streams = Vec::with_capacity(ids.len());
    let mut next_channel = 1_u32;
    for stream in ids {
        let first_channel: u32 = property(
            stream,
            address(
                kAudioStreamPropertyStartingChannel,
                kAudioObjectPropertyScopeGlobal,
            ),
        )?;
        // Preserve native stream/buffer order. Sorting could silently associate
        // a format with a different buffer or per-IOProc stream-usage slot.
        if first_channel != next_channel {
            return Err("native output stream order is not contiguous".into());
        }
        let asbd: AudioStreamBasicDescription = property(
            stream,
            address(
                kAudioStreamPropertyVirtualFormat,
                kAudioObjectPropertyScopeGlobal,
            ),
        )?;
        next_channel = next_channel
            .checked_add(asbd.mChannelsPerFrame)
            .ok_or("native output channel count overflow")?;
        streams.push(NativeStream {
            id: stream,
            first_channel,
            format: Format {
                rate: asbd.mSampleRate,
                id: asbd.mFormatID,
                flags: asbd.mFormatFlags,
                packet_bytes: asbd.mBytesPerPacket,
                packet_frames: asbd.mFramesPerPacket,
                frame_bytes: asbd.mBytesPerFrame,
                channels: asbd.mChannelsPerFrame,
                bits: asbd.mBitsPerChannel,
            },
        });
    }
    let layout = OutputLayout::new(&streams.iter().map(|s| s.format).collect::<Vec<_>>(), rate)
        .map_err(|e| format!("unsupported native output PCM: {e:?}"))?;
    let (configuration, bytes) = property_bytes(
        id,
        address(
            kAudioDevicePropertyStreamConfiguration,
            kAudioObjectPropertyScopeOutput,
        ),
    )?;
    if bytes < offset_of!(AudioBufferList, mBuffers) {
        return Err("invalid native output buffer-list property".into());
    }
    let pointer = configuration.as_ptr().cast::<AudioBufferList>();
    let count = unsafe { (*pointer).mNumberBuffers as usize };
    if count != layout.native_layout().planes.len()
        || bytes < offset_of!(AudioBufferList, mBuffers) + count * size_of::<AudioBuffer>()
    {
        return Err("native output buffer configuration does not match stream formats".into());
    }
    let buffers = unsafe {
        std::slice::from_raw_parts(
            ptr::addr_of!((*pointer).mBuffers).cast::<AudioBuffer>(),
            count,
        )
    };
    if buffers
        .iter()
        .zip(&layout.native_layout().planes)
        .any(|(buffer, plane)| buffer.mNumberChannels != plane.channels)
    {
        return Err("native output buffer channels do not match stream formats".into());
    }
    let input_streams = objects(
        id,
        kAudioDevicePropertyStreams,
        kAudioObjectPropertyScopeInput,
    )?
    .len();
    if input_streams > MAX_PLANES {
        return Err("unsupported native input stream count".into());
    }
    Ok(Selection {
        id,
        uid: wanted.into(),
        rate,
        streams,
        layout: Arc::new(layout),
        input_streams,
    })
}

struct Reservation {
    uid: String,
    release: bool,
}
struct Reservations {
    slots: [Option<String>; MAX_RESERVATIONS],
}
impl Reservations {
    const fn new() -> Self {
        Self {
            slots: [const { None }; MAX_RESERVATIONS],
        }
    }
    fn insert(&mut self, uid: &str) -> Result<(), String> {
        if self.slots.iter().flatten().any(|entry| entry == uid) {
            return Err("selected output still has a live or unquiesced owner; wait for shutdown or restart the application".into());
        }
        let slot=self.slots.iter_mut().find(|slot|slot.is_none()).ok_or("native output owner capacity is exhausted; wait for shutdown or restart the application")?;
        *slot = Some(uid.into());
        Ok(())
    }
    fn remove(&mut self, uid: &str) {
        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.as_deref() == Some(uid))
        {
            *slot = None;
        }
    }
}
static RESERVED: Mutex<Reservations> = Mutex::new(Reservations::new());
impl Reservation {
    fn acquire(uid: &str) -> Result<Self, String> {
        let mut registry = RESERVED
            .lock()
            .map_err(|_| "native output registry unavailable")?;
        registry.insert(uid)?;
        Ok(Self {
            uid: uid.into(),
            release: true,
        })
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if self.release {
            if let Ok(mut registry) = RESERVED.lock() {
                registry.remove(&self.uid);
            }
        }
    }
}

struct Gate {
    permit: OutputPermit,
    live: AtomicBool,
    program: AtomicBool,
    started: AtomicBool,
    start_progress: AtomicU64,
}
impl Gate {
    fn authorized(&self) -> bool {
        self.live.load(Ordering::Acquire) && self.permit.authorized()
    }
    fn revoke(&self) {
        self.program.store(false, Ordering::Release);
        self.live.store(false, Ordering::Release);
    }
    fn progressed(&self, timing: &NativeOutputTiming) -> bool {
        self.started.load(Ordering::Acquire)
            && timing.progress_count() > self.start_progress.load(Ordering::Acquire)
    }
    fn activate(&self, timing: &NativeOutputTiming) -> Result<(), AudioError> {
        if !self.authorized() || !self.progressed(timing) {
            return Err(AudioError::Cancelled);
        }
        self.program.store(true, Ordering::Release);
        if !self.authorized() {
            self.program.store(false, Ordering::Release);
            return Err(AudioError::Cancelled);
        }
        Ok(())
    }
}
type Reply = mpsc::SyncSender<Result<(), AudioError>>;
pub(crate) struct NativeOutput {
    commands: mpsc::SyncSender<Reply>,
    gate: Arc<Gate>,
    timing: Arc<NativeOutputTiming>,
}
impl NativeOutput {
    pub(crate) fn build(
        selection: Selection,
        render: OutputRenderer,
        timing: Arc<NativeOutputTiming>,
    ) -> Result<Self, AudioError> {
        // Reserve before creating a stack/owner, so pending starts and retained
        // failed owners share one finite process-wide capacity.
        let reservation = Reservation::acquire(&selection.uid)
            .map_err(|e| backend("native output reservation", None, e))?;
        let gate = Arc::new(Gate {
            permit: render.permit(),
            live: AtomicBool::new(true),
            program: AtomicBool::new(false),
            started: AtomicBool::new(false),
            start_progress: AtomicU64::new(0),
        });
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (commands, receiver) = mpsc::sync_channel(1);
        let owner_gate = gate.clone();
        let owner_timing = timing.clone();
        thread::Builder::new()
            .name("relink-coreaudio-output".into())
            .stack_size(OWNER_STACK)
            .spawn(move || {
                owner_loop(
                    selection,
                    render,
                    owner_gate,
                    owner_timing,
                    receiver,
                    ready_tx,
                    reservation,
                )
            })
            .map_err(|e| backend("native output owner creation", None, e))?;
        let handle = Self {
            commands,
            gate,
            timing,
        };
        match ready_rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(Ok(())) => Ok(handle),
            Ok(Err(error)) => Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(AudioError::Timeout {
                operation: "native output creation",
            }),
            Err(_) => Err(AudioError::RuntimeUnavailable),
        }
    }
    pub(crate) fn start(&self) -> Result<(), AudioError> {
        if !self.gate.authorized() {
            return Err(AudioError::Cancelled);
        }
        let (reply, rx) = mpsc::sync_channel(1);
        if self.commands.try_send(reply).is_err() {
            self.cancel();
            return Err(AudioError::RuntimeUnavailable);
        }
        match rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(Ok(())) if self.gate.authorized() => Ok(()),
            Ok(Ok(())) => Err(AudioError::Cancelled),
            Ok(Err(error)) => {
                self.cancel();
                Err(error)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.cancel();
                Err(AudioError::Timeout {
                    operation: "native output activation",
                })
            }
            Err(_) => {
                self.cancel();
                Err(AudioError::RuntimeUnavailable)
            }
        }
    }
    pub(crate) fn wait_for_progress(&self, timeout: Duration) -> Result<(), AudioError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(AudioError::Cancelled)?;
        loop {
            if !self.gate.authorized() {
                self.cancel();
                return Err(AudioError::Cancelled);
            }
            if self.gate.progressed(&self.timing) {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                self.gate.permit.fail_at(
                    AudioFault::Backend,
                    FaultOrigin::NativeOutputStartup,
                    self.gate.permit.metadata(),
                );
                self.cancel();
                return Err(AudioError::Timeout {
                    operation: "native output callback progress",
                });
            }
            thread::sleep(POLL.min(deadline - now));
        }
    }
    pub(crate) fn activate(&self) -> Result<(), AudioError> {
        self.gate.activate(&self.timing)
    }
    pub(crate) fn cancel(&self) {
        self.gate.revoke();
        self.timing.lifecycle(NativeOutputLifecycleEvent {
            phase: NativeOutputLifecyclePhase::HandleCancelled,
            host_time: host_time(),
            status: None,
            callbacks_in_flight: None,
        });
    }
}
impl Drop for NativeOutput {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn host_time() -> u64 {
    unsafe { AudioGetCurrentHostTime() }
}

fn owner_loop(
    selection: Selection,
    render: OutputRenderer,
    gate: Arc<Gate>,
    timing: Arc<NativeOutputTiming>,
    commands: mpsc::Receiver<Reply>,
    ready: Reply,
    reservation: Reservation,
) {
    if !gate.authorized() {
        let _ = ready.send(Err(AudioError::Cancelled));
        return;
    }
    let mut owner = match Owner::open(selection, render, gate.clone(), timing.clone(), reservation)
    {
        Ok(owner) => owner,
        Err(error) => {
            let _ = ready.send(Err(backend("native output creation", None, error)));
            return;
        }
    };
    if !gate.authorized() {
        let _ = ready.send(Err(AudioError::Cancelled));
        return;
    }
    owner.mark(NativeOutputLifecyclePhase::Ready, None);
    if ready.send(Ok(())).is_err() {
        return;
    }
    while gate.authorized() {
        match commands.recv_timeout(POLL) {
            Ok(reply) => {
                let result = owner
                    .start()
                    .map_err(|e| backend("native output activation", None, e));
                let success = result.is_ok();
                if !gate.authorized() {
                    let _ = reply.send(Err(AudioError::Cancelled));
                    return;
                }
                if reply.send(result).is_err() || !success {
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return,
        }
    }
}

struct PacketState {
    render: OutputRenderer,
    tracker: Tracker,
}
struct Context {
    object: AudioObjectID,
    layout: Arc<OutputLayout>,
    gate: Arc<Gate>,
    timing: Arc<NativeOutputTiming>,
    in_flight: AtomicUsize,
    entered: AtomicBool,
    state: UnsafeCell<PacketState>,
}
// Only the IOProc holding entered may borrow state mutably. OS-copied blocks
// retain the Arc even before in_flight admission. All other access is atomic.
unsafe impl Sync for Context {}
struct Flight<'a>(&'a Context);
impl Drop for Flight<'_> {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::Release);
    }
}
struct Entered<'a>(&'a AtomicBool);
impl Drop for Entered<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
fn stamp(value: AudioTimeStamp) -> NativeOutputStamp {
    NativeOutputStamp {
        sample_time: value.mSampleTime,
        host_time: value.mHostTime,
        rate_scalar: value.mRateScalar,
        flags: value.mFlags.0,
    }
}
fn report_packet_fault(
    context: &Context,
    packet: NativeOutputPacket,
    previous: Option<NativeOutputPacket>,
    fault: AudioFault,
    reason: NativeOutputFaultReason,
    origin: FaultOrigin,
) {
    context.timing.reject(NativeOutputFault {
        fault,
        reason,
        previous,
        current: packet,
    });
    context.gate.permit.fail_at(
        fault,
        origin,
        FaultMetadata {
            native_object: Some(context.object),
            packet_index: Some(packet.callback_index),
            observed_mach_host_time: Some(host_time()),
            ..context.gate.permit.metadata()
        },
    );
}

/// Native descriptor/range check, before forming *any* mutable PCM slice. The
/// SDK owns descriptor/readable allocation validity; this check enforces our
/// bounds and rejects overlaps among planes or with the native descriptor list.
fn validate_ranges(
    layout: &OutputLayout,
    buffers: &[AudioBuffer],
    forbidden: &[(usize, usize)],
) -> Result<usize, AudioFault> {
    let frames = layout
        .native_layout()
        .validate_sizes(buffers.iter().map(|b| {
            (
                b.mNumberChannels,
                b.mDataByteSize as usize,
                !b.mData.is_null(),
            )
        }))?;
    for (index, buffer) in buffers.iter().enumerate() {
        let start = buffer.mData as usize;
        let bytes = buffer.mDataByteSize as usize;
        if bytes > isize::MAX as usize {
            return Err(AudioFault::InvalidBuffer);
        }
        let end = start.checked_add(bytes).ok_or(AudioFault::InvalidBuffer)?;
        for &(reserved_start, reserved_bytes) in forbidden {
            let reserved_end = reserved_start
                .checked_add(reserved_bytes)
                .ok_or(AudioFault::InvalidBuffer)?;
            if start < reserved_end && reserved_start < end {
                return Err(AudioFault::InvalidBuffer);
            }
        }
        for previous in &buffers[..index] {
            let previous_start = previous.mData as usize;
            let previous_end = previous_start
                .checked_add(previous.mDataByteSize as usize)
                .ok_or(AudioFault::InvalidBuffer)?;
            if start < previous_end && previous_start < end {
                return Err(AudioFault::InvalidBuffer);
            }
        }
    }
    Ok(frames)
}

unsafe fn io_proc(
    context: &Context,
    now: NonNull<AudioTimeStamp>,
    output: NonNull<AudioBufferList>,
    presentation: NonNull<AudioTimeStamp>,
) {
    let entry = host_time();
    context.in_flight.fetch_add(1, Ordering::AcqRel);
    let _flight = Flight(context);
    let index = context.timing.enter();
    let mut packet = NativeOutputPacket {
        callback_index: index,
        epoch: context.gate.permit.epoch(),
        frames: 0,
        entry_host_time: entry,
        now: stamp(unsafe { now.as_ptr().read() }),
        presentation: stamp(unsafe { presentation.as_ptr().read() }),
    };
    if context
        .entered
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        // A broken host could have supplied the same PCM pointer to both
        // invocations. Never make a second mutable borrow or clear underneath
        // the first writer. Revocation makes that writer clear at its final gate.
        report_packet_fault(
            context,
            packet,
            context.timing.last_packet(),
            AudioFault::Xrun,
            NativeOutputFaultReason::Reentry,
            FaultOrigin::NativeOutputReentry,
        );
        return;
    }
    let _entered = Entered(&context.entered);
    let state = unsafe { &mut *context.state.get() };
    let count = unsafe { ptr::addr_of!((*output.as_ptr()).mNumberBuffers).read() as usize };
    let result = if count != context.layout.native_layout().planes.len() || count > MAX_PLANES {
        Err((
            AudioFault::InvalidBuffer,
            NativeOutputFaultReason::InvalidBuffer,
        ))
    } else {
        let mut descriptors: [AudioBuffer; MAX_PLANES] = std::array::from_fn(|_| AudioBuffer {
            mNumberChannels: 0,
            mDataByteSize: 0,
            mData: ptr::null_mut(),
        });
        let first = unsafe { ptr::addr_of!((*output.as_ptr()).mBuffers).cast::<AudioBuffer>() };
        for (i, slot) in descriptors[..count].iter_mut().enumerate() {
            *slot = unsafe { first.add(i).read() };
        }
        let native_extent =
            offset_of!(AudioBufferList, mBuffers) + count * size_of::<AudioBuffer>();
        match validate_ranges(
            &context.layout,
            &descriptors[..count],
            &[
                (output.as_ptr() as usize, native_extent),
                (now.as_ptr() as usize, size_of::<AudioTimeStamp>()),
                (presentation.as_ptr() as usize, size_of::<AudioTimeStamp>()),
            ],
        ) {
            Err(fault) => Err((fault, NativeOutputFaultReason::InvalidBuffer)),
            Ok(frames) => {
                packet.frames = frames as u32;
                // Every enabled range is present, bounded and disjoint. No
                // native pointer/descriptor is retained beyond this callback.
                let mut planes: [PlaneBuffer<'_>; MAX_PLANES] =
                    std::array::from_fn(|_| PlaneBuffer {
                        channels: 0,
                        bytes: &mut [],
                    });
                for (target, descriptor) in planes[..count].iter_mut().zip(&descriptors[..count]) {
                    target.channels = descriptor.mNumberChannels;
                    target.bytes = unsafe {
                        std::slice::from_raw_parts_mut(
                            descriptor.mData.cast::<u8>(),
                            descriptor.mDataByteSize as usize,
                        )
                    };
                }
                let mut sink = context
                    .layout
                    .prepare(&mut planes[..count])
                    .expect("prevalidated native output extents");
                if !context.gate.authorized() {
                    Ok(NativeOutputOutcome::Cancelled)
                } else if index == 0 {
                    Err((
                        AudioFault::ResourceExhausted,
                        NativeOutputFaultReason::CounterExhausted,
                    ))
                } else if let Err(fault) = state.tracker.check(packet) {
                    Err(fault)
                } else if !context.gate.program.load(Ordering::Acquire)
                    || !context.gate.permit.program_authorized()
                {
                    // Preparation already cleared every channel to native
                    // equilibrium, including unsigned PCM during startup.
                    Ok(NativeOutputOutcome::InactiveSilence)
                } else {
                    match state.render.write_to(&mut sink) {
                        RenderOutcome::Silence => Ok(if context.gate.authorized() {
                            NativeOutputOutcome::PrimingSilence
                        } else {
                            NativeOutputOutcome::Cancelled
                        }),
                        RenderOutcome::Rendered {
                            accepted_version, ..
                        } => match sink.complete() {
                            Err(_) => Err((
                                AudioFault::InvalidBuffer,
                                NativeOutputFaultReason::InvalidBuffer,
                            )),
                            Ok(completed) => {
                                if context.gate.authorized()
                                    && context.gate.program.load(Ordering::Acquire)
                                    && context.gate.permit.program_authorized()
                                {
                                    completed.handoff();
                                    context.gate.permit.submitted(frames, accepted_version);
                                    Ok(NativeOutputOutcome::Encoded { accepted_version })
                                } else {
                                    completed.cancel();
                                    Ok(NativeOutputOutcome::Cancelled)
                                }
                            }
                        },
                    }
                }
                // An incomplete/default-dropped sink clears all valid planes.
            }
        }
    };
    let after_render = host_time();
    let outcome = match result {
        Ok(outcome) => outcome,
        Err((fault, reason)) => {
            report_packet_fault(
                context,
                packet,
                state.tracker.previous,
                fault,
                reason,
                FaultOrigin::NativeOutputPacket,
            );
            NativeOutputOutcome::Rejected(fault)
        }
    };
    let progress = matches!(
        outcome,
        NativeOutputOutcome::InactiveSilence
            | NativeOutputOutcome::PrimingSilence
            | NativeOutputOutcome::Encoded { .. }
    ) && context.gate.authorized();
    if progress {
        state.tracker.commit(packet);
        context.timing.accept(packet);
    }
    context.timing.complete(
        NativeOutputCompletion {
            packet,
            after_render_host_time: after_render,
            completion_host_time: host_time(),
            outcome,
        },
        progress,
    );
}

type OutputBlock = RcBlock<
    dyn Fn(
        NonNull<AudioTimeStamp>,
        NonNull<AudioBufferList>,
        NonNull<AudioTimeStamp>,
        NonNull<AudioBufferList>,
        NonNull<AudioTimeStamp>,
    ),
>;
fn output_block(context: Arc<Context>) -> OutputBlock {
    RcBlock::new(
        move |now, _input, _input_time, output, presentation| unsafe {
            io_proc(&context, now, output, presentation)
        },
    )
}
type ListenerBlock = RcBlock<dyn Fn(u32, NonNull<AudioObjectPropertyAddress>)>;
fn listener_block(
    gate: Arc<Gate>,
    object: AudioObjectID,
    wanted: AudioObjectPropertyAddress,
    fault: AudioFault,
) -> ListenerBlock {
    RcBlock::new(
        move |count: u32, changed: NonNull<AudioObjectPropertyAddress>| {
            if !gate.authorized() {
                return;
            }
            if count as usize > MAX_PROPERTY_BYTES / size_of::<AudioObjectPropertyAddress>() {
                gate.permit.fail_at(
                    AudioFault::InvalidBuffer,
                    FaultOrigin::NativeOutputProperty,
                    FaultMetadata {
                        native_object: Some(object),
                        ..gate.permit.metadata()
                    },
                );
                return;
            }
            let changed = unsafe { std::slice::from_raw_parts(changed.as_ptr(), count as usize) };
            if let Some(property) = changed.iter().find(|p| {
                p.mSelector == wanted.mSelector
                    && p.mScope == wanted.mScope
                    && p.mElement == wanted.mElement
            }) {
                gate.permit.fail_at(
                    fault,
                    if property.mSelector == kAudioDeviceProcessorOverload {
                        FaultOrigin::NativeOutputOverload
                    } else {
                        FaultOrigin::NativeOutputProperty
                    },
                    FaultMetadata {
                        native_object: Some(object),
                        property: Some(FaultProperty {
                            selector: property.mSelector,
                            scope: property.mScope,
                            element: property.mElement,
                        }),
                        ..gate.permit.metadata()
                    },
                );
            }
        },
    )
}
struct Listener {
    object: AudioObjectID,
    address: AudioObjectPropertyAddress,
    block: ListenerBlock,
}
struct Owner {
    selection: Selection,
    context: Option<Arc<Context>>,
    callback: Option<OutputBlock>,
    gate: Arc<Gate>,
    timing: Arc<NativeOutputTiming>,
    proc_id: AudioDeviceIOProcID,
    start_attempted: bool,
    listeners: Vec<Listener>,
    reservation: Option<Reservation>,
}
impl Owner {
    fn mark(&self, phase: NativeOutputLifecyclePhase, status: Option<i32>) {
        self.timing.lifecycle(NativeOutputLifecycleEvent {
            phase,
            host_time: host_time(),
            status,
            callbacks_in_flight: Some(
                self.context
                    .as_ref()
                    .map_or(0, |c| c.in_flight.load(Ordering::Acquire) as u64),
            ),
        });
    }
    fn open(
        selection: Selection,
        render: OutputRenderer,
        gate: Arc<Gate>,
        timing: Arc<NativeOutputTiming>,
        reservation: Reservation,
    ) -> Result<Self, String> {
        timing.lifecycle(NativeOutputLifecycleEvent {
            phase: NativeOutputLifecyclePhase::ReservationAcquired,
            host_time: host_time(),
            status: None,
            callbacks_in_flight: Some(0),
        });
        if inspect(&selection.uid, selection.rate)? != selection {
            return Err("selected output changed since discovery".into());
        }
        let context = Arc::new(Context {
            object: selection.id,
            layout: selection.layout.clone(),
            gate: gate.clone(),
            timing: timing.clone(),
            in_flight: AtomicUsize::new(0),
            entered: AtomicBool::new(false),
            state: UnsafeCell::new(PacketState {
                render,
                tracker: Tracker::default(),
            }),
        });
        let callback = output_block(context.clone());
        let listeners = Vec::with_capacity(BASE_LISTENERS + selection.streams.len() * 2);
        let mut owner = Self {
            selection,
            context: Some(context),
            callback: Some(callback),
            gate,
            timing,
            proc_id: None,
            start_attempted: false,
            listeners,
            reservation: Some(reservation),
        };
        owner.mark(NativeOutputLifecyclePhase::CreateRequested, None);
        let created = unsafe {
            AudioDeviceCreateIOProcIDWithBlock(
                NonNull::from(&mut owner.proc_id),
                owner.selection.id,
                None,
                RcBlock::as_ptr(owner.callback.as_ref().unwrap()),
            )
        };
        owner.mark(NativeOutputLifecyclePhase::CreateReturned, Some(created));
        owner.check_native(created, None)?;
        if owner.proc_id.is_none() {
            return Err("CoreAudio returned no output IOProc".into());
        }
        owner.set_usage(
            kAudioObjectPropertyScopeInput,
            owner.selection.input_streams,
            false,
        )?;
        owner.set_usage(
            kAudioObjectPropertyScopeOutput,
            owner.selection.streams.len(),
            true,
        )?;
        for (selector, scope, fault) in [
            (
                kAudioDevicePropertyDeviceIsAlive,
                kAudioObjectPropertyScopeGlobal,
                AudioFault::DeviceUnavailable,
            ),
            (
                kAudioDevicePropertyDeviceUID,
                kAudioObjectPropertyScopeGlobal,
                AudioFault::DeviceChanged,
            ),
            (
                kAudioDevicePropertyNominalSampleRate,
                kAudioObjectPropertyScopeGlobal,
                AudioFault::StreamInvalidated,
            ),
            (
                kAudioDevicePropertyDeviceHasChanged,
                kAudioObjectPropertyScopeGlobal,
                AudioFault::DeviceChanged,
            ),
            (
                kAudioDevicePropertyStreams,
                kAudioObjectPropertyScopeOutput,
                AudioFault::StreamInvalidated,
            ),
            (
                kAudioDevicePropertyStreams,
                kAudioObjectPropertyScopeInput,
                AudioFault::StreamInvalidated,
            ),
            (
                kAudioDevicePropertyStreamConfiguration,
                kAudioObjectPropertyScopeOutput,
                AudioFault::StreamInvalidated,
            ),
            (
                kAudioDevicePropertyIOStoppedAbnormally,
                kAudioObjectPropertyScopeGlobal,
                AudioFault::StreamInvalidated,
            ),
            (
                kAudioDeviceProcessorOverload,
                kAudioObjectPropertyScopeGlobal,
                AudioFault::Xrun,
            ),
        ] {
            owner.listen(owner.selection.id, address(selector, scope), fault)?;
        }
        for i in 0..owner.selection.streams.len() {
            let stream = owner.selection.streams[i].id;
            for selector in [
                kAudioStreamPropertyVirtualFormat,
                kAudioStreamPropertyStartingChannel,
            ] {
                owner.listen(
                    stream,
                    address(selector, kAudioObjectPropertyScopeGlobal),
                    AudioFault::StreamInvalidated,
                )?;
            }
        }
        if inspect(&owner.selection.uid, owner.selection.rate)? != owner.selection
            || !owner.gate.authorized()
        {
            return Err("selected output changed during setup".into());
        }
        Ok(owner)
    }
    fn check_native(
        &self,
        code: i32,
        property: Option<AudioObjectPropertyAddress>,
    ) -> Result<(), String> {
        if code != 0 {
            self.gate.permit.fail_at(
                AudioFault::Backend,
                FaultOrigin::NativeOutputLifecycle,
                FaultMetadata {
                    native_object: Some(self.selection.id),
                    status: Some(code),
                    property: property.map(|p| FaultProperty {
                        selector: p.mSelector,
                        scope: p.mScope,
                        element: p.mElement,
                    }),
                    ..self.gate.permit.metadata()
                },
            );
        }
        status(code)
    }
    fn listen(
        &mut self,
        object: AudioObjectID,
        property: AudioObjectPropertyAddress,
        fault: AudioFault,
    ) -> Result<(), String> {
        let block = listener_block(self.gate.clone(), object, property, fault);
        let code = unsafe {
            AudioObjectAddPropertyListenerBlock(
                object,
                NonNull::from(&property),
                None,
                RcBlock::as_ptr(&block),
            )
        };
        if code != 0 {
            self.gate.permit.fail_at(
                AudioFault::Backend,
                FaultOrigin::NativeOutputLifecycle,
                FaultMetadata {
                    native_object: Some(object),
                    status: Some(code),
                    property: Some(FaultProperty {
                        selector: property.mSelector,
                        scope: property.mScope,
                        element: property.mElement,
                    }),
                    ..self.gate.permit.metadata()
                },
            );
            return status(code);
        }
        self.listeners.push(Listener {
            object,
            address: property,
            block,
        });
        Ok(())
    }
    fn set_usage(&self, scope: u32, streams: usize, enabled: bool) -> Result<(), String> {
        if streams == 0 {
            return Ok(());
        }
        let size =
            offset_of!(AudioHardwareIOProcStreamUsage, mStreamIsOn) + streams * size_of::<u32>();
        let mut storage = vec![0_u64; size.div_ceil(8)];
        let usage = storage
            .as_mut_ptr()
            .cast::<AudioHardwareIOProcStreamUsage>();
        unsafe {
            (*usage).mIOProc =
                NonNull::new(self.proc_id.unwrap() as *const () as *mut c_void).unwrap();
            (*usage).mNumberStreams = streams as u32;
            let on = ptr::addr_of_mut!((*usage).mStreamIsOn).cast::<u32>();
            for i in 0..streams {
                on.add(i).write(u32::from(enabled));
            }
        }
        let property = address(kAudioDevicePropertyIOProcStreamUsage, scope);
        let code = unsafe {
            AudioObjectSetPropertyData(
                self.selection.id,
                NonNull::from(&property),
                0,
                ptr::null(),
                size as u32,
                NonNull::new(storage.as_mut_ptr().cast()).unwrap(),
            )
        };
        self.check_native(code, Some(property))
    }
    fn start(&mut self) -> Result<(), String> {
        if self.gate.started.load(Ordering::Acquire) {
            return Ok(());
        }
        if !self.gate.authorized() {
            return Err("output activation cancelled".into());
        }
        if inspect(&self.selection.uid, self.selection.rate)? != self.selection {
            return Err("native output changed before activation".into());
        }
        self.start_attempted = true;
        self.mark(NativeOutputLifecyclePhase::StartRequested, None);
        let code = unsafe { AudioDeviceStart(self.selection.id, self.proc_id) };
        self.mark(NativeOutputLifecyclePhase::StartReturned, Some(code));
        self.check_native(code, None)?;
        acknowledge_start(&self.gate, &self.timing)
    }
}

fn acknowledge_start(gate: &Gate, timing: &NativeOutputTiming) -> Result<(), String> {
    // Only called after native Start returns success. Earlier callbacks do not
    // fulfill the post-start progress requirement or enable the program gate.
    gate.start_progress
        .store(timing.progress_count(), Ordering::Release);
    if !gate.authorized() {
        return Err("output start returned after cancellation".into());
    }
    gate.started.store(true, Ordering::Release);
    if !gate.authorized() {
        return Err("output start cancelled".into());
    }
    Ok(())
}

trait CloseOps {
    fn mark(&self, phase: NativeOutputLifecyclePhase, status: Option<i32>);
    fn stop(&mut self) -> i32;
    fn destroy(&mut self) -> i32;
    fn remove(&mut self, index: usize) -> i32;
}
#[derive(Debug, PartialEq, Eq)]
struct CloseResult {
    quiescent: bool,
    first_failure: Option<(i32, Option<usize>)>,
}
fn close_native(
    ops: &mut impl CloseOps,
    started: bool,
    created: bool,
    listeners: usize,
) -> CloseResult {
    let mut result = CloseResult {
        quiescent: true,
        first_failure: None,
    };
    for (requested, returned, needed) in [
        (
            NativeOutputLifecyclePhase::StopRequested,
            NativeOutputLifecyclePhase::StopReturned,
            started,
        ),
        (
            NativeOutputLifecyclePhase::DestroyRequested,
            NativeOutputLifecyclePhase::DestroyReturned,
            created,
        ),
    ] {
        if needed {
            ops.mark(requested, None);
            let code = if requested == NativeOutputLifecyclePhase::StopRequested {
                ops.stop()
            } else {
                ops.destroy()
            };
            ops.mark(returned, Some(code));
            if code != 0 {
                result.quiescent = false;
                result.first_failure.get_or_insert((code, None));
            }
        }
    }
    ops.mark(NativeOutputLifecyclePhase::ListenerRemovalRequested, None);
    let mut removal = 0;
    for index in 0..listeners {
        let code = ops.remove(index);
        if code != 0 {
            result.quiescent = false;
            if removal == 0 {
                removal = code;
            }
            result.first_failure.get_or_insert((code, Some(index)));
        }
    }
    ops.mark(
        NativeOutputLifecyclePhase::ListenerRemovalReturned,
        Some(removal),
    );
    result
}
struct NativeClose<'a>(&'a Owner);
impl CloseOps for NativeClose<'_> {
    fn mark(&self, phase: NativeOutputLifecyclePhase, status: Option<i32>) {
        self.0.mark(phase, status);
    }
    fn stop(&mut self) -> i32 {
        unsafe { AudioDeviceStop(self.0.selection.id, self.0.proc_id) }
    }
    fn destroy(&mut self) -> i32 {
        unsafe { AudioDeviceDestroyIOProcID(self.0.selection.id, self.0.proc_id) }
    }
    fn remove(&mut self, index: usize) -> i32 {
        let listener = &self.0.listeners[index];
        unsafe {
            AudioObjectRemovePropertyListenerBlock(
                listener.object,
                NonNull::from(&listener.address),
                None,
                RcBlock::as_ptr(&listener.block),
            )
        }
    }
}
fn drained(context: &Context, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while context.in_flight.load(Ordering::Acquire) != 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    context.in_flight.load(Ordering::Acquire) == 0
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.gate.revoke();
        self.mark(NativeOutputLifecyclePhase::GatesRevoked, None);
        let result = close_native(
            &mut NativeClose(self),
            self.start_attempted,
            self.proc_id.is_some(),
            self.listeners.len(),
        );
        self.finish_teardown(result, QUIESCE);
    }
}
impl Owner {
    fn finish_teardown(&mut self, result: CloseResult, timeout: Duration) {
        let callbacks_drained = self
            .context
            .as_ref()
            .is_none_or(|context| drained(context, timeout));
        let quiescent = result.quiescent && callbacks_drained;
        self.mark(NativeOutputLifecyclePhase::QuiescenceChecked, None);
        if quiescent {
            // Completion is after all native block captures/context and the UID
            // reservation have actually been released, not just after Stop.
            self.proc_id = None;
            self.start_attempted = false;
            self.listeners.clear();
            drop(self.callback.take());
            drop(self.context.take());
            drop(self.reservation.take());
            self.mark(NativeOutputLifecyclePhase::OwnerCompleted, None);
        } else {
            if let Some(reservation) = &mut self.reservation {
                reservation.release = false;
            }
            self.mark(NativeOutputLifecyclePhase::ContextRetained, None);
            let mut metadata = FaultMetadata {
                native_object: Some(self.selection.id),
                ..self.gate.permit.metadata()
            };
            if let Some((status, index)) = result.first_failure {
                metadata.status = Some(status);
                if let Some(listener) = index.and_then(|i| self.listeners.get(i)) {
                    metadata.native_object = Some(listener.object);
                    metadata.property = Some(FaultProperty {
                        selector: listener.address.mSelector,
                        scope: listener.address.mScope,
                        element: listener.address.mElement,
                    });
                }
            }
            self.gate.permit.fail_at(
                AudioFault::Backend,
                FaultOrigin::NativeOutputLifecycle,
                metadata,
            );
            // Preserve both our ownership and the OS-copied ownership. A failed
            // remove/destroy need not promise that even a future entry is absent.
            if let Some(context) = self.context.take() {
                std::mem::forget(context);
            }
            if let Some(callback) = self.callback.take() {
                std::mem::forget(callback);
            }
            std::mem::forget(std::mem::take(&mut self.listeners));
        }
    }
}

#[cfg(test)]
#[path = "native_output_tests.rs"]
mod tests;
