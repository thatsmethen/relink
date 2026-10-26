//! Original PortCls adapter and polled WaveRT transport. All resident state is
//! nonpaged. PortCls owns IRP dispatch; no private worker/timer can outlive PnP.
use crate::{abi::*, descriptors, transport::*};
use core::{
    cell::UnsafeCell,
    ffi::c_void,
    mem::size_of,
    ptr,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

const OK: NtStatus = 0;
const INVALID: NtStatus = 0xc000_000du32 as i32;
const NO_INTERFACE: NtStatus = 0xc000_02b9u32 as i32;
const NO_MEMORY: NtStatus = 0xc000_009au32 as i32;
const NOT_SUPPORTED: NtStatus = 0xc000_00bbu32 as i32;
const BAD_STATE: NtStatus = 0xc000_0184u32 as i32;
const TAG: u32 = u32::from_le_bytes(*b"Rvln");
const NONPAGED: u64 = 0x40;

extern "system" {
    fn PcInitializeAdapterDriver(
        driver: *mut DriverObject,
        registry: *mut UnicodeString,
        add: unsafe extern "system" fn(*mut DriverObject, *mut DeviceObject) -> NtStatus,
    ) -> NtStatus;
    fn PcAddAdapterDevice(
        driver: *mut DriverObject,
        physical: *mut DeviceObject,
        start: unsafe extern "system" fn(
            *mut DeviceObject,
            *mut Irp,
            *mut ResourceList,
        ) -> NtStatus,
        max_objects: u32,
        extension_bytes: u32,
    ) -> NtStatus;
    fn PcDispatchIrp(device: *mut DeviceObject, irp: *mut Irp) -> NtStatus;
    fn PcNewPort(port: *mut *mut Port, class: *const Guid) -> NtStatus;
    fn PcRegisterSubdevice(
        device: *mut DeviceObject,
        name: *const u16,
        port: *mut Unknown,
    ) -> NtStatus;
    fn PcRegisterPhysicalConnection(
        device: *mut DeviceObject,
        from: *mut Unknown,
        from_pin: u32,
        to: *mut Unknown,
        to_pin: u32,
    ) -> NtStatus;
    fn PcRegisterAdapterPowerManagement(adapter: *mut Unknown, device: Raw) -> NtStatus;
    fn PcUnregisterAdapterPowerManagement(device: *mut DeviceObject) -> NtStatus;
    fn PcRegisterAdapterPnpManagement(adapter: *mut Unknown, device: *mut DeviceObject)
        -> NtStatus;
    fn PcUnregisterAdapterPnpManagement(device: *mut DeviceObject) -> NtStatus;
    fn ExAllocatePool2(flags: u64, bytes: usize, tag: u32) -> Raw;
    fn ExFreePoolWithTag(pointer: Raw, tag: u32);
    fn KeAcquireSpinLockRaiseToDpc(lock: *mut usize) -> u8;
    fn KeReleaseSpinLock(lock: *mut usize, old_irql: u8);
    fn KeInitializeEvent(event: *mut KernelEvent, kind: i32, signaled: u8);
    fn KeClearEvent(event: *mut KernelEvent);
    fn KeSetEvent(event: *mut KernelEvent, increment: i32, wait: u8) -> i32;
    fn KeEnterCriticalRegion();
    fn KeLeaveCriticalRegion();
    fn KeGetCurrentIrql() -> u8;
    fn KeWaitForSingleObject(
        object: Raw,
        reason: i32,
        mode: i8,
        alertable: u8,
        timeout: *const PhysicalAddress,
    ) -> NtStatus;
    fn KeQueryPerformanceCounter(frequency: *mut PhysicalAddress) -> PhysicalAddress;
    #[cfg(not(test))]
    fn KeBugCheckEx(code: u32, a: usize, b: usize, c: usize, d: usize) -> !;
}
// MSVC object metadata references this marker even when only integer code is
// reachable. It has no runtime behavior; core is rebuilt with panic=abort.
#[cfg(not(test))]
#[no_mangle]
pub static _fltused: i32 = 0;

#[cfg(not(test))]
pub unsafe fn bugcheck() -> ! {
    KeBugCheckEx(0xe2, TAG as usize, 0, 0, 0)
}

struct Shared<T>(T);
unsafe impl<T> Sync for Shared<T> {}
#[derive(Clone, Copy)]
struct Frame {
    time_plus_one: u64,
    pair: u64,
}
#[derive(Clone, Copy)]
struct Slot {
    active: bool,
    allocating: bool,
    resume_after_power: bool,
    timeline: Timeline,
    processed: u64,
    global_base: u64,
    mdl: *mut Mdl,
    buffer: *mut u64,
    bytes: u32,
}
struct CableState {
    frequency: u64,
    epoch: u64,
    powered: bool,
    stopping: bool,
    slots: [Slot; 2],
    copying: bool,
    control_pending: bool,
    ports: [*mut Port; 2],
    subdevices: u8,
    connections: u8,
}
#[repr(C)]
struct PnpPart {
    vtable: *const AdapterPnpManagementVtable,
    owner: *mut Cable,
}
#[repr(C)]
struct Cable {
    vtable: *const AdapterPowerManagementVtable,
    refs: AtomicU32,
    power_registered: AtomicBool,
    pnp_registered: AtomicBool,
    teardown_started: AtomicBool,
    pnp: PnpPart,
    device: *mut DeviceObject,
    lock: UnsafeCell<usize>,
    state: UnsafeCell<CableState>,
    // Audio storage has separate aliasing/lifetime from metadata. Only the
    // current copy owner accesses it; metadata readers never borrow this array.
    frames: UnsafeCell<[Frame; CABLE_FRAMES]>,
    control_gate: UnsafeCell<KernelEvent>,
    copy_idle: UnsafeCell<KernelEvent>,
}
#[repr(C)]
struct Miniport {
    vtable: *const c_void,
    refs: AtomicU32,
    cable: *mut Cable,
    topology: bool,
}
#[repr(C)]
struct Stream {
    vtable: *const MiniportWaveRtStreamVtable,
    refs: AtomicU32,
    cable: *mut Cable,
    port: *mut PortWaveRtStream,
    index: usize,
}
struct Guard {
    cable: *mut Cable,
    irql: u8,
}
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            KeReleaseSpinLock((*self.cable).lock.get(), self.irql);
        }
    }
}
impl Guard {
    unsafe fn state(&mut self) -> &mut CableState {
        &mut *(*self.cable).state.get()
    }
}
unsafe fn lock(cable: *mut Cable) -> Guard {
    Guard {
        cable,
        irql: KeAcquireSpinLockRaiseToDpc((*cable).lock.get()),
    }
}
unsafe fn now() -> u64 {
    KeQueryPerformanceCounter(ptr::null_mut()).quad_part as u64
}
fn global_frame(state: &CableState, ticks: u64) -> u64 {
    frames_from_ticks(ticks.saturating_sub(state.epoch), state.frequency)
}
unsafe fn unknown_add(pointer: Raw) {
    let object = pointer.cast::<Unknown>();
    ((*(*object).vtable).add_ref)(pointer);
}
unsafe fn unknown_release(pointer: Raw) {
    if !pointer.is_null() {
        let object = pointer.cast::<Unknown>();
        ((*(*object).vtable).release)(pointer);
    }
}
unsafe fn query(pointer: Raw, id: &Guid) -> Result<Raw, NtStatus> {
    let mut result = ptr::null_mut();
    let status = ((*(*pointer.cast::<Unknown>()).vtable).query_interface)(pointer, id, &mut result);
    if status < 0 {
        Err(status)
    } else if result.is_null() {
        Err(NO_INTERFACE)
    } else {
        Ok(result)
    }
}

struct CriticalRegion;
impl Drop for CriticalRegion {
    fn drop(&mut self) {
        unsafe { KeLeaveCriticalRegion() }
    }
}
unsafe fn critical_region() -> CriticalRegion {
    KeEnterCriticalRegion();
    CriticalRegion
}

/// Passive-only controls serialize here, then wait for the one current copy
/// owner. No GetPosition call waits on either event. The spin lock protects only
/// metadata and event handoff, never PCM copy/clear or a blocking operation.
/// Normal kernel APCs remain disabled while acquiring and owning the event gate,
/// so a suspended request thread cannot strand power, removal, or buffer release.
struct Control {
    cable: *mut Cable,
    _apcs: CriticalRegion,
}
impl Drop for Control {
    fn drop(&mut self) {
        unsafe {
            {
                let mut guard = lock(self.cable);
                guard.state().control_pending = false;
            }
            KeSetEvent((*self.cable).control_gate.get(), 0, 0);
        }
    }
}
unsafe fn wait_event(event: *mut KernelEvent) {
    // Executive / KernelMode / nonalertable / indefinite. Callers are PASSIVE.
    assert_eq!(
        KeWaitForSingleObject(event.cast(), 0, 0, 0, ptr::null()),
        OK
    );
}
unsafe fn control(cable: *mut Cable) -> Control {
    let apcs = critical_region();
    wait_event((*cable).control_gate.get());
    let copying = {
        let mut guard = lock(cable);
        let state = guard.state();
        state.control_pending = true;
        state.copying
    };
    if copying {
        wait_event((*cable).copy_idle.get());
    }
    Control { cable, _apcs: apcs }
}

/// Copy at caller IRQL, without raising it or holding any adapter spin lock.
/// The copy owner has exclusive ring access and controls cannot unmap buffers.
/// At most one cyclic buffer per direction is processed; older missed periods
/// are dropped. There are no allocations, waits, or floating point operations.
unsafe fn copy_period(cable: *mut Cable, slots: [Slot; 2], ticks: u64, frequency: u64) -> [u64; 2] {
    #[cfg(test)]
    tests::before_pcm_copy();
    let frames = (*cable).frames.get().cast::<Frame>();
    let mut processed = [slots[0].processed, slots[1].processed];
    for (index, slot) in slots.into_iter().enumerate() {
        if slot.timeline.state != State::Run || slot.buffer.is_null() || slot.bytes < FRAME_BYTES {
            continue;
        }
        let end = slot.timeline.position(ticks, frequency);
        let capacity = u64::from(slot.bytes / FRAME_BYTES);
        let start = slot.processed.max(end.saturating_sub(capacity));
        for frame in start..end {
            let global = slot.global_base.saturating_add(frame);
            let buffer = slot.buffer.add((frame % capacity) as usize);
            if index == 0 {
                let pair = ptr::read_volatile(buffer);
                frames.add(global as usize % CABLE_FRAMES).write(Frame {
                    time_plus_one: global.saturating_add(1),
                    pair,
                });
            } else {
                let pair = if let Some(source) = global.checked_sub(DELAY_FRAMES) {
                    let stored = frames.add(source as usize % CABLE_FRAMES).read();
                    if stored.time_plus_one == source + 1 {
                        stored.pair
                    } else {
                        0
                    }
                } else {
                    0
                };
                ptr::write_volatile(buffer, pair);
            }
        }
        processed[index] = end;
    }
    processed
}

/// A contending audio query observes the last published position and returns;
/// it never spins waiting for the current PCM copy or a passive control.
unsafe fn try_pump(cable: *mut Cable) {
    // A passive caller must not be suspended while owning the copy and keeping
    // controls waiting. Dispatch callers already prevent normal kernel APCs.
    let _apcs = if KeGetCurrentIrql() <= 1 {
        Some(critical_region())
    } else {
        None
    };
    let (slots, ticks, frequency) = {
        let mut guard = lock(cable);
        let state = guard.state();
        if !state.powered || state.stopping || state.copying || state.control_pending {
            return;
        }
        state.copying = true;
        KeClearEvent((*cable).copy_idle.get());
        (state.slots, now(), state.frequency)
    };
    let processed = copy_period(cable, slots, ticks, frequency);
    let mut guard = lock(cable);
    let state = guard.state();
    for (slot, frame) in state.slots.iter_mut().zip(processed) {
        slot.processed = frame;
    }
    state.copying = false;
    KeSetEvent((*cable).copy_idle.get(), 0, 0);
}

/// Drain the final period before a passive state transition. The caller owns
/// Control, which excludes audio copies and buffer mutation throughout.
unsafe fn pump_control(owner: &Control) -> u64 {
    let cable = owner.cable;
    let (slots, ticks, frequency, enabled) = {
        let mut guard = lock(cable);
        let state = guard.state();
        (
            state.slots,
            now(),
            state.frequency,
            state.powered && !state.stopping,
        )
    };
    if enabled {
        let processed = copy_period(cable, slots, ticks, frequency);
        let mut guard = lock(cable);
        for (slot, frame) in guard.state().slots.iter_mut().zip(processed) {
            slot.processed = frame;
        }
    }
    ticks
}

unsafe extern "system" fn cable_query(this: Raw, id: *const Guid, out: *mut Raw) -> NtStatus {
    if this.is_null() || id.is_null() || out.is_null() {
        return INVALID;
    }
    *out = ptr::null_mut();
    let cable = this.cast::<Cable>();
    let requested = ptr::read_unaligned(id);
    if requested == IID_IUNKNOWN || requested == IID_IADAPTER_POWER_MANAGEMENT {
        *out = this;
    } else if requested == IID_IADAPTER_PNP_MANAGEMENT {
        *out = ptr::addr_of_mut!((*cable).pnp).cast();
    } else {
        return NO_INTERFACE;
    }
    cable_add(this);
    OK
}
unsafe extern "system" fn cable_add(this: Raw) -> u32 {
    (*this.cast::<Cable>()).refs.fetch_add(1, Ordering::Relaxed) + 1
}
unsafe extern "system" fn cable_release(this: Raw) -> u32 {
    let old = (*this.cast::<Cable>()).refs.fetch_sub(1, Ordering::AcqRel);
    if old == 1 {
        ExFreePoolWithTag(this, TAG);
    }
    old - 1
}
unsafe extern "system" fn power_change(this: Raw, power: PowerState) {
    let cable = this.cast::<Cable>();
    let owner = control(cable);
    let ticks = pump_control(&owner);
    let mut guard = lock(cable);
    let state = guard.state();
    let awake = power.device_state == 1;
    let global = global_frame(state, ticks);
    for slot in &mut state.slots {
        if !awake && slot.timeline.state == State::Run {
            slot.resume_after_power = true;
            slot.timeline
                .transition(State::Pause, ticks, state.frequency);
        } else if awake && slot.resume_after_power && !state.stopping {
            slot.timeline.transition(State::Run, ticks, state.frequency);
            slot.global_base = global.saturating_sub(slot.timeline.frames);
            slot.resume_after_power = false;
        }
    }
    state.powered = awake;
}
unsafe extern "system" fn query_power(_: Raw, power: PowerState) -> NtStatus {
    if (1..=4).contains(&power.device_state) {
        OK
    } else {
        INVALID
    }
}
unsafe extern "system" fn capabilities(_: Raw, out: *mut DeviceCapabilities) -> NtStatus {
    if out.is_null() {
        INVALID
    } else {
        OK
    }
}
static POWER_VTABLE: Shared<AdapterPowerManagementVtable> = Shared(AdapterPowerManagementVtable {
    unknown: UnknownVtable {
        query_interface: cable_query,
        add_ref: cable_add,
        release: cable_release,
    },
    power_change_state: power_change,
    query_power_change_state: query_power,
    query_device_capabilities: capabilities,
});
unsafe extern "system" fn pnp_query(this: Raw, id: *const Guid, out: *mut Raw) -> NtStatus {
    cable_query((*this.cast::<PnpPart>()).owner.cast(), id, out)
}
unsafe extern "system" fn pnp_add(this: Raw) -> u32 {
    cable_add((*this.cast::<PnpPart>()).owner.cast())
}
unsafe extern "system" fn pnp_release(this: Raw) -> u32 {
    cable_release((*this.cast::<PnpPart>()).owner.cast())
}
unsafe extern "system" fn rebalance(_: Raw) -> i32 {
    1
}
unsafe extern "system" fn pnp_query_stop(_: Raw) {}
unsafe extern "system" fn pnp_cancel_stop(_: Raw) {}
unsafe extern "system" fn pnp_stop(this: Raw) {
    teardown((*this.cast::<PnpPart>()).owner);
}
static PNP_VTABLE: Shared<AdapterPnpManagementVtable> = Shared(AdapterPnpManagementVtable {
    unknown: UnknownVtable {
        query_interface: pnp_query,
        add_ref: pnp_add,
        release: pnp_release,
    },
    get_supported_rebalance_type: rebalance,
    pnp_query_stop,
    pnp_cancel_stop,
    pnp_stop,
});

unsafe fn unregister_subdevice(device: *mut DeviceObject, port: *mut Port) {
    if let Ok(raw) = query(port.cast(), &IID_IUNREGISTER_SUBDEVICE) {
        let interface = raw.cast::<UnregisterSubdevice>();
        ((*(*interface).vtable).unregister_subdevice)(raw, device, port.cast());
        unknown_release(raw);
    }
}
unsafe fn unregister_connection(
    device: *mut DeviceObject,
    from: *mut Port,
    from_pin: u32,
    to: *mut Port,
    to_pin: u32,
) {
    if let Ok(raw) = query(from.cast(), &IID_IUNREGISTER_PHYSICAL_CONNECTION) {
        let interface = raw.cast::<UnregisterPhysicalConnection>();
        ((*(*interface).vtable).unregister_physical_connection)(
            raw,
            device,
            from.cast(),
            from_pin,
            to.cast(),
            to_pin,
        );
        unknown_release(raw);
    }
}
/// PortCls reserves extension slots 4..=7 for adapter-owned state. PnP requests
/// serialize this slot; its reference survives streams and registration errors.
unsafe fn adapter_slot(device: *mut DeviceObject) -> *mut *mut Cable {
    (*device.cast::<DeviceObjectPrefix>())
        .device_extension
        .cast::<*mut Cable>()
        .add(4)
}

unsafe fn teardown(cable: *mut Cable) {
    if (*cable).teardown_started.swap(true, Ordering::AcqRel) {
        return;
    }
    cable_add(cable.cast()); // Deregistration can release PortCls's adapter references.
    let owner = control(cable);
    let (ports, subdevices, connections) = {
        let mut guard = lock(cable);
        let state = guard.state();
        state.stopping = true;
        for slot in &mut state.slots {
            slot.timeline = Timeline::new();
            slot.resume_after_power = false;
        }
        let values = (state.ports, state.subdevices, state.connections);
        state.ports = [ptr::null_mut(); 2];
        state.subdevices = 0;
        state.connections = 0;
        values
    };
    drop(owner); // Stopping now excludes all new PCM copies.
    let device = (*cable).device;
    let extension_owns_ref = *adapter_slot(device) == cable;
    if extension_owns_ref {
        *adapter_slot(device) = ptr::null_mut();
    }
    if connections & 1 != 0 {
        unregister_connection(device, ports[0], 1, ports[1], 0);
    }
    if connections & 2 != 0 {
        unregister_connection(device, ports[1], 3, ports[0], 2);
    }
    for (index, port) in ports.iter().copied().enumerate() {
        if subdevices & (1 << index) != 0 {
            unregister_subdevice(device, port);
        }
    }
    if (*cable).power_registered.swap(false, Ordering::AcqRel) {
        PcUnregisterAdapterPowerManagement(device);
    }
    if (*cable).pnp_registered.swap(false, Ordering::AcqRel) {
        PcUnregisterAdapterPnpManagement(device);
    }
    for port in ports {
        unknown_release(port.cast());
    }
    if extension_owns_ref {
        cable_release(cable.cast());
    }
    cable_release(cable.cast());
}

unsafe extern "system" fn miniport_query(this: Raw, id: *const Guid, out: *mut Raw) -> NtStatus {
    if this.is_null() || id.is_null() || out.is_null() {
        return INVALID;
    }
    *out = ptr::null_mut();
    let miniport = &*this.cast::<Miniport>();
    let id = ptr::read_unaligned(id);
    if id == IID_IUNKNOWN
        || id == IID_IMINIPORT
        || (miniport.topology && id == IID_IMINIPORT_TOPOLOGY)
        || (!miniport.topology && id == IID_IMINIPORT_WAVERT)
    {
        *out = this;
        miniport_add(this);
        OK
    } else {
        NO_INTERFACE
    }
}
unsafe extern "system" fn miniport_add(this: Raw) -> u32 {
    (*this.cast::<Miniport>())
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}
unsafe extern "system" fn miniport_release(this: Raw) -> u32 {
    let object = this.cast::<Miniport>();
    let old = (*object).refs.fetch_sub(1, Ordering::AcqRel);
    if old == 1 {
        let cable = (*object).cable;
        ExFreePoolWithTag(this, TAG);
        cable_release(cable.cast());
    }
    old - 1
}
unsafe extern "system" fn get_description(
    this: Raw,
    out: *mut *mut PcFilterDescriptor,
) -> NtStatus {
    if out.is_null() {
        return INVALID;
    }
    *out = if (*this.cast::<Miniport>()).topology {
        ptr::addr_of!(descriptors::TOPOLOGY_FILTER).cast_mut()
    } else {
        ptr::addr_of!(descriptors::WAVE_FILTER).cast_mut()
    };
    OK
}
unsafe extern "system" fn intersection(
    this: Raw,
    pin: u32,
    requested: *mut KsDataFormat,
    matching: *mut KsDataFormat,
    length: u32,
    out: Raw,
    written: *mut u32,
) -> NtStatus {
    descriptors::data_range_intersection(
        !(*this.cast::<Miniport>()).topology,
        pin,
        requested,
        matching,
        length,
        out,
        written,
    )
}
unsafe extern "system" fn init(
    _: Raw,
    _: *mut Unknown,
    _: *mut ResourceList,
    _: *mut Port,
) -> NtStatus {
    OK
}
unsafe extern "system" fn device_description(_: Raw, out: *mut DeviceDescription) -> NtStatus {
    if out.is_null() {
        return INVALID;
    }
    ptr::write_bytes(out, 0, 1);
    (*out).version = 2;
    (*out).master = 1;
    (*out).scatter_gather = 1;
    (*out).dma_32_bit_addresses = 1;
    (*out).dma_64_bit_addresses = 1;
    (*out).maximum_length = MAX_BUFFER_BYTES;
    OK
}
const fn miniport_base() -> MiniportVtable {
    MiniportVtable {
        unknown: UnknownVtable {
            query_interface: miniport_query,
            add_ref: miniport_add,
            release: miniport_release,
        },
        get_description,
        data_range_intersection: intersection,
    }
}
static WAVE_VTABLE: Shared<MiniportWaveRtVtable> = Shared(MiniportWaveRtVtable {
    miniport: miniport_base(),
    init,
    new_stream,
    get_device_description: device_description,
});
static TOPOLOGY_VTABLE: Shared<MiniportTopologyVtable> = Shared(MiniportTopologyVtable {
    miniport: miniport_base(),
    init,
});

unsafe extern "system" fn stream_query(this: Raw, id: *const Guid, out: *mut Raw) -> NtStatus {
    if this.is_null() || id.is_null() || out.is_null() {
        return INVALID;
    }
    *out = ptr::null_mut();
    let id = ptr::read_unaligned(id);
    if id == IID_IUNKNOWN || id == IID_IMINIPORT_WAVERT_STREAM {
        *out = this;
        stream_add(this);
        OK
    } else {
        NO_INTERFACE
    }
}
unsafe extern "system" fn stream_add(this: Raw) -> u32 {
    (*this.cast::<Stream>())
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}
unsafe extern "system" fn stream_release(this: Raw) -> u32 {
    let stream = this.cast::<Stream>();
    let old = (*stream).refs.fetch_sub(1, Ordering::AcqRel);
    if old == 1 {
        let cable = (*stream).cable;
        let (mdl, bytes) = {
            let mut guard = lock(cable);
            let slot = guard.state().slots[(*stream).index];
            (slot.mdl, slot.bytes)
        };
        if !mdl.is_null() {
            // PortCls normally calls FreeAudioBuffer before final Release. This
            // fallback supports passive construction/error cleanup only; a
            // dispatch-level Release must arrive with its buffer already freed.
            free_buffer(this, mdl, bytes);
        }
        {
            let mut guard = lock(cable);
            guard.state().slots[(*stream).index].active = false;
        }
        unknown_release((*stream).port.cast());
        ExFreePoolWithTag(this, TAG);
        cable_release(cable.cast());
    }
    old - 1
}
unsafe extern "system" fn new_stream(
    this: Raw,
    out: *mut *mut MiniportWaveRtStream,
    port: *mut PortWaveRtStream,
    pin: u32,
    capture: u8,
    format: *mut KsDataFormat,
) -> NtStatus {
    if out.is_null() || port.is_null() {
        return INVALID;
    }
    *out = ptr::null_mut();
    if !((pin == 0 && capture == 0) || (pin == 3 && capture == 1)) {
        return INVALID;
    }
    if !descriptors::validate_format(format) {
        return INVALID;
    }
    let cable = (*this.cast::<Miniport>()).cable;
    let index = usize::from(capture != 0);
    let stream = ExAllocatePool2(NONPAGED, size_of::<Stream>(), TAG).cast::<Stream>();
    if stream.is_null() {
        return NO_MEMORY;
    }
    {
        let mut guard = lock(cable);
        let state = guard.state();
        if state.stopping || state.control_pending || state.slots[index].active {
            ExFreePoolWithTag(stream.cast(), TAG);
            return BAD_STATE;
        }
        state.slots[index].active = true;
        state.slots[index].timeline = Timeline::new();
    }
    ptr::write(
        stream,
        Stream {
            vtable: &STREAM_VTABLE.0,
            refs: AtomicU32::new(1),
            cable,
            port,
            index,
        },
    );
    cable_add(cable.cast());
    unknown_add(port.cast());
    *out = stream.cast();
    OK
}
unsafe extern "system" fn set_format(this: Raw, format: *mut KsDataFormat) -> NtStatus {
    if !descriptors::validate_format(format) {
        return INVALID;
    }
    let stream = &*this.cast::<Stream>();
    let _owner = control(stream.cable);
    let mut guard = lock(stream.cable);
    let state = guard.state();
    if state.stopping || state.slots[stream.index].timeline.state == State::Run {
        return BAD_STATE;
    }
    OK
}
unsafe extern "system" fn set_state(this: Raw, requested: KsState) -> NtStatus {
    let desired = match requested {
        0 => State::Stop,
        1 => State::Acquire,
        2 => State::Pause,
        3 => State::Run,
        _ => return INVALID,
    };
    let stream = &*this.cast::<Stream>();
    let owner = control(stream.cable);
    let ticks = pump_control(&owner);
    let mut guard = lock(stream.cable);
    let state = guard.state();
    let global = global_frame(state, ticks);
    let slot = &mut state.slots[stream.index];
    if desired == State::Run && (state.stopping || !state.powered || slot.buffer.is_null()) {
        return BAD_STATE;
    }
    slot.timeline.transition(desired, ticks, state.frequency);
    slot.resume_after_power = false;
    if desired == State::Run {
        slot.global_base = global.saturating_sub(slot.timeline.frames);
    }
    if desired == State::Stop {
        slot.processed = 0;
    }
    let clear = if desired == State::Stop {
        (slot.buffer, slot.bytes)
    } else {
        (ptr::null_mut(), 0)
    };
    drop(guard);
    if !clear.0.is_null() {
        #[cfg(test)]
        tests::assert_no_spin_lock();
        ptr::write_bytes(clear.0.cast::<u8>(), 0, clear.1 as usize);
    }
    OK
}
unsafe extern "system" fn position(this: Raw, out: *mut KsAudioPosition) -> NtStatus {
    if out.is_null() {
        return INVALID;
    }
    let stream = &*this.cast::<Stream>();
    try_pump(stream.cable);
    let mut guard = lock(stream.cable);
    let state = guard.state();
    if state.stopping {
        return BAD_STATE;
    }
    let slot = state.slots[stream.index];
    let offset = u64::from(byte_offset(slot.processed, slot.bytes));
    ptr::write_unaligned(
        out,
        KsAudioPosition {
            play_offset: offset,
            write_offset: offset,
        },
    );
    OK
}
unsafe extern "system" fn allocate_buffer(
    this: Raw,
    requested: u32,
    mdl_out: *mut *mut Mdl,
    actual: *mut u32,
    offset: *mut u32,
    cache: *mut MemoryCachingType,
) -> NtStatus {
    if mdl_out.is_null() || actual.is_null() || offset.is_null() || cache.is_null() {
        return INVALID;
    }
    *mdl_out = ptr::null_mut();
    *actual = 0;
    *offset = 0;
    *cache = MM_CACHED;
    let Some(bytes) = rounded_buffer_bytes(requested) else {
        return INVALID;
    };
    let stream = &*this.cast::<Stream>();
    let _owner = control(stream.cable);
    {
        let mut guard = lock(stream.cable);
        let state = guard.state();
        let slot = &mut state.slots[stream.index];
        if state.stopping || slot.allocating || !slot.mdl.is_null() {
            return BAD_STATE;
        }
        slot.allocating = true;
    }
    let vtable = &*(*stream.port).vtable;
    let mdl = (vtable.allocate_pages_for_mdl)(
        stream.port.cast(),
        PhysicalAddress {
            quad_part: i64::MAX,
        },
        bytes as usize,
    );
    let mut buffer = ptr::null_mut();
    if !mdl.is_null()
        && u64::from((vtable.get_physical_pages_count)(stream.port.cast(), mdl)) * 4096
            >= u64::from(bytes)
    {
        buffer = (vtable.map_allocated_pages)(stream.port.cast(), mdl, MM_CACHED);
    }
    if !buffer.is_null() {
        ptr::write_bytes(buffer.cast::<u8>(), 0, bytes as usize);
    }
    let accepted = {
        let mut guard = lock(stream.cable);
        let state = guard.state();
        let slot = &mut state.slots[stream.index];
        slot.allocating = false;
        if state.stopping || buffer.is_null() {
            false
        } else {
            slot.mdl = mdl;
            slot.buffer = buffer.cast();
            slot.bytes = bytes;
            true
        }
    };
    if !accepted {
        if !buffer.is_null() {
            (vtable.unmap_allocated_pages)(stream.port.cast(), buffer, mdl);
        }
        if !mdl.is_null() {
            (vtable.free_pages_from_mdl)(stream.port.cast(), mdl);
        }
        return NO_MEMORY;
    }
    *mdl_out = mdl;
    *actual = bytes;
    OK
}
unsafe extern "system" fn free_buffer(this: Raw, mdl: *mut Mdl, _: u32) {
    if mdl.is_null() {
        return;
    }
    let stream = &*this.cast::<Stream>();
    let _owner = control(stream.cable);
    let buffer = {
        let mut guard = lock(stream.cable);
        let slot = &mut guard.state().slots[stream.index];
        if slot.mdl != mdl {
            return;
        }
        let buffer = slot.buffer;
        slot.mdl = ptr::null_mut();
        slot.buffer = ptr::null_mut();
        slot.bytes = 0;
        slot.timeline = Timeline::new();
        slot.processed = 0;
        slot.resume_after_power = false;
        buffer
    };
    let vtable = &*(*stream.port).vtable;
    (vtable.unmap_allocated_pages)(stream.port.cast(), buffer.cast(), mdl);
    (vtable.free_pages_from_mdl)(stream.port.cast(), mdl);
}
unsafe extern "system" fn latency(this: Raw, out: *mut KsRtAudioHwLatency) {
    if out.is_null() {
        return;
    }
    let capture = (*this.cast::<Stream>()).index == 1;
    ptr::write_unaligned(
        out,
        KsRtAudioHwLatency {
            fifo_size: if capture {
                (DELAY_FRAMES * u64::from(FRAME_BYTES)) as u32
            } else {
                0
            },
            chipset_delay: 0,
            codec_delay: 0,
        },
    );
}
unsafe extern "system" fn no_register(_: Raw, _: *mut KsRtAudioHwRegister) -> NtStatus {
    NOT_SUPPORTED
}
static STREAM_VTABLE: Shared<MiniportWaveRtStreamVtable> = Shared(MiniportWaveRtStreamVtable {
    unknown: UnknownVtable {
        query_interface: stream_query,
        add_ref: stream_add,
        release: stream_release,
    },
    set_format,
    set_state,
    get_position: position,
    allocate_audio_buffer: allocate_buffer,
    free_audio_buffer: free_buffer,
    get_hw_latency: latency,
    get_position_register: no_register,
    get_clock_register: no_register,
});

unsafe fn make_miniport(cable: *mut Cable, topology: bool) -> *mut Miniport {
    let object = ExAllocatePool2(NONPAGED, size_of::<Miniport>(), TAG).cast::<Miniport>();
    if !object.is_null() {
        let vtable = if topology {
            ptr::addr_of!(TOPOLOGY_VTABLE.0).cast()
        } else {
            ptr::addr_of!(WAVE_VTABLE.0).cast()
        };
        ptr::write(
            object,
            Miniport {
                vtable,
                refs: AtomicU32::new(1),
                cable,
                topology,
            },
        );
        cable_add(cable.cast());
    }
    object
}
unsafe extern "system" fn start_device(
    device: *mut DeviceObject,
    irp: *mut Irp,
    resources: *mut ResourceList,
) -> NtStatus {
    let cable = ExAllocatePool2(NONPAGED, size_of::<Cable>(), TAG).cast::<Cable>();
    if cable.is_null() {
        return NO_MEMORY;
    }
    // Initialize the large zeroed pool allocation in-place; never materialize
    // the 256 KiB frame array on the small kernel stack.
    ptr::addr_of_mut!((*cable).vtable).write(&POWER_VTABLE.0);
    ptr::addr_of_mut!((*cable).refs).write(AtomicU32::new(1));
    ptr::addr_of_mut!((*cable).power_registered).write(AtomicBool::new(false));
    ptr::addr_of_mut!((*cable).pnp_registered).write(AtomicBool::new(false));
    ptr::addr_of_mut!((*cable).teardown_started).write(AtomicBool::new(false));
    ptr::addr_of_mut!((*cable).pnp).write(PnpPart {
        vtable: &PNP_VTABLE.0,
        owner: cable,
    });
    (*cable).device = device;
    KeInitializeEvent((*cable).control_gate.get(), 1, 1); // SynchronizationEvent
    KeInitializeEvent((*cable).copy_idle.get(), 0, 1); // NotificationEvent
    let mut frequency = PhysicalAddress { quad_part: 0 };
    let ticks = KeQueryPerformanceCounter(&mut frequency).quad_part as u64;
    if frequency.quad_part <= 0 || frequency.quad_part as u64 > MAX_CLOCK_FREQUENCY {
        cable_release(cable.cast());
        return BAD_STATE;
    }
    let state = &mut *(*cable).state.get();
    state.frequency = frequency.quad_part as u64;
    state.epoch = ticks;
    state.powered = true;
    if !(*adapter_slot(device)).is_null() {
        cable_release(cable.cast());
        return BAD_STATE;
    }
    cable_add(cable.cast());
    *adapter_slot(device) = cable;
    let status = setup(cable, device, irp, resources);
    if status < 0 {
        teardown(cable);
    }
    cable_release(cable.cast());
    status
}
unsafe fn setup(
    cable: *mut Cable,
    device: *mut DeviceObject,
    irp: *mut Irp,
    resources: *mut ResourceList,
) -> NtStatus {
    const WAVE_NAME: [u16; 5] = [87, 97, 118, 101, 0];
    const TOPOLOGY_NAME: [u16; 9] = [84, 111, 112, 111, 108, 111, 103, 121, 0];
    for index in 0..2 {
        let mut port = ptr::null_mut();
        let class = if index == 0 {
            &CLSID_PORT_WAVERT
        } else {
            &CLSID_PORT_TOPOLOGY
        };
        let status = PcNewPort(&mut port, class);
        if status < 0 {
            return status;
        }
        (*(*cable).state.get()).ports[index] = port;
        let miniport = make_miniport(cable, index == 1);
        if miniport.is_null() {
            return NO_MEMORY;
        }
        let status = ((*(*port).vtable).init)(
            port.cast(),
            device,
            irp,
            miniport.cast(),
            cable.cast(),
            resources,
        );
        miniport_release(miniport.cast());
        if status < 0 {
            return status;
        }
        let name = if index == 0 {
            WAVE_NAME.as_ptr()
        } else {
            TOPOLOGY_NAME.as_ptr()
        };
        let status = PcRegisterSubdevice(device, name, port.cast());
        if status < 0 {
            return status;
        }
        (*(*cable).state.get()).subdevices |= 1 << index;
    }
    let ports = (*(*cable).state.get()).ports;
    let status = PcRegisterPhysicalConnection(device, ports[0].cast(), 1, ports[1].cast(), 0);
    if status < 0 {
        return status;
    }
    (*(*cable).state.get()).connections |= 1;
    let status = PcRegisterPhysicalConnection(device, ports[1].cast(), 3, ports[0].cast(), 2);
    if status < 0 {
        return status;
    }
    (*(*cable).state.get()).connections |= 2;
    let status = PcRegisterAdapterPnpManagement(cable.cast(), device);
    if status < 0 {
        return status;
    }
    (*cable).pnp_registered.store(true, Ordering::Release);
    let status = PcRegisterAdapterPowerManagement(cable.cast(), device.cast());
    if status < 0 {
        return status;
    }
    (*cable).power_registered.store(true, Ordering::Release);
    OK
}
unsafe extern "system" fn add_device(
    driver: *mut DriverObject,
    physical: *mut DeviceObject,
) -> NtStatus {
    if driver.is_null() || physical.is_null() {
        return INVALID;
    }
    PcAddAdapterDevice(driver, physical, start_device, 2, 0)
}

unsafe extern "system" fn dispatch_pnp(device: *mut DeviceObject, irp: *mut Irp) -> NtStatus {
    // Remove can arrive without the rebalance PnpStop callback. Detach while the
    // FDO extension is valid, then let PortCls forward and complete the IRP.
    let stack = io_get_current_irp_stack_location(irp);
    if !stack.is_null()
        && matches!(
            (*stack).minor_function,
            IRP_MN_REMOVE_DEVICE | IRP_MN_SURPRISE_REMOVAL
        )
    {
        let cable = *adapter_slot(device);
        if !cable.is_null() {
            teardown(cable);
        }
    }
    PcDispatchIrp(device, irp)
}

/// WDM entry point; PortCls installs the documented PnP/power/KS IRP handlers.
#[no_mangle]
pub unsafe extern "system" fn DriverEntry(
    driver: *mut DriverObject,
    registry: *mut UnicodeString,
) -> NtStatus {
    if driver.is_null() || registry.is_null() {
        return INVALID;
    }
    let status = PcInitializeAdapterDriver(driver, registry, add_device);
    if status >= 0 {
        (*driver.cast::<DriverObjectFields>()).major_function[IRP_MJ_PNP] = Some(dispatch_pnp);
    }
    status
}

#[cfg(test)]
#[path = "kernel_tests.rs"]
mod tests;
