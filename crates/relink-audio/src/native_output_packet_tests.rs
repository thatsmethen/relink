use super::*;
use crate::{
    native_packet::{BIG_ENDIAN, FLOAT, HIGH_ALIGNED, LINEAR_PCM, NON_INTERLEAVED, PACKED, SIGNED},
    tests::{COUNTING, OPERATIONS},
    MAX_CALLBACK_FRAMES, MAX_DEVICE_CHANNELS,
};

fn format(bits: u32, width: u32, channels: u32, flags: u32) -> Format {
    let frame_bytes = width
        * if flags & NON_INTERLEAVED != 0 {
            1
        } else {
            channels
        };
    Format {
        rate: 48000.0,
        id: LINEAR_PCM,
        flags,
        packet_bytes: frame_bytes,
        packet_frames: 1,
        frame_bytes,
        channels,
        bits,
    }
}

fn encoded(format: Format, left: &[f32], right: &[f32]) -> Vec<u8> {
    let layout = OutputLayout::new(&[format], 48000).unwrap();
    let mut bytes = vec![0x5a; format.frame_bytes as usize * left.len()];
    {
        let mut planes = [PlaneBuffer {
            channels: format.channels,
            bytes: &mut bytes,
        }];
        let mut packet = layout.prepare(&mut planes).unwrap();
        assert_eq!(packet.frames(), left.len());
        packet.write_stereo(0, left, right).unwrap();
        let completed = packet.complete().unwrap();
        assert_eq!(completed.frames(), left.len());
        completed.handoff();
    }
    bytes
}

#[test]
fn float_endianness_extrema_nonfinite_and_negative_zero_have_exact_bytes() {
    let left = [-1.0, -0.0, 0.5, f32::NAN, f32::INFINITY];
    let right = [1.0, 0.0, 2.0, f32::NEG_INFINITY, -2.0];
    let expected32 = [
        0, 0, 128, 191, 0, 0, 128, 63, 0, 0, 0, 128, 0, 0, 0, 0, 0, 0, 0, 63, 0, 0, 128, 63, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 191,
    ];
    let expected64 = [
        0, 0, 0, 0, 0, 0, 240, 191, 0, 0, 0, 0, 0, 0, 240, 63, 0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 224, 63, 0, 0, 0, 0, 0, 0, 240, 63, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 240, 191,
    ];
    for (bits, expected) in [(32, expected32.as_slice()), (64, expected64.as_slice())] {
        let width = bits / 8;
        assert_eq!(
            encoded(format(bits, width, 2, FLOAT | PACKED), &left, &right),
            expected
        );
        let big: Vec<_> = expected
            .chunks(width as usize)
            .flat_map(|bytes| bytes.iter().rev().copied())
            .collect();
        assert_eq!(
            encoded(
                format(bits, width, 2, FLOAT | PACKED | BIG_ENDIAN),
                &left,
                &right
            ),
            big
        );
    }
}

#[test]
fn packed_signed_extrema_halves_clipping_and_nonfinite_have_exact_bytes() {
    let left = [-1.0, -0.5, -2.0, f32::NAN];
    let right = [1.0, 0.5, 2.0, f32::INFINITY];
    let fixtures: &[(u32, &[u8])] = &[
        (8, &[0x80, 0x7f, 0xc0, 0x40, 0x80, 0x7f, 0, 0]),
        (
            16,
            &[
                0, 0x80, 0xff, 0x7f, 0, 0xc0, 0, 0x40, 0, 0x80, 0xff, 0x7f, 0, 0, 0, 0,
            ],
        ),
        (
            24,
            &[
                0, 0, 0x80, 0xff, 0xff, 0x7f, 0, 0, 0xc0, 0, 0, 0x40, 0, 0, 0x80, 0xff, 0xff, 0x7f,
                0, 0, 0, 0, 0, 0,
            ],
        ),
        (
            32,
            &[
                0, 0, 0, 0x80, 0xff, 0xff, 0xff, 0x7f, 0, 0, 0, 0xc0, 0, 0, 0, 0x40, 0, 0, 0, 0x80,
                0xff, 0xff, 0xff, 0x7f, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        ),
    ];
    for &(bits, expected) in fixtures {
        let width = bits / 8;
        assert_eq!(
            encoded(format(bits, width, 2, SIGNED | PACKED), &left, &right),
            expected
        );
        let big: Vec<_> = expected
            .chunks(width as usize)
            .flat_map(|bytes| bytes.iter().rev().copied())
            .collect();
        assert_eq!(
            encoded(
                format(bits, width, 2, SIGNED | PACKED | BIG_ENDIAN),
                &left,
                &right
            ),
            big
        );
    }
}

#[test]
fn signed_rounding_uses_nearest_ties_away_before_saturation() {
    for bits in [8, 16, 24, 32] {
        let half_lsb = 0.5 / (1_u64 << (bits - 1)) as f32;
        let result = encoded(
            format(bits, bits / 8, 2, SIGNED | PACKED),
            &[-half_lsb, -half_lsb * 0.5],
            &[half_lsb, half_lsb * 0.5],
        );
        let width = bits as usize / 8;
        assert!(result[..width].iter().all(|b| *b == 0xff));
        assert_eq!(result[width], 1);
        assert!(result[width + 1..].iter().all(|b| *b == 0));
    }
}

#[test]
fn padded_valid_bits_zero_padding_and_endian_are_exact() {
    let fixtures: &[(u32, u32, &[u8], &[u8])] = &[
        (8, 2, &[0x80, 0, 0x7f, 0], &[0, 0x80, 0, 0x7f]),
        (8, 3, &[0x80, 0, 0, 0x7f, 0, 0], &[0, 0, 0x80, 0, 0, 0x7f]),
        (
            8,
            4,
            &[0x80, 0, 0, 0, 0x7f, 0, 0, 0],
            &[0, 0, 0, 0x80, 0, 0, 0, 0x7f],
        ),
        (
            16,
            3,
            &[0, 0x80, 0, 0xff, 0x7f, 0],
            &[0, 0, 0x80, 0, 0xff, 0x7f],
        ),
        (
            16,
            4,
            &[0, 0x80, 0, 0, 0xff, 0x7f, 0, 0],
            &[0, 0, 0, 0x80, 0, 0, 0xff, 0x7f],
        ),
        (
            24,
            4,
            &[0, 0, 0x80, 0, 0xff, 0xff, 0x7f, 0],
            &[0, 0, 0, 0x80, 0, 0xff, 0xff, 0x7f],
        ),
    ];
    for &(bits, width, low, high) in fixtures {
        for (alignment, expected) in [(0, low), (HIGH_ALIGNED, high)] {
            assert_eq!(
                encoded(format(bits, width, 2, SIGNED | alignment), &[-1.0], &[1.0]),
                expected
            );
            let big: Vec<_> = expected
                .chunks(width as usize)
                .flat_map(|b| b.iter().rev().copied())
                .collect();
            assert_eq!(
                encoded(
                    format(bits, width, 2, SIGNED | alignment | BIG_ENDIAN),
                    &[-1.0],
                    &[1.0]
                ),
                big
            );
        }
    }
    // Negative one LSB in 24 significant bits has canonical zero padding.
    assert_eq!(
        encoded(
            format(24, 4, 2, SIGNED | HIGH_ALIGNED),
            &[-1.0 / 8388608.0],
            &[0.0]
        ),
        [0, 0xff, 0xff, 0xff, 0, 0, 0, 0]
    );
}

#[test]
fn unsigned_eight_bit_equilibrium_rounding_and_saturation_are_exact() {
    assert_eq!(
        encoded(
            format(8, 1, 2, PACKED),
            &[-1.0, -0.5, 0.0, -2.0, f32::NAN, -0.5 / 128.0],
            &[1.0, 0.5, -0.0, 2.0, f32::NEG_INFINITY, 0.5 / 128.0]
        ),
        [0, 255, 64, 192, 128, 128, 0, 255, 128, 128, 128, 129]
    );
    // Endianness on single-byte samples does not change the representation.
    assert_eq!(
        encoded(format(8, 1, 2, BIG_ENDIAN), &[f32::INFINITY], &[0.0]),
        [128, 128]
    );
}

#[test]
fn planar_and_multiple_native_streams_select_first_pair_and_silence_the_rest() {
    let layout = OutputLayout::new(
        &[
            format(16, 2, 1, SIGNED | BIG_ENDIAN),
            format(8, 1, 3, NON_INTERLEAVED),
            format(32, 4, 2, FLOAT | PACKED),
        ],
        48000,
    )
    .unwrap();
    let mut a = [0x5a; 4];
    let mut b = [0x5a; 2];
    let mut c = b;
    let mut d = b;
    let mut e = [0x5a; 16];
    {
        let mut planes = [
            PlaneBuffer {
                channels: 1,
                bytes: &mut a,
            },
            PlaneBuffer {
                channels: 1,
                bytes: &mut b,
            },
            PlaneBuffer {
                channels: 1,
                bytes: &mut c,
            },
            PlaneBuffer {
                channels: 1,
                bytes: &mut d,
            },
            PlaneBuffer {
                channels: 2,
                bytes: &mut e,
            },
        ];
        let mut packet = layout.prepare(&mut planes).unwrap();
        packet.write_stereo(0, &[0.5], &[-0.5]).unwrap();
        packet.write_stereo(1, &[-1.0], &[1.0]).unwrap();
        packet.complete().unwrap().handoff();
    }
    assert_eq!(a, [0x40, 0, 0x80, 0]);
    assert_eq!(b, [64, 255]);
    assert_eq!(c, [128; 2]);
    assert_eq!(d, [128; 2]);
    assert_eq!(e, [0; 16]);
    assert_eq!(
        encoded(format(8, 1, 5, PACKED), &[1.0, -1.0], &[-1.0, 1.0]),
        [255, 0, 128, 128, 128, 0, 255, 128, 128, 128]
    );
}

#[test]
fn malformed_extents_are_rejected_before_any_plane_is_modified() {
    let layout = OutputLayout::new(&[format(16, 2, 2, SIGNED | NON_INTERLEAVED)], 48000).unwrap();
    let assert_bad = |first_channels, second_channels, first_len, second_len, plane_count| {
        let mut a = vec![0x5a; first_len];
        let mut b = vec![0xa5; second_len];
        {
            let mut planes = [
                PlaneBuffer {
                    channels: first_channels,
                    bytes: &mut a,
                },
                PlaneBuffer {
                    channels: second_channels,
                    bytes: &mut b,
                },
            ];
            assert!(matches!(
                layout.prepare(&mut planes[..plane_count]),
                Err(Error::InvalidBuffers(_))
            ));
        }
        assert!(a.iter().all(|v| *v == 0x5a));
        assert!(b.iter().all(|v| *v == 0xa5));
    };
    assert_bad(1, 2, 4, 4, 2);
    assert_bad(1, 1, 4, 3, 2);
    assert_bad(1, 1, 4, 2, 2);
    assert_bad(1, 1, 4, 4, 1);
    assert_bad(1, 1, 4, 4, 0);
    assert_bad(1, 1, 0, 0, 2);
    assert_bad(
        1,
        1,
        (MAX_CALLBACK_FRAMES + 1) * 2,
        (MAX_CALLBACK_FRAMES + 1) * 2,
        2,
    );
    // More planes than the known ASBD has is also invalid and untouched.
    let single = OutputLayout::new(&[format(8, 1, 2, PACKED)], 48000).unwrap();
    let mut a = [0x5a; 2];
    let mut b = a;
    let mut planes = [
        PlaneBuffer {
            channels: 2,
            bytes: &mut a,
        },
        PlaneBuffer {
            channels: 2,
            bytes: &mut b,
        },
    ];
    assert!(matches!(
        single.prepare(&mut planes),
        Err(Error::InvalidBuffers(_))
    ));
    assert_eq!(a, [0x5a; 2]);
    assert_eq!(b, [0x5a; 2]);
}

#[test]
fn ambiguous_asbds_mono_and_rate_substitution_are_rejected() {
    assert!(matches!(
        OutputLayout::new(&[format(32, 4, 1, FLOAT)], 48000),
        Err(Error::InsufficientChannels(1))
    ));
    assert!(matches!(
        OutputLayout::new(&[], 48000),
        Err(Error::InvalidFormat(_))
    ));
    assert!(matches!(
        OutputLayout::new(&[format(32, 4, 2, FLOAT)], 0),
        Err(Error::InvalidRate)
    ));
    assert!(matches!(
        OutputLayout::new(&[format(32, 4, 2, FLOAT)], 44100),
        Err(Error::InvalidFormat(_))
    ));
    for case in 0..15 {
        let mut f = format(32, 4, 2, FLOAT | PACKED);
        match case {
            0 => f.flags |= 1 << 15,
            1 => f.flags |= SIGNED,
            2 => f.flags |= HIGH_ALIGNED,
            3 => f.id = 0,
            4 => f.bits = 24,
            5 => f.packet_frames = 2,
            6 => f.packet_bytes += 1,
            7 => f.channels = 0,
            8 => f.channels = u32::from(MAX_DEVICE_CHANNELS) + 1,
            9 => f.rate = f64::NAN,
            10 => {
                f.frame_bytes = u32::MAX;
                f.packet_bytes = u32::MAX;
                f.channels = 1;
            }
            11 => f = format(24, 4, 2, SIGNED | PACKED),
            12 => f = format(16, 2, 2, PACKED), // unsupported unsigned width
            13 => f = format(8, 1, 2, HIGH_ALIGNED),
            14 => f = format(12, 2, 2, SIGNED),
            _ => unreachable!(),
        }
        assert!(
            matches!(OutputLayout::new(&[f], 48000), Err(Error::InvalidFormat(_))),
            "case {case}"
        );
    }
}

#[test]
fn malformed_write_is_sticky_and_clears_partial_program_pcm() {
    let layout = OutputLayout::new(&[format(8, 1, 3, PACKED)], 48000).unwrap();
    for case in 0..4 {
        let mut bytes = [0x5a; 9];
        {
            let mut planes = [PlaneBuffer {
                channels: 3,
                bytes: &mut bytes,
            }];
            let mut packet = layout.prepare(&mut planes).unwrap();
            packet.write_stereo(0, &[1.0], &[-1.0]).unwrap();
            let error = match case {
                0 => packet.write_stereo(0, &[1.0], &[1.0]),
                1 => packet.write_stereo(usize::MAX, &[1.0], &[1.0]),
                2 => packet.write_stereo(1, &[1.0, 1.0], &[1.0]),
                _ => packet.write_stereo(1, &[1.0; 3], &[1.0; 3]),
            }
            .unwrap_err();
            assert_eq!(packet.failure(), Some(error));
            assert_eq!(StereoSink::frames(&packet), Err(InvalidOutputBuffer));
            packet.clear();
            assert_eq!(packet.write_stereo(0, &[0.0; 3], &[0.0; 3]), Err(error));
            assert!(matches!(packet.complete(), Err(e) if e == error));
        }
        assert_eq!(bytes, [128; 9]);
    }
}

#[test]
fn inactive_incomplete_abandoned_and_cancelled_after_final_chunk_are_silent() {
    for flags in [PACKED, SIGNED | PACKED] {
        let layout = OutputLayout::new(&[format(8, 1, 4, flags)], 48000).unwrap();
        for scenario in 0..7 {
            let mut bytes = [0x5a; 12];
            {
                let mut planes = [PlaneBuffer {
                    channels: 4,
                    bytes: &mut bytes,
                }];
                let mut packet = layout.prepare(&mut planes).unwrap();
                match scenario {
                    0 => (), // inactive: preparation and drop only
                    1 => {
                        packet.write_stereo(0, &[1.0], &[-1.0]).unwrap();
                    }
                    2 => {
                        packet.write_stereo(0, &[1.0], &[-1.0]).unwrap();
                        assert!(matches!(
                            packet.complete(),
                            Err(Error::Incomplete {
                                written: 1,
                                total: 3
                            })
                        ));
                    }
                    3 => {
                        packet.write_stereo(0, &[1.0; 3], &[-1.0; 3]).unwrap();
                        StereoSink::clear(&mut packet); // cancellation at the final software gate
                        assert!(matches!(
                            packet.complete(),
                            Err(Error::Incomplete {
                                written: 0,
                                total: 3
                            })
                        ));
                    }
                    4 => {
                        packet.write_stereo(0, &[1.0; 3], &[-1.0; 3]).unwrap();
                        // Fully written but never authorized/marked complete.
                    }
                    5 => {
                        packet.write_stereo(0, &[1.0; 3], &[-1.0; 3]).unwrap();
                        packet.complete().unwrap().cancel();
                    }
                    _ => {
                        packet.write_stereo(0, &[1.0; 3], &[-1.0; 3]).unwrap();
                        drop(packet.complete().unwrap());
                    }
                }
            }
            assert_eq!(bytes, [if flags & SIGNED == 0 { 128 } else { 0 }; 12]);
        }
    }
}

#[test]
fn sequential_partitioning_matches_whole_packet_and_empty_chunks_do_not_complete() {
    let f = format(24, 4, 2, SIGNED | HIGH_ALIGNED | BIG_ENDIAN);
    let left = [-1.0, 0.0, 0.5, f32::NAN, 1.0];
    let right = [0.125, 0.25, 0.5, -0.5, -1.0];
    let expected = encoded(f, &left, &right);
    let layout = OutputLayout::new(&[f], 48000).unwrap();
    let mut bytes = [0x5a; 40];
    {
        let mut planes = [PlaneBuffer {
            channels: 2,
            bytes: &mut bytes,
        }];
        let mut packet = layout.prepare(&mut planes).unwrap();
        assert_eq!(StereoSink::frames(&packet), Ok(5));
        StereoSink::clear(&mut packet);
        for (start, end) in [(0, 0), (0, 1), (1, 3), (3, 3), (3, 5)] {
            StereoSink::write_stereo(&mut packet, start, &left[start..end], &right[start..end])
                .unwrap();
        }
        packet.complete().unwrap().handoff();
    }
    assert_eq!(bytes.as_slice(), expected);
    let mut planes = [PlaneBuffer {
        channels: 2,
        bytes: &mut bytes,
    }];
    let mut packet = layout.prepare(&mut planes).unwrap();
    packet.write_stereo(0, &[], &[]).unwrap();
    assert!(matches!(
        packet.complete(),
        Err(Error::Incomplete {
            written: 0,
            total: 5
        })
    ));
}

#[test]
fn bounded_maximum_layout_and_all_packet_paths_allocate_and_free_nothing() {
    let layout = OutputLayout::new(
        &[format(
            8,
            1,
            u32::from(MAX_DEVICE_CHANNELS),
            NON_INTERLEAVED,
        )],
        48000,
    )
    .unwrap();
    assert_eq!(
        layout.storage_bytes(),
        std::mem::size_of::<OutputLayout>()
            + layout.0.planes.capacity() * std::mem::size_of::<Plane>()
    );
    assert!(layout.0.planes.capacity() <= usize::from(MAX_DEVICE_CHANNELS));
    let mut storage = vec![0x5a; MAX_CALLBACK_FRAMES * usize::from(MAX_DEVICE_CHANNELS)];
    let mut planes: Vec<_> = storage
        .chunks_mut(MAX_CALLBACK_FRAMES)
        .map(|bytes| PlaneBuffer { channels: 1, bytes })
        .collect();
    let left = [0.5; MAX_CALLBACK_FRAMES];
    let right = [-0.5; MAX_CALLBACK_FRAMES];
    OPERATIONS.with(|n| n.set(0));
    COUNTING.with(|on| on.set(true));
    {
        let mut packet = layout.prepare(&mut planes).unwrap();
        packet
            .write_stereo(0, &left[..4096], &right[..4096])
            .unwrap();
        packet
            .write_stereo(4096, &left[4096..], &right[4096..])
            .unwrap();
        packet.complete().unwrap().handoff();
    }
    {
        let mut packet = layout.prepare(&mut planes).unwrap();
        packet.write_stereo(0, &left, &right).unwrap();
        packet.complete().unwrap().cancel();
    }
    {
        let mut packet = layout.prepare(&mut planes).unwrap();
        assert!(packet.write_stereo(usize::MAX, &left, &right).is_err());
        assert!(packet.complete().is_err());
    }
    assert!(layout.prepare(&mut planes[..1]).is_err());
    COUNTING.with(|on| on.set(false));
    assert_eq!(OPERATIONS.with(|n| n.get()), 0);
    drop(planes);
    assert!(storage.iter().all(|byte| *byte == 128));
}
