//! Minimal, independently transcribed Windows WDK ABI declarations.
//!
//! Layout authority: Microsoft.Windows.WDK.x64 10.0.26100.6584, headers under
//! `c/Include/10.0.26100.0/km/{portcls.h,unknown.h,wdm.h}` and the matching SDK
//! `shared/{ks.h,ksmedia.h,mmreg.h,ntdef.h}`. No driver implementation is used.
//! PortCls interfaces are COM-shaped C++ interfaces. Their inherited methods
//! are represented explicitly in declaration order with an explicit `this`.
#![allow(dead_code)]

use core::ffi::c_void;

pub type NtStatus = i32;
pub type Raw = *mut c_void;
pub type Mdl = c_void;
pub type DeviceObject = c_void;
pub type DriverObject = c_void;
pub type Irp = c_void;
pub type ResourceList = c_void;
pub type DeviceCapabilities = c_void;
/// Storage layout of WDK KEVENT (DISPATCHER_HEADER), initialized only by KeInitializeEvent.
#[repr(C, align(8))]
pub struct KernelEvent {
    pub opaque: [u64; 3],
}
pub type MemoryCachingType = i32;
pub type KsState = i32;

pub const IRP_MJ_PNP: usize = 0x1b;
pub const IRP_MJ_MAXIMUM_FUNCTION: usize = 0x1b;
pub const IRP_MN_REMOVE_DEVICE: u8 = 0x02;
pub const IRP_MN_STOP_DEVICE: u8 = 0x04;
pub const IRP_MN_SURPRISE_REMOVAL: u8 = 0x17;
pub const PORT_CLASS_DEVICE_EXTENSION_SIZE: usize = 64 * core::mem::size_of::<usize>();
pub type DriverDispatch = unsafe extern "system" fn(*mut DeviceObject, *mut Irp) -> NtStatus;

/// WDK DRIVER_OBJECT layout. Opaque members are pointers only; use the documented
/// dispatch member without interpreting PortCls's other driver state.
#[repr(C)]
pub struct DriverObjectFields {
    pub object_type: i16,
    pub size: i16,
    pub device_object: *mut DeviceObject,
    pub flags: u32,
    pub driver_start: Raw,
    pub driver_size: u32,
    pub driver_section: Raw,
    pub driver_extension: Raw,
    pub driver_name: UnicodeString,
    pub hardware_database: *mut UnicodeString,
    pub fast_io_dispatch: Raw,
    pub driver_init: Raw,
    pub driver_start_io: Raw,
    pub driver_unload: Raw,
    pub major_function: [Option<DriverDispatch>; IRP_MJ_MAXIMUM_FUNCTION + 1],
}

/// Prefix of the system-owned DEVICE_OBJECT, through DeviceExtension. This is
/// not an allocation-sized DEVICE_OBJECT and must never be created as one.
#[repr(C, align(16))]
pub struct DeviceObjectPrefix {
    pub object_type: i16,
    pub size: u16,
    pub reference_count: i32,
    pub driver_object: *mut DriverObject,
    pub next_device: *mut DeviceObject,
    pub attached_device: *mut DeviceObject,
    pub current_irp: *mut Irp,
    pub timer: Raw,
    pub flags: u32,
    pub characteristics: u32,
    pub vpb: Raw,
    pub device_extension: Raw,
}

#[repr(C)]
pub union IoStatusValue {
    pub status: NtStatus,
    pub pointer: Raw,
}
#[repr(C)]
pub struct IoStatusBlock {
    pub value: IoStatusValue,
    pub information: usize,
}
#[repr(C)]
pub struct ListEntry {
    pub forward: Raw,
    pub backward: Raw,
}

/// Prefix needed by the Rust equivalent of WDK's inline
/// IoGetCurrentIrpStackLocation. Union storage preceding that member is kept
/// opaque. This is not an allocation-sized IRP and must never be allocated.
#[repr(C, align(16))]
pub struct IrpPrefixToCurrentStack {
    pub object_type: i16,
    pub size: u16,
    pub mdl_address: *mut Mdl,
    pub flags: u32,
    pub associated_irp: Raw,
    pub thread_list_entry: ListEntry,
    pub io_status: IoStatusBlock,
    pub requestor_mode: i8,
    pub pending_returned: u8,
    pub stack_count: i8,
    pub current_location: i8,
    pub cancel: u8,
    pub cancel_irql: u8,
    pub apc_environment: i8,
    pub allocation_flags: u8,
    pub user_iosb_or_ring_context: Raw,
    pub user_event: Raw,
    pub asynchronous_parameters: [Raw; 2],
    pub cancel_routine: Raw,
    pub user_buffer: Raw,
    pub tail_driver_context: [Raw; 4],
    pub tail_thread: Raw,
    pub tail_auxiliary_buffer: Raw,
    pub tail_list_entry: ListEntry,
    pub current_stack_location: *mut IoStackLocationPrefix,
}

/// Only the leading dispatch bytes of the system-owned IO_STACK_LOCATION.
#[repr(C)]
pub struct IoStackLocationPrefix {
    pub major_function: u8,
    pub minor_function: u8,
    pub flags: u8,
    pub control: u8,
}

/// # Safety
/// `irp` must be a live system IRP owned by the current dispatch callback. This
/// mirrors the field access in WDK's inline IoGetCurrentIrpStackLocation; it
/// does not change the stack location or take ownership of any IRP storage.
pub unsafe fn io_get_current_irp_stack_location(irp: *mut Irp) -> *mut IoStackLocationPrefix {
    core::ptr::read(core::ptr::addr_of!(
        (*irp.cast::<IrpPrefixToCurrentStack>()).current_stack_location
    ))
}

pub const MM_NON_CACHED: MemoryCachingType = 0;
pub const MM_CACHED: MemoryCachingType = 1;
pub const MM_WRITE_COMBINED: MemoryCachingType = 2;
pub const KSSTATE_STOP: KsState = 0;
pub const KSSTATE_ACQUIRE: KsState = 1;
pub const KSSTATE_PAUSE: KsState = 2;
pub const KSSTATE_RUN: KsState = 3;
pub const KSPIN_DATAFLOW_IN: i32 = 1;
pub const KSPIN_DATAFLOW_OUT: i32 = 2;
pub const KSPIN_COMMUNICATION_NONE: i32 = 0;
pub const KSPIN_COMMUNICATION_SINK: i32 = 1;
pub const KSPIN_COMMUNICATION_SOURCE: i32 = 2;
pub const KSPIN_COMMUNICATION_BOTH: i32 = 3;
pub const KSPIN_COMMUNICATION_BRIDGE: i32 = 4;
pub const PCFILTER_NODE: u32 = u32::MAX;
pub const PC_REBALANCE_NOT_SUPPORTED: i32 = 0;
pub const PC_REBALANCE_REMOVE_SUBDEVICES: i32 = 1;
pub const STATUS_SUCCESS: NtStatus = 0;
pub const STATUS_INVALID_PARAMETER: NtStatus = 0xc000000du32 as i32;
pub const STATUS_BUFFER_OVERFLOW: NtStatus = 0x80000005u32 as i32;
pub const STATUS_BUFFER_TOO_SMALL: NtStatus = 0xc0000023u32 as i32;
pub const STATUS_NO_MATCH: NtStatus = 0xc0000272u32 as i32;
pub const STATUS_NOT_SUPPORTED: NtStatus = 0xc00000bbu32 as i32;
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;
pub const KSAUDIO_SPEAKER_STEREO: u32 = 3;
pub const KSNODEPIN_STANDARD_IN: u32 = 1;
pub const KSNODEPIN_STANDARD_OUT: u32 = 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }
}

pub const IID_IUNKNOWN: Guid = Guid::new(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_IMINIPORT: Guid = Guid::new(
    0xb4c90a24,
    0x5791,
    0x11d0,
    [0x86, 0xf9, 0, 0xa0, 0xc9, 0x11, 0xb5, 0x44],
);
pub const IID_IPORT: Guid = Guid::new(
    0xb4c90a25,
    0x5791,
    0x11d0,
    [0x86, 0xf9, 0, 0xa0, 0xc9, 0x11, 0xb5, 0x44],
);
pub const IID_IPORT_TOPOLOGY: Guid = Guid::new(
    0xb4c90a30,
    0x5791,
    0x11d0,
    [0x86, 0xf9, 0, 0xa0, 0xc9, 0x11, 0xb5, 0x44],
);
pub const IID_IMINIPORT_TOPOLOGY: Guid = Guid::new(
    0xb4c90a31,
    0x5791,
    0x11d0,
    [0x86, 0xf9, 0, 0xa0, 0xc9, 0x11, 0xb5, 0x44],
);
pub const IID_IPORT_WAVERT: Guid = Guid::new(
    0x339ff909,
    0x68a9,
    0x4310,
    [0xb0, 0x9b, 0x27, 0x4e, 0x96, 0xee, 0x4c, 0xbd],
);
pub const IID_IPORT_WAVERT_STREAM: Guid = Guid::new(
    0x1809ce5a,
    0x64bc,
    0x4e62,
    [0xbd, 0x7d, 0x95, 0xbc, 0xe4, 0x3d, 0xe3, 0x93],
);
pub const IID_IMINIPORT_WAVERT: Guid = Guid::new(
    0x0f9fc4d6,
    0x6061,
    0x4f3c,
    [0xb1, 0xfc, 0x07, 0x5e, 0x35, 0xf7, 0x96, 0x0a],
);
pub const IID_IMINIPORT_WAVERT_STREAM: Guid = Guid::new(
    0x000ac9ab,
    0xfaab,
    0x4f3d,
    [0x94, 0x55, 0x6f, 0xf8, 0x30, 0x6a, 0x74, 0xa0],
);
pub const IID_IADAPTER_POWER_MANAGEMENT: Guid = Guid::new(
    0x793417d0,
    0x35fe,
    0x11d1,
    [0xad, 0x08, 0, 0xa0, 0xc9, 0x0a, 0xb1, 0xb0],
);
pub const IID_IMINIPORT_PNP_NOTIFY: Guid = Guid::new(
    0x6b735365,
    0x9487,
    0x464c,
    [0x93, 0xe3, 0xfa, 0x2c, 0x63, 0x91, 0xd5, 0xa4],
);
pub const CLSID_PORT_TOPOLOGY: Guid = Guid::new(
    0xb4c90a32,
    0x5791,
    0x11d0,
    [0x86, 0xf9, 0, 0xa0, 0xc9, 0x11, 0xb5, 0x44],
);
pub const CLSID_PORT_WAVERT: Guid = Guid::new(
    0xcc9be57a,
    0xeb9e,
    0x42b4,
    [0x94, 0xfc, 0x0c, 0xad, 0x3d, 0xbc, 0xe7, 0xfa],
);
pub const IID_IUNREGISTER_SUBDEVICE: Guid = Guid::new(
    0x16738177,
    0xe199,
    0x41f9,
    [0x9a, 0x87, 0xab, 0xb2, 0xa5, 0x43, 0x2f, 0x21],
);
pub const IID_IUNREGISTER_PHYSICAL_CONNECTION: Guid = Guid::new(
    0x6c38e231,
    0x2a0d,
    0x428d,
    [0x81, 0xf8, 0x07, 0xcc, 0x42, 0x8b, 0xb9, 0xa4],
);
pub const KSDATAFORMAT_TYPE_AUDIO: Guid = Guid::new(
    0x73647561,
    0,
    0x0010,
    [0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71],
);
pub const KSDATAFORMAT_SUBTYPE_PCM: Guid =
    Guid::new(1, 0, 0x0010, [0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]);
pub const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: Guid =
    Guid::new(3, 0, 0x0010, [0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]);
pub const KSDATAFORMAT_SUBTYPE_ANALOG: Guid = Guid::new(
    0x6dba3190,
    0x67bd,
    0x11cf,
    [0xa0, 0xf7, 0, 0x20, 0xaf, 0xd1, 0x56, 0xe4],
);
pub const KSDATAFORMAT_SPECIFIER_WAVEFORMATEX: Guid = Guid::new(
    0x05589f81,
    0xc356,
    0x11ce,
    [0xbf, 0x01, 0, 0xaa, 0, 0x55, 0x59, 0x5a],
);
pub const KSDATAFORMAT_SPECIFIER_NONE: Guid = Guid::new(
    0x0f6417d6,
    0xc318,
    0x11d0,
    [0xa4, 0x3f, 0, 0xa0, 0xc9, 0x22, 0x31, 0x96],
);
pub const KSCATEGORY_AUDIO: Guid = Guid::new(
    0x6994ad04,
    0x93ef,
    0x11d0,
    [0xa3, 0xcc, 0, 0xa0, 0xc9, 0x22, 0x31, 0x96],
);
pub const KSCATEGORY_RENDER: Guid = Guid::new(
    0x65e8773e,
    0x8f56,
    0x11d0,
    [0xa3, 0xb9, 0, 0xa0, 0xc9, 0x22, 0x31, 0x96],
);
pub const KSCATEGORY_CAPTURE: Guid = Guid::new(
    0x65e8773d,
    0x8f56,
    0x11d0,
    [0xa3, 0xb9, 0, 0xa0, 0xc9, 0x22, 0x31, 0x96],
);
pub const KSCATEGORY_TOPOLOGY: Guid = Guid::new(
    0xdda54a40,
    0x1e4c,
    0x11d1,
    [0xa0, 0x50, 0x40, 0x57, 0x05, 0xc1, 0, 0],
);
pub const KSNODETYPE_MICROPHONE: Guid = Guid::new(
    0xdff21be1,
    0xf70f,
    0x11d0,
    [0xb9, 0x17, 0, 0xa0, 0xc9, 0x22, 0x31, 0x96],
);
pub const KSNODETYPE_SPEAKER: Guid = Guid::new(
    0xdff21ce1,
    0xf70f,
    0x11d0,
    [0xb9, 0x17, 0, 0xa0, 0xc9, 0x22, 0x31, 0x96],
);
pub const KSNODETYPE_LINE_CONNECTOR: Guid = Guid::new(
    0xdff21fe3,
    0xf70f,
    0x11d0,
    [0xb9, 0x17, 0, 0xa0, 0xc9, 0x22, 0x31, 0x96],
);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LargeIntegerParts {
    pub low_part: u32,
    pub high_part: i32,
}

/// A by-value Windows LARGE_INTEGER, also typedef'd PHYSICAL_ADDRESS.
#[repr(C)]
#[derive(Clone, Copy)]
pub union PhysicalAddress {
    pub parts: LargeIntegerParts,
    pub quad_part: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union PowerState {
    pub system_state: i32,
    pub device_state: i32,
}

#[repr(C)]
pub struct UnicodeString {
    pub length: u16,
    pub maximum_length: u16,
    pub buffer: *mut u16,
}

/// Flattened named arm of KSDATAFORMAT's anonymous union. The original union's
/// LONGLONG arm requires eight-byte alignment, including for KSDATARANGE_AUDIO.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug)]
pub struct KsDataFormat {
    pub format_size: u32,
    pub flags: u32,
    pub sample_size: u32,
    pub reserved: u32,
    pub major_format: Guid,
    pub sub_format: Guid,
    pub specifier: Guid,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct KsDataRangeAudio {
    pub data_range: KsDataFormat,
    pub maximum_channels: u32,
    pub minimum_bits_per_sample: u32,
    pub maximum_bits_per_sample: u32,
    pub minimum_sample_frequency: u32,
    pub maximum_sample_frequency: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct WaveFormatEx {
    pub format_tag: u16,
    pub channels: u16,
    pub samples_per_sec: u32,
    pub avg_bytes_per_sec: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub cb_size: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct WaveFormatExtensible {
    pub format: WaveFormatEx,
    /// The shared u16 storage for valid bits, samples per block, or reserved.
    pub samples: u16,
    pub channel_mask: u32,
    pub sub_format: Guid,
}

// ksmedia.h packs the combined KSDATAFORMAT_WAVEFORMATEX structures to one
// byte. Serialize the 64-byte KsDataFormat followed by these packed payloads;
// do not put the explicitly aligned Rust KsDataFormat in a packed Rust struct.
pub const KS_WAVEFORMATEX_SIZE: usize = 64 + 18;
pub const KS_WAVEFORMATEXTENSIBLE_SIZE: usize = 64 + 40;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KsAudioPosition {
    pub play_offset: u64,
    pub write_offset: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KsRtAudioHwLatency {
    pub fifo_size: u32,
    pub chipset_delay: u32,
    pub codec_delay: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KsRtAudioHwRegister {
    pub register: Raw,
    pub width: u32,
    pub numerator: u64,
    pub denominator: u64,
    pub accuracy: u32,
}

#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct KsIdentifier {
    pub set: Guid,
    pub id: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KsConstrainedDataRanges {
    pub count: u32,
    pub ranges: *mut *mut KsDataFormat,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union KsPinDescriptorTail {
    pub reserved: i64,
    pub constrained: KsConstrainedDataRanges,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KsPinDescriptor {
    pub interfaces_count: u32,
    pub interfaces: *const KsIdentifier,
    pub mediums_count: u32,
    pub mediums: *const KsIdentifier,
    pub data_ranges_count: u32,
    pub data_ranges: *const *mut KsDataFormat,
    pub data_flow: i32,
    pub communication: i32,
    pub category: *const Guid,
    pub name: *const Guid,
    pub tail: KsPinDescriptorTail,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PcConnectionDescriptor {
    pub from_node: u32,
    pub from_node_pin: u32,
    pub to_node: u32,
    pub to_node_pin: u32,
}

pub const KSNODETYPE_DAC: Guid = Guid::new(
    0x507ae360,
    0xc554,
    0x11d0,
    [0x8a, 0x2b, 0, 0xa0, 0xc9, 0x25, 0x5a, 0xc1],
);
pub const KSNODETYPE_ADC: Guid = Guid::new(
    0x4d837fe0,
    0xc555,
    0x11d0,
    [0x8a, 0x2b, 0, 0xa0, 0xc9, 0x25, 0x5a, 0xc1],
);

/// Full Windows 8+ DEVICE_DESCRIPTION, including the VERSION3 tail.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceDescription {
    pub version: u32,
    pub master: u8,
    pub scatter_gather: u8,
    pub demand_mode: u8,
    pub auto_initialize: u8,
    pub dma_32_bit_addresses: u8,
    pub ignore_count: u8,
    pub reserved1: u8,
    pub dma_64_bit_addresses: u8,
    pub bus_number: u32,
    pub dma_channel: u32,
    pub interface_type: i32,
    pub dma_width: i32,
    pub dma_speed: i32,
    pub maximum_length: u32,
    pub dma_port: u32,
    pub dma_address_width: u32,
    pub dma_controller_instance: u32,
    pub dma_request_line: u32,
    pub device_address: PhysicalAddress,
}

#[repr(C)]
pub struct Unknown {
    pub vtable: *const UnknownVtable,
}

#[repr(C)]
pub struct UnknownVtable {
    pub query_interface: unsafe extern "system" fn(Raw, *const Guid, *mut Raw) -> NtStatus,
    pub add_ref: unsafe extern "system" fn(Raw) -> u32,
    pub release: unsafe extern "system" fn(Raw) -> u32,
}

#[repr(C)]
pub struct MiniportVtable {
    pub unknown: UnknownVtable,
    pub get_description: unsafe extern "system" fn(Raw, *mut *mut PcFilterDescriptor) -> NtStatus,
    pub data_range_intersection: unsafe extern "system" fn(
        Raw,
        u32,
        *mut KsDataFormat,
        *mut KsDataFormat,
        u32,
        Raw,
        *mut u32,
    ) -> NtStatus,
}

#[repr(C)]
pub struct MiniportWaveRtVtable {
    pub miniport: MiniportVtable,
    pub init:
        unsafe extern "system" fn(Raw, *mut Unknown, *mut ResourceList, *mut Port) -> NtStatus,
    pub new_stream: unsafe extern "system" fn(
        Raw,
        *mut *mut MiniportWaveRtStream,
        *mut PortWaveRtStream,
        u32,
        u8,
        *mut KsDataFormat,
    ) -> NtStatus,
    pub get_device_description: unsafe extern "system" fn(Raw, *mut DeviceDescription) -> NtStatus,
}

#[repr(C)]
pub struct MiniportTopologyVtable {
    pub miniport: MiniportVtable,
    pub init:
        unsafe extern "system" fn(Raw, *mut Unknown, *mut ResourceList, *mut Port) -> NtStatus,
}

/// IPortWaveRT and IPortTopology add no methods to IPort.
#[repr(C)]
pub struct Port {
    pub vtable: *const PortVtable,
}

#[repr(C)]
pub struct PortVtable {
    pub unknown: UnknownVtable,
    pub init: unsafe extern "system" fn(
        Raw,
        *mut DeviceObject,
        *mut Irp,
        *mut Unknown,
        *mut Unknown,
        *mut ResourceList,
    ) -> NtStatus,
    pub get_device_property: unsafe extern "system" fn(Raw, i32, u32, Raw, *mut u32) -> NtStatus,
    pub new_registry_key: unsafe extern "system" fn(
        Raw,
        *mut Raw,
        *mut Unknown,
        u32,
        u32,
        Raw,
        u32,
        *mut u32,
    ) -> NtStatus,
}

#[repr(C)]
pub struct PortWaveRtStream {
    pub vtable: *const PortWaveRtStreamVtable,
}

#[repr(C)]
pub struct PortWaveRtStreamVtable {
    pub unknown: UnknownVtable,
    pub allocate_pages_for_mdl: unsafe extern "system" fn(Raw, PhysicalAddress, usize) -> *mut Mdl,
    pub allocate_contiguous_pages_for_mdl:
        unsafe extern "system" fn(Raw, PhysicalAddress, PhysicalAddress, usize) -> *mut Mdl,
    pub map_allocated_pages: unsafe extern "system" fn(Raw, *mut Mdl, MemoryCachingType) -> Raw,
    pub unmap_allocated_pages: unsafe extern "system" fn(Raw, Raw, *mut Mdl),
    pub free_pages_from_mdl: unsafe extern "system" fn(Raw, *mut Mdl),
    pub get_physical_pages_count: unsafe extern "system" fn(Raw, *mut Mdl) -> u32,
    pub get_physical_page_address: unsafe extern "system" fn(Raw, *mut Mdl, u32) -> PhysicalAddress,
}

#[repr(C)]
pub struct MiniportWaveRtStream {
    pub vtable: *const MiniportWaveRtStreamVtable,
}

#[repr(C)]
pub struct MiniportWaveRtStreamVtable {
    pub unknown: UnknownVtable,
    pub set_format: unsafe extern "system" fn(Raw, *mut KsDataFormat) -> NtStatus,
    pub set_state: unsafe extern "system" fn(Raw, KsState) -> NtStatus,
    pub get_position: unsafe extern "system" fn(Raw, *mut KsAudioPosition) -> NtStatus,
    pub allocate_audio_buffer: unsafe extern "system" fn(
        Raw,
        u32,
        *mut *mut Mdl,
        *mut u32,
        *mut u32,
        *mut MemoryCachingType,
    ) -> NtStatus,
    pub free_audio_buffer: unsafe extern "system" fn(Raw, *mut Mdl, u32),
    pub get_hw_latency: unsafe extern "system" fn(Raw, *mut KsRtAudioHwLatency),
    pub get_position_register: unsafe extern "system" fn(Raw, *mut KsRtAudioHwRegister) -> NtStatus,
    pub get_clock_register: unsafe extern "system" fn(Raw, *mut KsRtAudioHwRegister) -> NtStatus,
}

#[repr(C)]
pub struct AdapterPowerManagementVtable {
    pub unknown: UnknownVtable,
    pub power_change_state: unsafe extern "system" fn(Raw, PowerState),
    pub query_power_change_state: unsafe extern "system" fn(Raw, PowerState) -> NtStatus,
    pub query_device_capabilities:
        unsafe extern "system" fn(Raw, *mut DeviceCapabilities) -> NtStatus,
}

#[repr(C)]
pub struct MiniportPnpNotifyVtable {
    pub unknown: UnknownVtable,
    pub pnp_stop: unsafe extern "system" fn(Raw),
}

pub const IID_IADAPTER_PNP_MANAGEMENT: Guid = Guid::new(
    0x706f2368,
    0x4086,
    0x47f5,
    [0xb9, 0x13, 0x57, 0xb7, 0x6e, 0xed, 0x1a, 0x32],
);
pub const IID_IPORT_CLS_PNP: Guid = Guid::new(
    0x0485791b,
    0x17f5,
    0x4b73,
    [0x90, 0x2a, 0x20, 0x34, 0x48, 0x60, 0xb9, 0xca],
);

#[repr(C)]
pub struct AdapterPnpManagementVtable {
    pub unknown: UnknownVtable,
    pub get_supported_rebalance_type: unsafe extern "system" fn(Raw) -> i32,
    pub pnp_query_stop: unsafe extern "system" fn(Raw),
    pub pnp_cancel_stop: unsafe extern "system" fn(Raw),
    pub pnp_stop: unsafe extern "system" fn(Raw),
}

#[repr(C)]
pub struct PortClsPnp {
    pub vtable: *const PortClsPnpVtable,
}

#[repr(C)]
pub struct PortClsPnpVtable {
    pub unknown: UnknownVtable,
    pub register_adapter_pnp_management:
        unsafe extern "system" fn(Raw, *mut Unknown, *mut DeviceObject) -> NtStatus,
    pub unregister_adapter_pnp_management:
        unsafe extern "system" fn(Raw, *mut DeviceObject) -> NtStatus,
}

#[repr(C)]
pub struct UnregisterSubdevice {
    pub vtable: *const UnregisterSubdeviceVtable,
}

#[repr(C)]
pub struct UnregisterSubdeviceVtable {
    pub unknown: UnknownVtable,
    pub unregister_subdevice:
        unsafe extern "system" fn(Raw, *mut DeviceObject, *mut Unknown) -> NtStatus,
}

#[repr(C)]
pub struct UnregisterPhysicalConnection {
    pub vtable: *const UnregisterPhysicalConnectionVtable,
}

#[repr(C)]
pub struct UnregisterPhysicalConnectionVtable {
    pub unknown: UnknownVtable,
    pub unregister_physical_connection: unsafe extern "system" fn(
        Raw,
        *mut DeviceObject,
        *mut Unknown,
        u32,
        *mut Unknown,
        u32,
    ) -> NtStatus,
    pub unregister_physical_connection_to_external: unsafe extern "system" fn(
        Raw,
        *mut DeviceObject,
        *mut Unknown,
        u32,
        *mut UnicodeString,
        u32,
    ) -> NtStatus,
    pub unregister_physical_connection_from_external: unsafe extern "system" fn(
        Raw,
        *mut DeviceObject,
        *mut UnicodeString,
        u32,
        *mut Unknown,
        u32,
    ) -> NtStatus,
}

#[repr(C)]
pub struct PcAutomationTable {
    pub property_item_size: u32,
    pub property_count: u32,
    pub properties: *const PcPropertyItem,
    pub method_item_size: u32,
    pub method_count: u32,
    pub methods: *const c_void,
    pub event_item_size: u32,
    pub event_count: u32,
    pub events: *const c_void,
    pub reserved: u32,
}

#[repr(C)]
pub struct PcPropertyItem {
    pub set: *const Guid,
    pub id: u32,
    pub flags: u32,
    // PCPFNPROPERTY_HANDLER uses the compiler's C convention, not STDMETHOD.
    pub handler: Option<unsafe extern "C" fn(*mut PcPropertyRequest) -> NtStatus>,
}

#[repr(C)]
pub struct PcPropertyRequest {
    pub major_target: *mut Unknown,
    pub minor_target: *mut Unknown,
    pub node: u32,
    pub property_item: *const PcPropertyItem,
    pub verb: u32,
    pub instance_size: u32,
    pub instance: Raw,
    pub value_size: u32,
    pub value: Raw,
    pub irp: *mut Irp,
}

#[repr(C)]
pub struct PcPinDescriptor {
    pub max_global_instance_count: u32,
    pub max_filter_instance_count: u32,
    pub min_filter_instance_count: u32,
    pub automation_table: *const PcAutomationTable,
    pub ks_pin_descriptor: KsPinDescriptor,
}

#[repr(C)]
pub struct PcNodeDescriptor {
    pub flags: u32,
    pub automation_table: *const PcAutomationTable,
    pub node_type: *const Guid,
    pub name: *const Guid,
}

#[repr(C)]
pub struct PcFilterDescriptor {
    pub version: u32,
    pub automation_table: *const PcAutomationTable,
    pub pin_size: u32,
    pub pin_count: u32,
    pub pins: *const PcPinDescriptor,
    pub node_size: u32,
    pub node_count: u32,
    pub nodes: *const PcNodeDescriptor,
    pub connection_count: u32,
    pub connections: *const PcConnectionDescriptor,
    pub category_count: u32,
    pub categories: *const Guid,
}

// Immutable descriptor tables refer to other immutable, process-lifetime tables.
// These types provide no safe dereferencing or mutation of those raw pointers.
unsafe impl Sync for KsPinDescriptor {}
unsafe impl Sync for PcPinDescriptor {}
unsafe impl Sync for PcNodeDescriptor {}
unsafe impl Sync for PcFilterDescriptor {}
unsafe impl Sync for PcAutomationTable {}
unsafe impl Sync for PcPropertyItem {}

#[cfg(all(test, target_pointer_width = "64"))]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    #[test]
    fn sixty_four_bit_header_layout_contract() {
        assert_eq!(size_of::<KernelEvent>(), 24);
        assert_eq!(align_of::<KernelEvent>(), 8);
        assert_eq!(size_of::<DriverObjectFields>(), 336);
        assert_eq!(offset_of!(DriverObjectFields, major_function), 112);
        assert_eq!(offset_of!(DeviceObjectPrefix, device_extension), 64);
        assert_eq!(offset_of!(IrpPrefixToCurrentStack, current_location), 67);
        assert_eq!(
            offset_of!(IrpPrefixToCurrentStack, current_stack_location),
            184
        );
        assert_eq!(offset_of!(IoStackLocationPrefix, minor_function), 1);
        assert_eq!((size_of::<Guid>(), align_of::<Guid>()), (16, 4));
        assert_eq!(
            (size_of::<PhysicalAddress>(), align_of::<PhysicalAddress>()),
            (8, 8)
        );
        assert_eq!(size_of::<PowerState>(), 4);
        assert_eq!(size_of::<DeviceDescription>(), 64);
        assert_eq!(offset_of!(DeviceDescription, device_address), 56);
        assert_eq!(
            (size_of::<KsDataFormat>(), align_of::<KsDataFormat>()),
            (64, 8)
        );
        assert_eq!(
            (size_of::<KsIdentifier>(), align_of::<KsIdentifier>()),
            (24, 8)
        );
        assert_eq!(size_of::<KsDataRangeAudio>(), 88);
        assert_eq!(
            (size_of::<WaveFormatEx>(), align_of::<WaveFormatEx>()),
            (18, 1)
        );
        assert_eq!(
            (
                size_of::<WaveFormatExtensible>(),
                align_of::<WaveFormatExtensible>()
            ),
            (40, 1)
        );
        assert_eq!(offset_of!(WaveFormatExtensible, sub_format), 24);
        assert_eq!(size_of::<KsPinDescriptorTail>(), 16);
        assert_eq!(size_of::<KsPinDescriptor>(), 88);
        assert_eq!(offset_of!(KsPinDescriptor, tail), 72);
        assert_eq!(size_of::<PcPinDescriptor>(), 112);
        assert_eq!(offset_of!(PcPinDescriptor, ks_pin_descriptor), 24);
        assert_eq!(size_of::<PcNodeDescriptor>(), 32);
        assert_eq!(size_of::<PcFilterDescriptor>(), 80);
        assert_eq!(size_of::<PcConnectionDescriptor>(), 16);
        assert_eq!(size_of::<PcAutomationTable>(), 56);
        assert_eq!(size_of::<PcPropertyItem>(), 24);
        assert_eq!(size_of::<PcPropertyRequest>(), 72);
        assert_eq!(size_of::<KsAudioPosition>(), 16);
        assert_eq!(size_of::<KsRtAudioHwLatency>(), 12);
        assert_eq!(size_of::<KsRtAudioHwRegister>(), 40);
        assert_eq!(size_of::<UnknownVtable>(), 3 * 8);
        assert_eq!(size_of::<MiniportVtable>(), 5 * 8);
        assert_eq!(size_of::<MiniportWaveRtVtable>(), 8 * 8);
        assert_eq!(size_of::<MiniportTopologyVtable>(), 6 * 8);
        assert_eq!(size_of::<PortVtable>(), 6 * 8);
        assert_eq!(size_of::<PortWaveRtStreamVtable>(), 10 * 8);
        assert_eq!(
            offset_of!(PortWaveRtStreamVtable, get_physical_page_address),
            9 * 8
        );
        assert_eq!(size_of::<MiniportWaveRtStreamVtable>(), 11 * 8);
        assert_eq!(
            offset_of!(MiniportWaveRtStreamVtable, allocate_audio_buffer),
            6 * 8
        );
        assert_eq!(size_of::<AdapterPowerManagementVtable>(), 6 * 8);
        assert_eq!(size_of::<MiniportPnpNotifyVtable>(), 4 * 8);
        assert_eq!(size_of::<AdapterPnpManagementVtable>(), 7 * 8);
        assert_eq!(size_of::<PortClsPnpVtable>(), 5 * 8);
        assert_eq!(size_of::<UnregisterSubdeviceVtable>(), 4 * 8);
        assert_eq!(size_of::<UnregisterPhysicalConnectionVtable>(), 6 * 8);
    }
}
