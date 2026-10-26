//! Original fixed-format PortCls descriptors and bounded format negotiation.
use crate::abi::*;
use core::{mem::size_of, ptr};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;
pub const BITS_PER_SAMPLE: u16 = 32;
pub const FRAME_BYTES: u16 = 8;
pub const FORMAT_BYTES: u32 = KS_WAVEFORMATEXTENSIBLE_SIZE as u32;
const NULL_GUID: Guid = Guid::new(0, 0, 0, [0; 8]);

pub static AUDIO_RANGE: KsDataRangeAudio = KsDataRangeAudio {
    data_range: KsDataFormat {
        format_size: size_of::<KsDataRangeAudio>() as u32,
        flags: 0,
        sample_size: FRAME_BYTES as u32,
        reserved: 0,
        major_format: KSDATAFORMAT_TYPE_AUDIO,
        sub_format: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        specifier: KSDATAFORMAT_SPECIFIER_WAVEFORMATEX,
    },
    maximum_channels: CHANNELS as u32,
    minimum_bits_per_sample: BITS_PER_SAMPLE as u32,
    maximum_bits_per_sample: BITS_PER_SAMPLE as u32,
    minimum_sample_frequency: SAMPLE_RATE,
    maximum_sample_frequency: SAMPLE_RATE,
};

pub static ANALOG_RANGE: KsDataFormat = KsDataFormat {
    format_size: size_of::<KsDataFormat>() as u32,
    flags: 0,
    sample_size: 0,
    reserved: 0,
    major_format: KSDATAFORMAT_TYPE_AUDIO,
    sub_format: KSDATAFORMAT_SUBTYPE_ANALOG,
    specifier: KSDATAFORMAT_SPECIFIER_NONE,
};

struct RangePointers([*mut KsDataFormat; 1]);
// PortCls describes these pointers as mutable KSDATARANGE pointers but only reads
// the process-lifetime miniport descriptor tables.
unsafe impl Sync for RangePointers {}
static AUDIO_RANGES: RangePointers =
    RangePointers([ptr::addr_of!(AUDIO_RANGE.data_range).cast_mut()]);
static ANALOG_RANGES: RangePointers = RangePointers([ptr::addr_of!(ANALOG_RANGE).cast_mut()]);

const fn pin(stream: bool, flow: i32, category: *const Guid) -> PcPinDescriptor {
    PcPinDescriptor {
        max_global_instance_count: if stream { 1 } else { 0 },
        max_filter_instance_count: if stream { 1 } else { 0 },
        min_filter_instance_count: 0,
        automation_table: ptr::null(),
        ks_pin_descriptor: KsPinDescriptor {
            interfaces_count: 0,
            interfaces: ptr::null(),
            mediums_count: 0,
            mediums: ptr::null(),
            data_ranges_count: 1,
            data_ranges: if stream {
                AUDIO_RANGES.0.as_ptr()
            } else {
                ANALOG_RANGES.0.as_ptr()
            },
            data_flow: flow,
            communication: if stream {
                KSPIN_COMMUNICATION_SINK
            } else {
                KSPIN_COMMUNICATION_NONE
            },
            category,
            name: ptr::null(),
            // Initialize the full x64 union, including the pointer-sized tail.
            tail: KsPinDescriptorTail {
                constrained: KsConstrainedDataRanges {
                    count: 0,
                    ranges: ptr::null_mut(),
                },
            },
        },
    }
}

pub static WAVE_PINS: [PcPinDescriptor; 4] = [
    pin(true, KSPIN_DATAFLOW_IN, &KSCATEGORY_AUDIO),
    pin(false, KSPIN_DATAFLOW_OUT, &KSCATEGORY_AUDIO),
    pin(false, KSPIN_DATAFLOW_IN, &KSCATEGORY_AUDIO),
    pin(true, KSPIN_DATAFLOW_OUT, &KSCATEGORY_AUDIO),
];

static WAVE_NODES: [PcNodeDescriptor; 2] = [
    PcNodeDescriptor {
        flags: 0,
        automation_table: ptr::null(),
        node_type: &KSNODETYPE_DAC,
        name: ptr::null(),
    },
    PcNodeDescriptor {
        flags: 0,
        automation_table: ptr::null(),
        node_type: &KSNODETYPE_ADC,
        name: ptr::null(),
    },
];

const fn connection(
    from_node: u32,
    from_node_pin: u32,
    to_node: u32,
    to_node_pin: u32,
) -> PcConnectionDescriptor {
    PcConnectionDescriptor {
        from_node,
        from_node_pin,
        to_node,
        to_node_pin,
    }
}

static WAVE_CONNECTIONS: [PcConnectionDescriptor; 4] = [
    connection(PCFILTER_NODE, 0, 0, KSNODEPIN_STANDARD_IN),
    connection(0, KSNODEPIN_STANDARD_OUT, PCFILTER_NODE, 1),
    connection(PCFILTER_NODE, 2, 1, KSNODEPIN_STANDARD_IN),
    connection(1, KSNODEPIN_STANDARD_OUT, PCFILTER_NODE, 3),
];
static WAVE_CATEGORIES: [Guid; 3] = [KSCATEGORY_AUDIO, KSCATEGORY_RENDER, KSCATEGORY_CAPTURE];

pub static WAVE_FILTER: PcFilterDescriptor = PcFilterDescriptor {
    version: 0,
    automation_table: ptr::null(),
    pin_size: size_of::<PcPinDescriptor>() as u32,
    pin_count: WAVE_PINS.len() as u32,
    pins: WAVE_PINS.as_ptr(),
    node_size: size_of::<PcNodeDescriptor>() as u32,
    node_count: WAVE_NODES.len() as u32,
    nodes: WAVE_NODES.as_ptr(),
    connection_count: WAVE_CONNECTIONS.len() as u32,
    connections: WAVE_CONNECTIONS.as_ptr(),
    category_count: WAVE_CATEGORIES.len() as u32,
    categories: WAVE_CATEGORIES.as_ptr(),
};

pub static TOPOLOGY_PINS: [PcPinDescriptor; 4] = [
    pin(false, KSPIN_DATAFLOW_IN, &KSCATEGORY_AUDIO),
    pin(false, KSPIN_DATAFLOW_OUT, &KSNODETYPE_SPEAKER),
    pin(false, KSPIN_DATAFLOW_IN, &KSNODETYPE_MICROPHONE),
    pin(false, KSPIN_DATAFLOW_OUT, &KSCATEGORY_AUDIO),
];
static TOPOLOGY_CONNECTIONS: [PcConnectionDescriptor; 2] = [
    connection(PCFILTER_NODE, 0, PCFILTER_NODE, 1),
    connection(PCFILTER_NODE, 2, PCFILTER_NODE, 3),
];
static TOPOLOGY_CATEGORIES: [Guid; 2] = [KSCATEGORY_AUDIO, KSCATEGORY_TOPOLOGY];

pub static TOPOLOGY_FILTER: PcFilterDescriptor = PcFilterDescriptor {
    version: 0,
    automation_table: ptr::null(),
    pin_size: size_of::<PcPinDescriptor>() as u32,
    pin_count: TOPOLOGY_PINS.len() as u32,
    pins: TOPOLOGY_PINS.as_ptr(),
    node_size: size_of::<PcNodeDescriptor>() as u32,
    node_count: 0,
    nodes: ptr::null(),
    connection_count: TOPOLOGY_CONNECTIONS.len() as u32,
    connections: TOPOLOGY_CONNECTIONS.as_ptr(),
    category_count: TOPOLOGY_CATEGORIES.len() as u32,
    categories: TOPOLOGY_CATEGORIES.as_ptr(),
};

pub const fn stream_capture(pin: u32) -> Option<bool> {
    match pin {
        0 => Some(false),
        3 => Some(true),
        _ => None,
    }
}

fn fixed_header(size: u32) -> KsDataFormat {
    KsDataFormat {
        format_size: size,
        ..AUDIO_RANGE.data_range
    }
}

fn fixed_wave() -> WaveFormatExtensible {
    WaveFormatExtensible {
        format: WaveFormatEx {
            format_tag: WAVE_FORMAT_EXTENSIBLE,
            channels: CHANNELS,
            samples_per_sec: SAMPLE_RATE,
            avg_bytes_per_sec: SAMPLE_RATE * u32::from(FRAME_BYTES),
            block_align: FRAME_BYTES,
            bits_per_sample: BITS_PER_SAMPLE,
            cb_size: 22,
        },
        samples: BITS_PER_SAMPLE,
        channel_mask: KSAUDIO_SPEAKER_STEREO,
        sub_format: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
    }
}

/// Validate a complete bounded KS format without taking an aligned reference.
/// The canonical extensible format and its equivalent plain IEEE_FLOAT format
/// are accepted; attributes and all different audio formats are rejected.
pub fn validate_format_bytes(bytes: &[u8]) -> bool {
    if bytes.len() < size_of::<KsDataFormat>() {
        return false;
    }
    let header = unsafe { ptr::read_unaligned(bytes.as_ptr().cast::<KsDataFormat>()) };
    if header.format_size as usize != bytes.len()
        || header.flags != 0
        || header.reserved != 0
        || !matches!(header.sample_size, 0 | 8)
        || header.major_format != KSDATAFORMAT_TYPE_AUDIO
        || header.sub_format != KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        || header.specifier != KSDATAFORMAT_SPECIFIER_WAVEFORMATEX
        || !matches!(
            bytes.len(),
            KS_WAVEFORMATEX_SIZE | KS_WAVEFORMATEXTENSIBLE_SIZE
        )
    {
        return false;
    }
    let wave = unsafe { ptr::read_unaligned(bytes.as_ptr().add(64).cast::<WaveFormatEx>()) };
    if wave.channels != CHANNELS
        || wave.samples_per_sec != SAMPLE_RATE
        || wave.avg_bytes_per_sec != SAMPLE_RATE * u32::from(FRAME_BYTES)
        || wave.block_align != FRAME_BYTES
        || wave.bits_per_sample != BITS_PER_SAMPLE
    {
        return false;
    }
    if wave.format_tag == WAVE_FORMAT_IEEE_FLOAT {
        return wave.cb_size == 0 && bytes.len() == KS_WAVEFORMATEX_SIZE;
    }
    if wave.format_tag != WAVE_FORMAT_EXTENSIBLE
        || wave.cb_size != 22
        || bytes.len() != KS_WAVEFORMATEXTENSIBLE_SIZE
    {
        return false;
    }
    let ext = unsafe { ptr::read_unaligned(bytes.as_ptr().add(64).cast::<WaveFormatExtensible>()) };
    // Channel mask zero denotes direct channel assignment; it is equivalent for
    // the only two-channel format supported here.
    let sub_format = ext.sub_format;
    ext.samples == BITS_PER_SAMPLE
        && matches!(ext.channel_mask, 0 | KSAUDIO_SPEAKER_STEREO)
        && sub_format == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
}

/// # Safety
/// `format` must be null or point to a readable KS header followed by at least
/// `format_size` readable bytes, as required of PortCls format parameters.
pub unsafe fn validate_format(format: *const KsDataFormat) -> bool {
    if format.is_null() {
        return false;
    }
    let size = ptr::read_unaligned(format).format_size as usize;
    if !matches!(size, KS_WAVEFORMATEX_SIZE | KS_WAVEFORMATEXTENSIBLE_SIZE) {
        return false;
    }
    validate_format_bytes(core::slice::from_raw_parts(format.cast::<u8>(), size))
}

fn guid_matches(requested: Guid, supported: Guid) -> bool {
    requested == NULL_GUID || requested == supported
}

unsafe fn range_matches(range: *const KsDataFormat, digital: bool) -> bool {
    if range.is_null() {
        return false;
    }
    let header = ptr::read_unaligned(range);
    let supported = if digital {
        &AUDIO_RANGE.data_range
    } else {
        &ANALOG_RANGE
    };
    if header.flags != 0
        || header.reserved != 0
        || !guid_matches(header.major_format, supported.major_format)
        || !guid_matches(header.sub_format, supported.sub_format)
        || !guid_matches(header.specifier, supported.specifier)
        || (digital && !matches!(header.sample_size, 0 | 8))
    {
        return false;
    }
    if header.format_size == size_of::<KsDataFormat>() as u32 {
        return !digital || header.specifier == NULL_GUID;
    }
    if !digital || header.format_size != size_of::<KsDataRangeAudio>() as u32 {
        return false;
    }
    let audio = ptr::read_unaligned(range.cast::<KsDataRangeAudio>());
    audio.maximum_channels >= u32::from(CHANNELS)
        && audio.minimum_bits_per_sample <= u32::from(BITS_PER_SAMPLE)
        && audio.maximum_bits_per_sample >= u32::from(BITS_PER_SAMPLE)
        && audio.minimum_sample_frequency <= SAMPLE_RATE
        && audio.maximum_sample_frequency >= SAMPLE_RATE
}

/// Intersect against the single native format or the analog bridge range.
///
/// # Safety
/// Both range pointers must be null or readable for their declared sizes.
/// `written` must be null or writable for a u32. A non-null `out` must refer to
/// at least `capacity` writable bytes and must not alias `written` or the ranges.
pub unsafe fn data_range_intersection(
    wave: bool,
    pin: u32,
    requested: *mut KsDataFormat,
    matching: *mut KsDataFormat,
    capacity: u32,
    out: Raw,
    written: *mut u32,
) -> NtStatus {
    if written.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    ptr::write_unaligned(written, 0);
    if pin >= 4 || requested.is_null() || matching.is_null() || (capacity != 0 && out.is_null()) {
        return STATUS_INVALID_PARAMETER;
    }
    let digital = wave && stream_capture(pin).is_some();
    if !range_matches(requested, digital) || !range_matches(matching, digital) {
        return STATUS_NO_MATCH;
    }
    let required = if digital {
        FORMAT_BYTES
    } else {
        size_of::<KsDataFormat>() as u32
    };
    if capacity == 0 {
        ptr::write_unaligned(written, required);
        return STATUS_BUFFER_OVERFLOW;
    }
    if capacity < required {
        return STATUS_BUFFER_TOO_SMALL;
    }
    if digital {
        ptr::write_unaligned(out.cast::<KsDataFormat>(), fixed_header(FORMAT_BYTES));
        ptr::write_unaligned(
            out.cast::<u8>().add(64).cast::<WaveFormatExtensible>(),
            fixed_wave(),
        );
    } else {
        ptr::write_unaligned(out.cast::<KsDataFormat>(), ANALOG_RANGE);
    }
    ptr::write_unaligned(written, required);
    STATUS_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_format_negotiation_is_bounded_and_rejects_malformed_formats() {
        let requested = ptr::addr_of!(AUDIO_RANGE.data_range).cast_mut();
        let mut written = 99;
        let mut bytes = [0xa5; KS_WAVEFORMATEXTENSIBLE_SIZE + 2];
        unsafe {
            assert_eq!(
                data_range_intersection(
                    true,
                    0,
                    requested,
                    requested,
                    0,
                    ptr::null_mut(),
                    &mut written
                ),
                STATUS_BUFFER_OVERFLOW
            );
            assert_eq!(written, FORMAT_BYTES);
            assert_eq!(
                data_range_intersection(
                    true,
                    0,
                    requested,
                    requested,
                    FORMAT_BYTES - 1,
                    bytes.as_mut_ptr().add(1).cast(),
                    &mut written
                ),
                STATUS_BUFFER_TOO_SMALL
            );
            assert_eq!(written, 0);
            assert!(bytes.iter().all(|b| *b == 0xa5));
            assert_eq!(
                data_range_intersection(
                    true,
                    0,
                    requested,
                    requested,
                    FORMAT_BYTES,
                    bytes.as_mut_ptr().add(1).cast(),
                    &mut written
                ),
                STATUS_SUCCESS
            );
        }
        assert_eq!(written, FORMAT_BYTES);
        assert_eq!(
            (bytes[0], bytes[KS_WAVEFORMATEXTENSIBLE_SIZE + 1]),
            (0xa5, 0xa5)
        );
        assert!(validate_format_bytes(
            &bytes[1..KS_WAVEFORMATEXTENSIBLE_SIZE + 1]
        ));
        for n in 0..KS_WAVEFORMATEXTENSIBLE_SIZE {
            assert!(!validate_format_bytes(&bytes[1..1 + n]));
        }
        bytes[1 + 64 + 4..1 + 64 + 8].copy_from_slice(&44_100u32.to_le_bytes());
        assert!(!validate_format_bytes(
            &bytes[1..KS_WAVEFORMATEXTENSIBLE_SIZE + 1]
        ));
    }

    #[test]
    fn range_constraints_and_bridge_negotiation_are_respected() {
        let mut requested = AUDIO_RANGE;
        requested.maximum_channels = 1;
        let matching = ptr::addr_of!(AUDIO_RANGE.data_range).cast_mut();
        let mut written = 99;
        unsafe {
            assert_eq!(
                data_range_intersection(
                    true,
                    3,
                    &mut requested.data_range,
                    matching,
                    0,
                    ptr::null_mut(),
                    &mut written
                ),
                STATUS_NO_MATCH
            );
            assert_eq!(written, 0);
            let analog = ptr::addr_of!(ANALOG_RANGE).cast_mut();
            assert_eq!(
                data_range_intersection(false, 2, analog, analog, 0, ptr::null_mut(), &mut written),
                STATUS_BUFFER_OVERFLOW
            );
            assert_eq!(written, 64);
        }
    }

    #[test]
    fn descriptor_graphs_only_instantiate_the_two_streaming_pins() {
        assert_eq!(
            (
                WAVE_FILTER.pin_count,
                WAVE_FILTER.node_count,
                WAVE_FILTER.connection_count
            ),
            (4, 2, 4)
        );
        assert_eq!(
            (
                TOPOLOGY_FILTER.pin_count,
                TOPOLOGY_FILTER.node_count,
                TOPOLOGY_FILTER.connection_count
            ),
            (4, 0, 2)
        );
        assert_eq!(stream_capture(0), Some(false));
        assert_eq!(stream_capture(3), Some(true));
        for (index, pin) in WAVE_PINS.iter().enumerate() {
            assert_eq!(
                pin.max_global_instance_count,
                u32::from(matches!(index, 0 | 3))
            );
            assert_eq!(pin.min_filter_instance_count, 0);
        }
        assert!(TOPOLOGY_PINS
            .iter()
            .all(|p| p.max_global_instance_count == 0));
        for edge in WAVE_CONNECTIONS {
            assert!(edge.from_node == PCFILTER_NODE || edge.from_node < WAVE_FILTER.node_count);
            assert!(edge.to_node == PCFILTER_NODE || edge.to_node < WAVE_FILTER.node_count);
            if edge.from_node == PCFILTER_NODE {
                assert!(edge.from_node_pin < WAVE_FILTER.pin_count);
            }
            if edge.to_node == PCFILTER_NODE {
                assert!(edge.to_node_pin < WAVE_FILTER.pin_count);
            }
        }
    }
}
