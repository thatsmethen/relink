//! Independently authored, fixed-format macOS virtual loopback AudioServerPlugIn.
//! The ABI is public for native-host conformance testing; it is not a Rust API.
#![cfg(target_os = "macos")]

pub mod abi;
mod device;
mod properties;
mod ring;

use abi::*;
use core::{
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicPtr, AtomicU32, Ordering},
};

static HOST: AtomicPtr<HostInterface> = AtomicPtr::new(ptr::null_mut());
static REFERENCES: AtomicU32 = AtomicU32::new(0);
static NUMERATOR: AtomicU32 = AtomicU32::new(0);
static DENOMINATOR: AtomicU32 = AtomicU32::new(0);

struct DriverSlot(*const DriverInterface);
// CFPlugIn uses a pointer to an immutable vtable pointer. The host does not mutate it.
unsafe impl Sync for DriverSlot {}
static DRIVER_SLOT: DriverSlot = DriverSlot(&INTERFACE);
fn driver_ref() -> DriverRef {
    ptr::addr_of!(DRIVER_SLOT.0).cast_mut()
}
fn valid_driver(driver: DriverRef) -> bool {
    driver == driver_ref()
}

static INTERFACE: DriverInterface = DriverInterface {
    reserved: ptr::null(),
    query_interface,
    add_ref,
    release,
    initialize,
    create_device,
    destroy_device,
    add_client,
    remove_client,
    perform_configuration: configuration,
    abort_configuration: configuration,
    has_property: properties::has,
    is_property_settable: properties::settable,
    get_property_size: properties::size,
    get_property: properties::get,
    set_property: properties::set,
    start_io,
    stop_io,
    zero_timestamp,
    will_do_io,
    begin_io: boundary_io,
    do_io,
    end_io: boundary_io,
};

/// CFPlugIn factory. The requested type must be kAudioServerPlugInTypeUUID.
///
/// # Safety
/// `requested_type` must be null or a valid, live CFUUIDRef supplied by CFPlugIn.
#[no_mangle]
pub unsafe extern "C" fn RaveLinkHALFactory(_: CfRef, requested_type: CfRef) -> *mut c_void {
    if requested_type.is_null() || CFUUIDGetUUIDBytes(requested_type) != TYPE_UUID {
        return ptr::null_mut();
    }
    add_ref(driver_ref().cast());
    driver_ref().cast()
}

unsafe extern "C" fn query_interface(
    driver: *mut c_void,
    uuid: Uuid,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return 0x8000_0005u32 as i32;
    }
    ptr::write_unaligned(out, ptr::null_mut());
    if !valid_driver(driver.cast()) {
        return 0x8000_0003u32 as i32;
    }
    if uuid != DRIVER_UUID && uuid != UNKNOWN_UUID {
        return 0x8000_0004u32 as i32;
    }
    add_ref(driver);
    ptr::write_unaligned(out, driver);
    OK
}
unsafe extern "C" fn add_ref(driver: *mut c_void) -> u32 {
    if !valid_driver(driver.cast()) {
        return 0;
    }
    REFERENCES
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            Some(n.saturating_add(1))
        })
        .unwrap_or(0)
        .saturating_add(1)
}
unsafe extern "C" fn release(driver: *mut c_void) -> u32 {
    if !valid_driver(driver.cast()) {
        return 0;
    }
    // This static singleton remains allocated when its COM count reaches zero.
    REFERENCES
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            Some(n.saturating_sub(1))
        })
        .unwrap_or(0)
        .saturating_sub(1)
}
unsafe extern "C" fn initialize(driver: DriverRef, host: HostRef) -> Status {
    if !valid_driver(driver) {
        return BAD_OBJECT;
    }
    if host.is_null() {
        return ILLEGAL;
    }
    let existing = HOST.load(Ordering::Acquire);
    if !existing.is_null() {
        return if ptr::eq(existing, host) { OK } else { ILLEGAL };
    }
    let mut timebase = Timebase::default();
    if mach_timebase_info(&mut timebase) != 0
        || timebase.numerator == 0
        || timebase.denominator == 0
    {
        return ILLEGAL;
    }
    NUMERATOR.store(timebase.numerator, Ordering::Relaxed);
    DENOMINATOR.store(timebase.denominator, Ordering::Relaxed);
    let now = mach_absolute_time();
    for d in &device::DEVICES {
        let _ = d
            .anchor
            .compare_exchange(0, now, Ordering::Release, Ordering::Relaxed);
    }
    match HOST.compare_exchange(
        ptr::null_mut(),
        host.cast_mut(),
        Ordering::Release,
        Ordering::Acquire,
    ) {
        Ok(_) => OK,
        Err(h) if ptr::eq(h, host) => OK,
        Err(_) => ILLEGAL,
    }
}
unsafe extern "C" fn create_device(
    driver: DriverRef,
    _: CfRef,
    _: *const ClientInfo,
    out: *mut ObjectId,
) -> Status {
    if !valid_driver(driver) {
        return BAD_OBJECT;
    }
    if out.is_null() {
        return ILLEGAL;
    }
    ptr::write_unaligned(out, 0);
    UNSUPPORTED
}
unsafe extern "C" fn destroy_device(driver: DriverRef, id: ObjectId) -> Status {
    if !valid_driver(driver) {
        BAD_OBJECT
    } else if device::find(id).is_none() {
        BAD_DEVICE
    } else {
        UNSUPPORTED
    }
}
unsafe extern "C" fn configuration(
    driver: DriverRef,
    id: ObjectId,
    _: u64,
    _: *mut c_void,
) -> Status {
    destroy_device(driver, id)
}
unsafe extern "C" fn add_client(
    driver: DriverRef,
    id: ObjectId,
    client: *const ClientInfo,
) -> Status {
    if !valid_driver(driver) {
        return BAD_OBJECT;
    }
    let Some(d) = device::find(id) else {
        return BAD_DEVICE;
    };
    if client.is_null() {
        return ILLEGAL;
    }
    let client = ptr::read_unaligned(client);
    if client.native_endian == 0 {
        return UNSUPPORTED;
    }
    d.add_client(client.id)
}
unsafe extern "C" fn remove_client(
    driver: DriverRef,
    id: ObjectId,
    client: *const ClientInfo,
) -> Status {
    if !valid_driver(driver) {
        return BAD_OBJECT;
    }
    let Some(d) = device::find(id) else {
        return BAD_DEVICE;
    };
    if client.is_null() {
        return ILLEGAL;
    }
    d.remove_client(ptr::read_unaligned(client).id)
}
unsafe fn running_changed(id: ObjectId) {
    let host = HOST.load(Ordering::Acquire);
    if !host.is_null() {
        let address = Address {
            selector: code(b"goin"),
            scope: GLOBAL,
            element: 0,
        };
        ((*host).properties_changed)(host, id, 1, &address);
    }
}
unsafe extern "C" fn start_io(driver: DriverRef, id: ObjectId, client: u32) -> Status {
    if !valid_driver(driver) {
        return BAD_OBJECT;
    }
    if HOST.load(Ordering::Acquire).is_null() {
        return ILLEGAL;
    }
    let Some(d) = device::find(id) else {
        return BAD_DEVICE;
    };
    match d.start(client) {
        Ok(changed) => {
            if changed {
                running_changed(id);
            }
            OK
        }
        Err(e) => e,
    }
}
unsafe extern "C" fn stop_io(driver: DriverRef, id: ObjectId, client: u32) -> Status {
    if !valid_driver(driver) {
        return BAD_OBJECT;
    }
    let Some(d) = device::find(id) else {
        return BAD_DEVICE;
    };
    match d.stop(client) {
        Ok(changed) => {
            if changed {
                running_changed(id);
            }
            OK
        }
        Err(e) => e,
    }
}
unsafe extern "C" fn zero_timestamp(
    driver: DriverRef,
    id: ObjectId,
    _: u32,
    sample: *mut f64,
    host: *mut u64,
    seed: *mut u64,
) -> Status {
    if !valid_driver(driver) {
        return BAD_OBJECT;
    }
    let Some(d) = device::find(id) else {
        return BAD_DEVICE;
    };
    if sample.is_null() || host.is_null() || seed.is_null() {
        return ILLEGAL;
    }
    if HOST.load(Ordering::Acquire).is_null() {
        return ILLEGAL;
    }
    let (sample_time, host_time) = device::timestamp(
        mach_absolute_time(),
        d.anchor.load(Ordering::Acquire),
        NUMERATOR.load(Ordering::Relaxed),
        DENOMINATOR.load(Ordering::Relaxed),
    );
    ptr::write_unaligned(sample, sample_time);
    ptr::write_unaligned(host, host_time);
    ptr::write_unaligned(seed, 1);
    OK
}
unsafe extern "C" fn will_do_io(
    driver: DriverRef,
    id: ObjectId,
    _: u32,
    operation: u32,
    will: *mut u8,
    in_place: *mut u8,
) -> Status {
    if !valid_driver(driver) {
        return BAD_OBJECT;
    }
    if device::find(id).is_none() {
        return BAD_DEVICE;
    }
    if will.is_null() || in_place.is_null() {
        return ILLEGAL;
    }
    ptr::write(will, u8::from(operation == READ || operation == WRITE));
    ptr::write(in_place, 1);
    OK
}
unsafe extern "C" fn boundary_io(
    driver: DriverRef,
    id: ObjectId,
    _: u32,
    _: u32,
    frames: u32,
    cycle: *const CycleInfo,
) -> Status {
    if !valid_driver(driver) {
        BAD_OBJECT
    } else if device::find(id).is_none() {
        BAD_DEVICE
    } else if cycle.is_null() || frames > device::MAX_IO_FRAMES {
        ILLEGAL
    } else {
        OK
    }
}
fn sample_index(time: TimeStamp) -> Option<i64> {
    if time.flags & 1 == 0
        || !time.sample_time.is_finite()
        || time.sample_time.fract() != 0.0
        || time.sample_time.abs() > (1u64 << 53) as f64
    {
        return None;
    }
    Some(time.sample_time as i64)
}
unsafe extern "C" fn do_io(
    driver: DriverRef,
    id: ObjectId,
    stream: ObjectId,
    _: u32,
    operation: u32,
    frames: u32,
    cycle: *const CycleInfo,
    main: *mut c_void,
    _: *mut c_void,
) -> Status {
    if !valid_driver(driver) {
        return BAD_OBJECT;
    }
    let Some(d) = device::find(id) else {
        return BAD_DEVICE;
    };
    if operation != READ && operation != WRITE {
        return UNSUPPORTED;
    }
    if (operation == READ && stream != d.input) || (operation == WRITE && stream != d.output) {
        return BAD_STREAM;
    }
    if cycle.is_null() || frames > device::MAX_IO_FRAMES || (frames > 0 && main.is_null()) {
        return ILLEGAL;
    }
    if d.running.load(Ordering::Acquire) == 0 {
        return ILLEGAL;
    }
    let cycle = ptr::read_unaligned(cycle);
    let Some(start) = sample_index(if operation == READ {
        cycle.input
    } else {
        cycle.output
    }) else {
        return ILLEGAL;
    };
    for index in 0..frames as usize {
        let at = start + index as i64;
        let buffer = main.cast::<f32>().add(index * 2);
        if operation == WRITE {
            if at >= 0 {
                d.ring.write(
                    at as u64,
                    ptr::read_unaligned(buffer),
                    ptr::read_unaligned(buffer.add(1)),
                );
            }
        } else {
            let source = at - device::LOOPBACK_DELAY as i64;
            let samples = if source < 0 {
                [0.0; 2]
            } else {
                d.ring.read(source as u64)
            };
            ptr::write_unaligned(buffer, samples[0]);
            ptr::write_unaligned(buffer.add(1), samples[1]);
        }
    }
    OK
}

#[cfg(test)]
mod tests;
