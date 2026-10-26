//! Minimal ABI declarations transcribed from the installed Apple SDK declarations.
//! No implementation code from other drivers is used. See drivers/macos/PROVENANCE.md.
use core::ffi::c_void;

pub type Status = i32;
pub type ObjectId = u32;
pub type CfRef = *const c_void;
pub type DriverRef = *mut *const DriverInterface;
pub type HostRef = *const HostInterface;
pub const fn code(bytes: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*bytes)
}
pub const OK: Status = 0;
pub const BAD_OBJECT: Status = code(b"!obj") as i32;
pub const BAD_DEVICE: Status = code(b"!dev") as i32;
pub const BAD_STREAM: Status = code(b"!str") as i32;
pub const BAD_SIZE: Status = code(b"!siz") as i32;
pub const UNKNOWN_PROPERTY: Status = code(b"who?") as i32;
pub const ILLEGAL: Status = code(b"nope") as i32;
pub const UNSUPPORTED: Status = code(b"unop") as i32;
pub const BAD_FORMAT: Status = code(b"!dat") as i32;
pub const GLOBAL: u32 = code(b"glob");
pub const INPUT: u32 = code(b"inpt");
pub const OUTPUT: u32 = code(b"outp");
pub const READ: u32 = code(b"read");
pub const WRITE: u32 = code(b"rite");
pub const PLUGIN: ObjectId = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Uuid {
    pub bytes: [u8; 16],
}
pub const TYPE_UUID: Uuid = Uuid {
    bytes: [
        0x44, 0x3a, 0xba, 0xb8, 0xe7, 0xb3, 0x49, 0x1a, 0xb9, 0x85, 0xbe, 0xb9, 0x18, 0x70, 0x30,
        0xdb,
    ],
};
pub const DRIVER_UUID: Uuid = Uuid {
    bytes: [
        0xee, 0xa5, 0x77, 0x3d, 0xcc, 0x43, 0x49, 0xf1, 0x8e, 0, 0x8f, 0x96, 0xe7, 0xd2, 0x3b, 0x17,
    ],
};
pub const UNKNOWN_UUID: Uuid = Uuid {
    bytes: [0, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46],
};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Address {
    pub selector: u32,
    pub scope: u32,
    pub element: u32,
}

#[repr(C)]
pub struct ClientInfo {
    pub id: u32,
    pub process_id: i32,
    pub native_endian: u8,
    pub bundle_id: CfRef,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SmpteTime {
    pub subframes: i16,
    pub subframe_divisor: i16,
    pub counter: u32,
    pub kind: u32,
    pub flags: u32,
    pub hours: i16,
    pub minutes: i16,
    pub seconds: i16,
    pub frames: i16,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TimeStamp {
    pub sample_time: f64,
    pub host_time: u64,
    pub rate_scalar: f64,
    pub word_clock_time: u64,
    pub smpte: SmpteTime,
    pub flags: u32,
    pub reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CycleInfo {
    pub cycle: u64,
    pub nominal_frames: u32,
    pub current: TimeStamp,
    pub input: TimeStamp,
    pub output: TimeStamp,
    pub main_ticks_per_frame: f64,
    pub device_ticks_per_frame: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Format {
    pub sample_rate: f64,
    pub format_id: u32,
    pub flags: u32,
    pub bytes_per_packet: u32,
    pub frames_per_packet: u32,
    pub bytes_per_frame: u32,
    pub channels: u32,
    pub bits_per_channel: u32,
    pub reserved: u32,
}
pub const FORMAT: Format = Format {
    sample_rate: 48_000.0,
    format_id: code(b"lpcm"),
    flags: 9,
    bytes_per_packet: 8,
    frames_per_packet: 1,
    bytes_per_frame: 8,
    channels: 2,
    bits_per_channel: 32,
    reserved: 0,
};
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Range {
    pub minimum: f64,
    pub maximum: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RangedFormat {
    pub format: Format,
    pub range: Range,
}

#[repr(C)]
pub struct HostInterface {
    pub properties_changed: unsafe extern "C" fn(HostRef, ObjectId, u32, *const Address) -> Status,
    pub copy_from_storage: unsafe extern "C" fn(HostRef, CfRef, *mut CfRef) -> Status,
    pub write_to_storage: unsafe extern "C" fn(HostRef, CfRef, CfRef) -> Status,
    pub delete_from_storage: unsafe extern "C" fn(HostRef, CfRef) -> Status,
    pub request_configuration: unsafe extern "C" fn(HostRef, ObjectId, u64, *mut c_void) -> Status,
}

// Field order is the complete AudioServerPlugInDriverInterface, including IUnknown.
#[repr(C)]
pub struct DriverInterface {
    pub reserved: *const c_void,
    pub query_interface: unsafe extern "C" fn(*mut c_void, Uuid, *mut *mut c_void) -> i32,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub initialize: unsafe extern "C" fn(DriverRef, HostRef) -> Status,
    pub create_device:
        unsafe extern "C" fn(DriverRef, CfRef, *const ClientInfo, *mut ObjectId) -> Status,
    pub destroy_device: unsafe extern "C" fn(DriverRef, ObjectId) -> Status,
    pub add_client: unsafe extern "C" fn(DriverRef, ObjectId, *const ClientInfo) -> Status,
    pub remove_client: unsafe extern "C" fn(DriverRef, ObjectId, *const ClientInfo) -> Status,
    pub perform_configuration:
        unsafe extern "C" fn(DriverRef, ObjectId, u64, *mut c_void) -> Status,
    pub abort_configuration: unsafe extern "C" fn(DriverRef, ObjectId, u64, *mut c_void) -> Status,
    pub has_property: unsafe extern "C" fn(DriverRef, ObjectId, i32, *const Address) -> u8,
    pub is_property_settable:
        unsafe extern "C" fn(DriverRef, ObjectId, i32, *const Address, *mut u8) -> Status,
    pub get_property_size: unsafe extern "C" fn(
        DriverRef,
        ObjectId,
        i32,
        *const Address,
        u32,
        *const c_void,
        *mut u32,
    ) -> Status,
    pub get_property: unsafe extern "C" fn(
        DriverRef,
        ObjectId,
        i32,
        *const Address,
        u32,
        *const c_void,
        u32,
        *mut u32,
        *mut c_void,
    ) -> Status,
    pub set_property: unsafe extern "C" fn(
        DriverRef,
        ObjectId,
        i32,
        *const Address,
        u32,
        *const c_void,
        u32,
        *const c_void,
    ) -> Status,
    pub start_io: unsafe extern "C" fn(DriverRef, ObjectId, u32) -> Status,
    pub stop_io: unsafe extern "C" fn(DriverRef, ObjectId, u32) -> Status,
    pub zero_timestamp:
        unsafe extern "C" fn(DriverRef, ObjectId, u32, *mut f64, *mut u64, *mut u64) -> Status,
    pub will_do_io: unsafe extern "C" fn(DriverRef, ObjectId, u32, u32, *mut u8, *mut u8) -> Status,
    pub begin_io:
        unsafe extern "C" fn(DriverRef, ObjectId, u32, u32, u32, *const CycleInfo) -> Status,
    pub do_io: unsafe extern "C" fn(
        DriverRef,
        ObjectId,
        ObjectId,
        u32,
        u32,
        u32,
        *const CycleInfo,
        *mut c_void,
        *mut c_void,
    ) -> Status,
    pub end_io:
        unsafe extern "C" fn(DriverRef, ObjectId, u32, u32, u32, *const CycleInfo) -> Status,
}
// Immutable vtables only contain function addresses and a null reserved pointer.
unsafe impl Sync for DriverInterface {}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub fn CFUUIDGetUUIDBytes(uuid: CfRef) -> Uuid;
    pub fn CFStringCreateWithBytes(
        allocator: CfRef,
        bytes: *const u8,
        count: isize,
        encoding: u32,
        external: u8,
    ) -> CfRef;
    pub fn CFEqual(a: CfRef, b: CfRef) -> u8;
    pub fn CFRelease(object: CfRef);
}
#[repr(C)]
#[derive(Default)]
pub struct Timebase {
    pub numerator: u32,
    pub denominator: u32,
}
extern "C" {
    pub fn mach_absolute_time() -> u64;
    pub fn mach_timebase_info(info: *mut Timebase) -> i32;
}
