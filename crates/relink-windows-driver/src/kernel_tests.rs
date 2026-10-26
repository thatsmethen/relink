//! Host-only contract mocks. These exercise the actual Rust COM callbacks, not
//! Windows scheduling, IRQL enforcement, driver loading, or real PortCls.
use super::*;
use std::{
    alloc::{alloc_zeroed, dealloc, Layout},
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, AtomicUsize},
        Arc, Condvar, Mutex,
    },
};

#[derive(Default)]
struct Mock {
    pools: BTreeMap<usize, Layout>,
    ports: Vec<usize>,
    registered: Vec<usize>,
    connections: Vec<(usize, u32, usize, u32)>,
    power: usize,
    pnp: usize,
    live_ports: usize,
    live_mdls: usize,
    mappings: usize,
    dispatches: usize,
    operation: usize,
    fail_operation: usize,
    short_mdl: bool,
    fail_map: bool,
    events: BTreeMap<usize, Arc<Event>>,
}
static MOCK: Mutex<Option<Mock>> = Mutex::new(None);
static TICKS: AtomicU64 = AtomicU64::new(0);
static HELD_LOCKS: AtomicUsize = AtomicUsize::new(0);
static HELD_REGIONS: AtomicUsize = AtomicUsize::new(0);
std::thread_local! {
    static LOCAL_LOCKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LOCAL_REGIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LOCAL_IRQL: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}
struct Event {
    signal: Mutex<bool>,
    changed: Condvar,
    automatic: bool,
}
struct CopyPause {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}
static PAUSE_COPY: Mutex<Option<Arc<CopyPause>>> = Mutex::new(None);
pub(super) fn assert_no_spin_lock() {
    LOCAL_LOCKS.with(|count| assert_eq!(count.get(), 0));
}
pub(super) fn before_pcm_copy() {
    assert_no_spin_lock();
    if LOCAL_IRQL.with(|level| level.get() <= 1) {
        LOCAL_REGIONS.with(|count| assert!(count.get() > 0));
    }
    let pause = PAUSE_COPY.lock().unwrap().clone();
    if let Some(pause) = pause {
        let mut state = pause.state.lock().unwrap();
        state.0 = true;
        pause.changed.notify_all();
        while !state.1 {
            state = pause.changed.wait(state).unwrap();
        }
    }
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn KeEnterCriticalRegion() {
    assert_no_spin_lock();
    LOCAL_IRQL.with(|level| assert!(level.get() <= 1));
    LOCAL_REGIONS.with(|count| count.set(count.get() + 1));
    HELD_REGIONS.fetch_add(1, Ordering::SeqCst);
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn KeLeaveCriticalRegion() {
    assert_no_spin_lock();
    LOCAL_IRQL.with(|level| assert!(level.get() <= 1));
    LOCAL_REGIONS.with(|count| {
        assert!(count.get() > 0);
        count.set(count.get() - 1);
    });
    HELD_REGIONS.fetch_sub(1, Ordering::SeqCst);
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn KeGetCurrentIrql() -> u8 {
    LOCAL_IRQL.with(|level| level.get())
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn KeInitializeEvent(event: *mut KernelEvent, kind: i32, signaled: u8) {
    with_mock(|m| {
        m.events.insert(
            event as usize,
            Arc::new(Event {
                signal: Mutex::new(signaled != 0),
                changed: Condvar::new(),
                automatic: kind == 1,
            }),
        );
    });
}
fn event(pointer: usize) -> Arc<Event> {
    with_mock(|m| m.events.get(&pointer).unwrap().clone())
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn KeClearEvent(pointer: *mut KernelEvent) {
    *event(pointer as usize).signal.lock().unwrap() = false;
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn KeSetEvent(pointer: *mut KernelEvent, increment: i32, wait: u8) -> i32 {
    assert_eq!((increment, wait), (0, 0));
    let event = event(pointer as usize);
    if event.automatic {
        LOCAL_REGIONS.with(|count| assert!(count.get() > 0));
    }
    let mut signaled = event.signal.lock().unwrap();
    let old = *signaled;
    *signaled = true;
    event.changed.notify_all();
    i32::from(old)
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn KeWaitForSingleObject(
    pointer: Raw,
    reason: i32,
    mode: i8,
    alertable: u8,
    timeout: *const PhysicalAddress,
) -> NtStatus {
    assert_no_spin_lock();
    LOCAL_REGIONS.with(|count| assert!(count.get() > 0));
    LOCAL_IRQL.with(|level| assert_eq!(level.get(), 0));
    assert_eq!((reason, mode, alertable), (0, 0, 0));
    assert!(timeout.is_null());
    let event = event(pointer as usize);
    let mut signaled = event.signal.lock().unwrap();
    while !*signaled {
        signaled = event.changed.wait(signaled).unwrap();
    }
    if event.automatic {
        *signaled = false;
    }
    OK
}
fn with_mock<T>(f: impl FnOnce(&mut Mock) -> T) -> T {
    f(MOCK.lock().unwrap().as_mut().unwrap())
}
fn fail_step() -> bool {
    with_mock(|m| {
        m.operation += 1;
        m.operation == m.fail_operation
    })
}
fn reset(fail_operation: usize) {
    *MOCK.lock().unwrap() = Some(Mock {
        fail_operation,
        ..Mock::default()
    });
    TICKS.store(0, Ordering::SeqCst);
    assert_eq!(HELD_LOCKS.load(Ordering::SeqCst), 0);
    assert_eq!(HELD_REGIONS.load(Ordering::SeqCst), 0);
}
fn assert_clean() {
    assert_eq!(HELD_REGIONS.load(Ordering::SeqCst), 0);
    with_mock(|m| {
        assert!(
            m.pools.is_empty(),
            "remaining pool allocations: {:?}",
            m.pools.keys()
        );
        assert!(m.registered.is_empty());
        assert!(m.connections.is_empty());
        assert!(m.events.is_empty());
        assert_eq!(
            (m.power, m.pnp, m.live_ports, m.live_mdls, m.mappings),
            (0, 0, 0, 0, 0)
        );
    });
    assert_eq!(HELD_LOCKS.load(Ordering::SeqCst), 0);
}

#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn ExAllocatePool2(flags: u64, bytes: usize, tag: u32) -> Raw {
    assert_eq!(flags, NONPAGED);
    assert_eq!(tag, TAG);
    let layout = Layout::from_size_align(bytes, 16).unwrap();
    let pointer = alloc_zeroed(layout);
    if !pointer.is_null() {
        with_mock(|m| {
            m.pools.insert(pointer as usize, layout);
        });
    }
    pointer.cast()
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn ExFreePoolWithTag(pointer: Raw, tag: u32) {
    assert_eq!(tag, TAG);
    let layout =
        with_mock(|m| m.pools.remove(&(pointer as usize))).expect("double or foreign pool free");
    with_mock(|m| {
        m.events.retain(|address, _| {
            *address < pointer as usize || *address >= pointer as usize + layout.size()
        });
    });
    dealloc(pointer.cast(), layout);
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn KeAcquireSpinLockRaiseToDpc(pointer: *mut usize) -> u8 {
    let atomic = AtomicUsize::from_ptr(pointer);
    while atomic
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        std::hint::spin_loop();
    }
    HELD_LOCKS.fetch_add(1, Ordering::SeqCst);
    LOCAL_LOCKS.with(|count| count.set(count.get() + 1));
    LOCAL_IRQL.with(|level| level.replace(2))
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn KeReleaseSpinLock(pointer: *mut usize, old_irql: u8) {
    assert_eq!(AtomicUsize::from_ptr(pointer).swap(0, Ordering::Release), 1);
    HELD_LOCKS.fetch_sub(1, Ordering::SeqCst);
    LOCAL_LOCKS.with(|count| count.set(count.get() - 1));
    LOCAL_IRQL.with(|level| level.set(old_irql));
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn KeQueryPerformanceCounter(
    frequency: *mut PhysicalAddress,
) -> PhysicalAddress {
    if !frequency.is_null() {
        (*frequency).quad_part = 48_000;
    } else {
        // Clock samples used for progress must be serialized with the pump;
        // sampling before the lock can reorder concurrent callback timestamps.
        LOCAL_LOCKS.with(|count| assert_eq!(count.get(), 1));
    }
    PhysicalAddress {
        quad_part: TICKS.load(Ordering::SeqCst) as i64,
    }
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn KeBugCheckEx(_: u32, _: usize, _: usize, _: usize, _: usize) -> ! {
    std::process::abort()
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn PcInitializeAdapterDriver(
    _: *mut DriverObject,
    _: *mut UnicodeString,
    _: unsafe extern "system" fn(*mut DriverObject, *mut DeviceObject) -> NtStatus,
) -> NtStatus {
    OK
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn PcAddAdapterDevice(
    _: *mut DriverObject,
    _: *mut DeviceObject,
    _: unsafe extern "system" fn(*mut DeviceObject, *mut Irp, *mut ResourceList) -> NtStatus,
    objects: u32,
    extra: u32,
) -> NtStatus {
    assert_eq!((objects, extra), (2, 0));
    OK
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn PcDispatchIrp(_: *mut DeviceObject, _: *mut Irp) -> NtStatus {
    with_mock(|m| m.dispatches += 1);
    OK
}

#[repr(C)]
struct PortPart {
    vtable: *const c_void,
    owner: *mut FakePort,
}
#[repr(C)]
struct FakePort {
    vtable: *const PortVtable,
    refs: AtomicU32,
    miniport: Raw,
    adapter: Raw,
    unregister: PortPart,
    disconnect: PortPart,
}
unsafe extern "system" fn port_add(this: Raw) -> u32 {
    (*this.cast::<FakePort>())
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}
unsafe extern "system" fn port_release(this: Raw) -> u32 {
    let port = this.cast::<FakePort>();
    let old = (*port).refs.fetch_sub(1, Ordering::AcqRel);
    if old == 1 {
        unknown_release((*port).miniport);
        unknown_release((*port).adapter);
        drop(Box::from_raw(port));
        with_mock(|m| m.live_ports -= 1);
    }
    old - 1
}
unsafe extern "system" fn port_query(this: Raw, id: *const Guid, out: *mut Raw) -> NtStatus {
    let port = this.cast::<FakePort>();
    *out = ptr::null_mut();
    if *id == IID_IUNREGISTER_SUBDEVICE {
        *out = ptr::addr_of_mut!((*port).unregister).cast();
    } else if *id == IID_IUNREGISTER_PHYSICAL_CONNECTION {
        *out = ptr::addr_of_mut!((*port).disconnect).cast();
    } else if *id == IID_IUNKNOWN {
        *out = this;
    } else {
        return NO_INTERFACE;
    }
    port_add(this);
    OK
}
unsafe extern "system" fn part_query(this: Raw, id: *const Guid, out: *mut Raw) -> NtStatus {
    port_query((*this.cast::<PortPart>()).owner.cast(), id, out)
}
unsafe extern "system" fn part_add(this: Raw) -> u32 {
    port_add((*this.cast::<PortPart>()).owner.cast())
}
unsafe extern "system" fn part_release(this: Raw) -> u32 {
    port_release((*this.cast::<PortPart>()).owner.cast())
}
unsafe extern "system" fn port_init(
    this: Raw,
    _: *mut DeviceObject,
    _: *mut Irp,
    miniport: *mut Unknown,
    adapter: *mut Unknown,
    resources: *mut ResourceList,
) -> NtStatus {
    if fail_step() {
        return NO_MEMORY;
    }
    let port = this.cast::<FakePort>();
    (*port).miniport = miniport.cast();
    (*port).adapter = adapter.cast();
    unknown_add(miniport.cast());
    unknown_add(adapter.cast());
    // The topology and WaveRT Init signatures share their machine ABI.
    let table = (*miniport.cast::<Miniport>())
        .vtable
        .cast::<MiniportWaveRtVtable>();
    ((*table).init)(miniport.cast(), adapter, resources, this.cast())
}
unsafe extern "system" fn device_property(_: Raw, _: i32, _: u32, _: Raw, _: *mut u32) -> NtStatus {
    NOT_SUPPORTED
}
unsafe extern "system" fn registry_key(
    _: Raw,
    _: *mut Raw,
    _: *mut Unknown,
    _: u32,
    _: u32,
    _: Raw,
    _: u32,
    _: *mut u32,
) -> NtStatus {
    NOT_SUPPORTED
}
const fn part_unknown() -> UnknownVtable {
    UnknownVtable {
        query_interface: part_query,
        add_ref: part_add,
        release: part_release,
    }
}
unsafe extern "system" fn unregister(_: Raw, _: *mut DeviceObject, port: *mut Unknown) -> NtStatus {
    let removed = with_mock(|m| {
        let index = m
            .registered
            .iter()
            .position(|p| *p == port as usize)
            .unwrap();
        m.registered.remove(index)
    });
    unknown_release(removed as Raw);
    OK
}
unsafe extern "system" fn disconnect(
    _: Raw,
    _: *mut DeviceObject,
    from: *mut Unknown,
    fp: u32,
    to: *mut Unknown,
    tp: u32,
) -> NtStatus {
    with_mock(|m| {
        let index = m
            .connections
            .iter()
            .position(|c| *c == (from as usize, fp, to as usize, tp))
            .unwrap();
        m.connections.remove(index);
    });
    OK
}
unsafe extern "system" fn disconnect_to(
    _: Raw,
    _: *mut DeviceObject,
    _: *mut Unknown,
    _: u32,
    _: *mut UnicodeString,
    _: u32,
) -> NtStatus {
    NOT_SUPPORTED
}
unsafe extern "system" fn disconnect_from(
    _: Raw,
    _: *mut DeviceObject,
    _: *mut UnicodeString,
    _: u32,
    _: *mut Unknown,
    _: u32,
) -> NtStatus {
    NOT_SUPPORTED
}
static PORT_TABLE: Shared<PortVtable> = Shared(PortVtable {
    unknown: UnknownVtable {
        query_interface: port_query,
        add_ref: port_add,
        release: port_release,
    },
    init: port_init,
    get_device_property: device_property,
    new_registry_key: registry_key,
});
static UNREGISTER_TABLE: Shared<UnregisterSubdeviceVtable> = Shared(UnregisterSubdeviceVtable {
    unknown: part_unknown(),
    unregister_subdevice: unregister,
});
static DISCONNECT_TABLE: Shared<UnregisterPhysicalConnectionVtable> =
    Shared(UnregisterPhysicalConnectionVtable {
        unknown: part_unknown(),
        unregister_physical_connection: disconnect,
        unregister_physical_connection_to_external: disconnect_to,
        unregister_physical_connection_from_external: disconnect_from,
    });
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn PcNewPort(out: *mut *mut Port, class: *const Guid) -> NtStatus {
    if fail_step() {
        return NO_MEMORY;
    }
    assert!(*class == CLSID_PORT_WAVERT || *class == CLSID_PORT_TOPOLOGY);
    let port = Box::into_raw(Box::new(FakePort {
        vtable: &PORT_TABLE.0,
        refs: AtomicU32::new(1),
        miniport: ptr::null_mut(),
        adapter: ptr::null_mut(),
        unregister: PortPart {
            vtable: ptr::addr_of!(UNREGISTER_TABLE.0).cast(),
            owner: ptr::null_mut(),
        },
        disconnect: PortPart {
            vtable: ptr::addr_of!(DISCONNECT_TABLE.0).cast(),
            owner: ptr::null_mut(),
        },
    }));
    (*port).unregister.owner = port;
    (*port).disconnect.owner = port;
    *out = port.cast();
    with_mock(|m| {
        m.ports.push(port as usize);
        m.live_ports += 1;
    });
    OK
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn PcRegisterSubdevice(
    _: *mut DeviceObject,
    _: *const u16,
    port: *mut Unknown,
) -> NtStatus {
    if fail_step() {
        return NO_MEMORY;
    }
    unknown_add(port.cast());
    with_mock(|m| m.registered.push(port as usize));
    OK
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn PcRegisterPhysicalConnection(
    _: *mut DeviceObject,
    from: *mut Unknown,
    fp: u32,
    to: *mut Unknown,
    tp: u32,
) -> NtStatus {
    if fail_step() {
        return NO_MEMORY;
    }
    with_mock(|m| m.connections.push((from as usize, fp, to as usize, tp)));
    OK
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn PcRegisterAdapterPowerManagement(
    adapter: *mut Unknown,
    _: Raw,
) -> NtStatus {
    if fail_step() {
        return NO_MEMORY;
    }
    let interface = query(adapter.cast(), &IID_IADAPTER_POWER_MANAGEMENT).unwrap();
    with_mock(|m| {
        assert_eq!(m.power, 0);
        m.power = interface as usize;
    });
    OK
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn PcUnregisterAdapterPowerManagement(_: *mut DeviceObject) -> NtStatus {
    let pointer = with_mock(|m| std::mem::take(&mut m.power));
    assert_ne!(pointer, 0);
    unknown_release(pointer as Raw);
    OK
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn PcRegisterAdapterPnpManagement(
    adapter: *mut Unknown,
    _: *mut DeviceObject,
) -> NtStatus {
    if fail_step() {
        return NO_MEMORY;
    }
    let interface = query(adapter.cast(), &IID_IADAPTER_PNP_MANAGEMENT).unwrap();
    with_mock(|m| {
        assert_eq!(m.pnp, 0);
        m.pnp = interface as usize;
    });
    OK
}
#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn PcUnregisterAdapterPnpManagement(_: *mut DeviceObject) -> NtStatus {
    let pointer = with_mock(|m| std::mem::take(&mut m.pnp));
    assert_ne!(pointer, 0);
    unknown_release(pointer as Raw);
    OK
}

#[repr(C)]
struct FakeStream {
    vtable: *const PortWaveRtStreamVtable,
    refs: AtomicU32,
}
struct FakeMdl {
    buffer: *mut u8,
    layout: Layout,
    mapped: bool,
    pages: u32,
}
unsafe extern "system" fn lower_query(this: Raw, _: *const Guid, out: *mut Raw) -> NtStatus {
    *out = this;
    lower_add(this);
    OK
}
unsafe extern "system" fn lower_add(this: Raw) -> u32 {
    (*this.cast::<FakeStream>())
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}
unsafe extern "system" fn lower_release(this: Raw) -> u32 {
    (*this.cast::<FakeStream>())
        .refs
        .fetch_sub(1, Ordering::AcqRel)
        - 1
}
unsafe extern "system" fn pages(_: Raw, _: PhysicalAddress, bytes: usize) -> *mut Mdl {
    assert_eq!(HELD_LOCKS.load(Ordering::SeqCst), 0);
    let layout = Layout::from_size_align(bytes.next_multiple_of(4096), 4096).unwrap();
    let buffer = alloc_zeroed(layout);
    assert!(!buffer.is_null());
    let count = with_mock(|m| {
        m.live_mdls += 1;
        if m.short_mdl {
            0
        } else {
            (layout.size() / 4096) as u32
        }
    });
    Box::into_raw(Box::new(FakeMdl {
        buffer,
        layout,
        mapped: false,
        pages: count,
    }))
    .cast()
}
unsafe extern "system" fn contiguous(
    this: Raw,
    _: PhysicalAddress,
    high: PhysicalAddress,
    bytes: usize,
) -> *mut Mdl {
    pages(this, high, bytes)
}
unsafe extern "system" fn map(_: Raw, mdl: *mut Mdl, cache: i32) -> Raw {
    assert_eq!(HELD_LOCKS.load(Ordering::SeqCst), 0);
    assert_eq!(cache, MM_CACHED);
    if with_mock(|m| m.fail_map) {
        return ptr::null_mut();
    }
    let mdl = &mut *mdl.cast::<FakeMdl>();
    assert!(!mdl.mapped);
    mdl.mapped = true;
    with_mock(|m| m.mappings += 1);
    mdl.buffer.cast()
}
unsafe extern "system" fn unmap(_: Raw, base: Raw, mdl: *mut Mdl) {
    assert_eq!(HELD_LOCKS.load(Ordering::SeqCst), 0);
    let mdl = &mut *mdl.cast::<FakeMdl>();
    assert!(mdl.mapped);
    assert_eq!(base, mdl.buffer.cast());
    mdl.mapped = false;
    with_mock(|m| m.mappings -= 1);
}
unsafe extern "system" fn free_pages(_: Raw, mdl: *mut Mdl) {
    assert_eq!(HELD_LOCKS.load(Ordering::SeqCst), 0);
    let mdl = Box::from_raw(mdl.cast::<FakeMdl>());
    assert!(!mdl.mapped);
    dealloc(mdl.buffer, mdl.layout);
    with_mock(|m| m.live_mdls -= 1);
}
unsafe extern "system" fn page_count(_: Raw, mdl: *mut Mdl) -> u32 {
    (*mdl.cast::<FakeMdl>()).pages
}
unsafe extern "system" fn page_address(_: Raw, _: *mut Mdl, _: u32) -> PhysicalAddress {
    PhysicalAddress { quad_part: 0 }
}
static LOWER_TABLE: Shared<PortWaveRtStreamVtable> = Shared(PortWaveRtStreamVtable {
    unknown: UnknownVtable {
        query_interface: lower_query,
        add_ref: lower_add,
        release: lower_release,
    },
    allocate_pages_for_mdl: pages,
    allocate_contiguous_pages_for_mdl: contiguous,
    map_allocated_pages: map,
    unmap_allocated_pages: unmap,
    free_pages_from_mdl: free_pages,
    get_physical_pages_count: page_count,
    get_physical_page_address: page_address,
});

struct DeviceFixture {
    device: Box<DeviceObjectPrefix>,
    _extension: Box<[usize; 64]>,
}
impl DeviceFixture {
    fn new() -> Self {
        unsafe {
            let mut extension = Box::new([0; 64]);
            let mut device: Box<DeviceObjectPrefix> = Box::new(std::mem::zeroed());
            device.device_extension = extension.as_mut_ptr().cast();
            Self {
                device,
                _extension: extension,
            }
        }
    }
    fn pointer(&mut self) -> *mut DeviceObject {
        (&mut *self.device as *mut DeviceObjectPrefix).cast()
    }
}
unsafe fn remove(device: *mut DeviceObject, minor: u8) {
    let mut stack = IoStackLocationPrefix {
        major_function: IRP_MJ_PNP as u8,
        minor_function: minor,
        flags: 0,
        control: 0,
    };
    let mut irp: IrpPrefixToCurrentStack = std::mem::zeroed();
    irp.current_stack_location = &mut stack;
    assert_eq!(
        dispatch_pnp(device, (&mut irp as *mut IrpPrefixToCurrentStack).cast()),
        OK
    );
}
unsafe fn format() -> [u8; 104] {
    let mut out = [0; 104];
    let mut size = 0;
    assert_eq!(
        descriptors::data_range_intersection(
            true,
            0,
            ptr::addr_of!(descriptors::AUDIO_RANGE.data_range).cast_mut(),
            ptr::addr_of!(descriptors::AUDIO_RANGE.data_range).cast_mut(),
            out.len() as u32,
            out.as_mut_ptr().cast(),
            &mut size
        ),
        OK
    );
    assert_eq!(size, 104);
    out
}
unsafe fn create_stream(
    miniport: Raw,
    lower: &mut FakeStream,
    capture: bool,
    format: &mut [u8; 104],
) -> *mut MiniportWaveRtStream {
    let table = (*miniport.cast::<Miniport>())
        .vtable
        .cast::<MiniportWaveRtVtable>();
    let mut stream = ptr::null_mut();
    assert_eq!(
        ((*table).new_stream)(
            miniport,
            &mut stream,
            (lower as *mut FakeStream).cast(),
            if capture { 3 } else { 0 },
            u8::from(capture),
            format.as_mut_ptr().cast()
        ),
        OK
    );
    stream
}
unsafe fn allocate(stream: *mut MiniportWaveRtStream, expected: NtStatus) -> *mut FakeMdl {
    let mut mdl = ptr::null_mut();
    let mut bytes = 0;
    let mut offset = 1;
    let mut cache = MM_NON_CACHED;
    assert_eq!(
        ((*(*stream).vtable).allocate_audio_buffer)(
            stream.cast(),
            8192,
            &mut mdl,
            &mut bytes,
            &mut offset,
            &mut cache
        ),
        expected
    );
    if expected == OK {
        assert_eq!((bytes, offset, cache), (8192, 0, MM_CACHED));
    } else {
        assert!(mdl.is_null());
        assert_eq!(bytes, 0);
    }
    mdl.cast()
}
unsafe fn set(stream: *mut MiniportWaveRtStream, state: i32) {
    assert_eq!(((*(*stream).vtable).set_state)(stream.cast(), state), OK);
}
unsafe fn cursor(stream: *mut MiniportWaveRtStream) -> u64 {
    let mut out = KsAudioPosition {
        play_offset: 0,
        write_offset: 0,
    };
    assert_eq!(
        ((*(*stream).vtable).get_position)(stream.cast(), &mut out),
        OK
    );
    out.play_offset
}
fn sample(frame: usize) -> u64 {
    0x3f80_0000_bf00_0000 ^ frame as u64
}

#[test]
fn mocked_portcls_vtables_transfer_audio_and_release_every_owned_resource() {
    unsafe {
        // Every setup acquisition point rolls back previously acquired resources.
        for fail in 1..=10 {
            reset(fail);
            let mut device = DeviceFixture::new();
            let pointer = device.pointer();
            assert_eq!(
                start_device(pointer, ptr::null_mut(), ptr::null_mut()),
                NO_MEMORY
            );
            assert!((*adapter_slot(pointer)).is_null());
            assert_clean();
        }
        reset(0);
        let mut device = DeviceFixture::new();
        let pointer = device.pointer();
        let mut driver: DriverObjectFields = std::mem::zeroed();
        let mut registry: UnicodeString = std::mem::zeroed();
        assert_eq!(
            DriverEntry(
                (&mut driver as *mut DriverObjectFields).cast(),
                &mut registry
            ),
            OK
        );
        assert!(driver.major_function[IRP_MJ_PNP].is_some());
        assert_eq!(start_device(pointer, ptr::null_mut(), ptr::null_mut()), OK);
        let wave = with_mock(|m| m.ports[0]) as *mut FakePort;
        let cable = *adapter_slot(pointer);
        let mut format = format();
        let mut render_lower = FakeStream {
            vtable: &LOWER_TABLE.0,
            refs: AtomicU32::new(1),
        };
        let mut capture_lower = FakeStream {
            vtable: &LOWER_TABLE.0,
            refs: AtomicU32::new(1),
        };
        let render = create_stream((*wave).miniport, &mut render_lower, false, &mut format);
        let capture = create_stream((*wave).miniport, &mut capture_lower, true, &mut format);
        with_mock(|m| m.short_mdl = true);
        allocate(render, NO_MEMORY);
        with_mock(|m| m.short_mdl = false);
        with_mock(|m| m.fail_map = true);
        allocate(render, NO_MEMORY);
        with_mock(|m| m.fail_map = false);
        let render_mdl = allocate(render, OK);
        let capture_mdl = allocate(capture, OK);
        for frame in 0..1024 {
            (*render_mdl)
                .buffer
                .cast::<u64>()
                .add(frame)
                .write(sample(frame));
        }
        for stream in [render, capture] {
            set(stream, KSSTATE_ACQUIRE);
            set(stream, KSSTATE_PAUSE);
            set(stream, KSSTATE_RUN);
        }
        TICKS.store(768, Ordering::SeqCst);
        assert_eq!(cursor(capture), 768 * 8);
        for frame in 0..768 {
            let expected = if frame < 512 { 0 } else { sample(frame - 512) };
            assert_eq!(
                (*capture_mdl).buffer.cast::<u64>().add(frame).read(),
                expected
            );
        }
        assert_eq!(cursor(render), 768 * 8);
        LOCAL_IRQL.with(|level| level.set(2));
        assert_eq!(cursor(capture), 768 * 8);
        LOCAL_IRQL.with(|level| level.set(0));
        assert_eq!(HELD_REGIONS.load(Ordering::SeqCst), 0);
        // PortCls pauses active streams before D3 and resumes them after D0.
        for stream in [render, capture] {
            set(stream, KSSTATE_PAUSE);
        }
        power_change(cable.cast(), PowerState { device_state: 4 });
        TICKS.store(48_768, Ordering::SeqCst);
        assert_eq!(cursor(capture), 768 * 8);
        power_change(cable.cast(), PowerState { device_state: 1 });
        for stream in [render, capture] {
            set(stream, KSSTATE_RUN);
        }
        TICKS.store(48_896, Ordering::SeqCst);
        assert_eq!(cursor(capture), 896 * 8);
        // Pause the real PCM-copy function outside the metadata lock. A second
        // GetPosition must return without waiting. FreeAudioBuffer must wait at
        // passive level and must not unmap until the in-flight copy completes.
        let pause = Arc::new(CopyPause {
            state: Mutex::new((false, false)),
            changed: Condvar::new(),
        });
        *PAUSE_COPY.lock().unwrap() = Some(pause.clone());
        TICKS.store(48_960, Ordering::SeqCst);
        let capture_address = capture as usize;
        let copier = std::thread::spawn(move || {
            cursor(capture_address as *mut MiniportWaveRtStream);
        });
        {
            let mut state = pause.state.lock().unwrap();
            while !state.0 {
                state = pause.changed.wait(state).unwrap();
            }
        }
        assert_eq!(cursor(render), 896 * 8); // last committed position, no wait
        let mdl_address = capture_mdl as usize;
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let freer = std::thread::spawn(move || {
            let stream = capture_address as *mut MiniportWaveRtStream;
            ((*(*stream).vtable).free_audio_buffer)(stream.cast(), mdl_address as *mut Mdl, 8192);
            done_tx.send(()).unwrap();
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let pending = {
                let mut guard = lock(cable);
                guard.state().control_pending
            };
            if pending {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "free did not enter control wait"
            );
            std::thread::yield_now();
        }
        assert!(done_rx.try_recv().is_err());
        assert_eq!(with_mock(|m| m.mappings), 2);
        {
            let mut state = pause.state.lock().unwrap();
            state.1 = true;
            pause.changed.notify_all();
        }
        copier.join().unwrap();
        freer.join().unwrap();
        *PAUSE_COPY.lock().unwrap() = None;
        assert_eq!(with_mock(|m| m.mappings), 1);
        // Removal must detach the adapter even while clients retain stream objects.
        remove(pointer, IRP_MN_SURPRISE_REMOVAL);
        assert!((*adapter_slot(pointer)).is_null());
        let mut position = KsAudioPosition {
            play_offset: 0,
            write_offset: 0,
        };
        assert_eq!(
            ((*(*capture).vtable).get_position)(capture.cast(), &mut position),
            BAD_STATE
        );
        remove(pointer, IRP_MN_REMOVE_DEVICE);
        assert_eq!(with_mock(|m| m.dispatches), 2);
        unknown_release(render.cast()); // passive fallback releases its MDL
        LOCAL_IRQL.with(|level| level.set(2));
        unknown_release(capture.cast()); // already freed: no passive wait
        LOCAL_IRQL.with(|level| level.set(0));
        assert_eq!(render_lower.refs.load(Ordering::SeqCst), 1);
        assert_eq!(capture_lower.refs.load(Ordering::SeqCst), 1);
        assert_clean();
        // PnP rebalance releases the old adapter and allows a fresh start.
        assert_eq!(start_device(pointer, ptr::null_mut(), ptr::null_mut()), OK);
        let pnp = with_mock(|m| m.pnp) as *mut PnpPart;
        ((*(*pnp).vtable).pnp_stop)(pnp.cast());
        assert!((*adapter_slot(pointer)).is_null());
        assert_clean();
        assert_eq!(start_device(pointer, ptr::null_mut(), ptr::null_mut()), OK);
        remove(pointer, IRP_MN_REMOVE_DEVICE);
        assert_clean();
    }
}
