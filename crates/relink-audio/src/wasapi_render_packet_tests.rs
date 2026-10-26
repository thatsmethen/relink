use super::*;

fn layout(encoding: Encoding, channels: u16) -> Layout {
    Layout { channels, encoding }
}

#[test]
fn independent_golden_bytes_cover_all_encodings_and_extra_channel_equilibrium() {
    // Each row is one four-channel frame: L=+0.5, R=-0.5, two silent channels.
    let cases: &[(Encoding, &[u8])] = &[
        (
            Encoding::F32,
            &[0, 0, 0, 0x3f, 0, 0, 0, 0xbf, 0, 0, 0, 0, 0, 0, 0, 0],
        ),
        (
            Encoding::F64,
            &[
                0, 0, 0, 0, 0, 0, 0xe0, 0x3f, 0, 0, 0, 0, 0, 0, 0xe0, 0xbf, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0,
            ],
        ),
        (Encoding::I16, &[0, 0x40, 0, 0xc0, 0, 0, 0, 0]),
        (
            Encoding::I24In32,
            &[0, 0, 0, 0x40, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0, 0],
        ),
        (
            Encoding::I32,
            &[0, 0, 0, 0x40, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0, 0],
        ),
        (Encoding::U8, &[192, 64, 128, 128]),
    ];
    for &(encoding, expected) in cases {
        // Deliberately offset storage; encoding must not require typed alignment.
        let mut storage = [0x55; 35];
        let bytes = &mut storage[1..expected.len() + 1];
        let mut packet = Packet::new(layout(encoding, 4), 1, bytes).unwrap();
        packet.write_stereo(0, &[0.5], &[-0.5]).unwrap();
        assert_eq!(packet.complete(), Ok(()));
        assert_eq!(packet.bytes, expected, "{encoding:?}");
        assert_eq!(storage[0], 0x55);
        assert_eq!(storage[expected.len() + 1], 0x55);
    }
}

#[test]
fn saturation_extrema_and_nonfinite_samples_have_independent_expected_bytes() {
    type Extrema<'a> = (Encoding, &'a [u8], &'a [u8], &'a [u8]);
    let cases: &[Extrema<'_>] = &[
        (
            Encoding::F32,
            &[0, 0, 0x80, 0xbf],
            &[0, 0, 0x80, 0x3f],
            &[0; 4],
        ),
        (
            Encoding::F64,
            &[0, 0, 0, 0, 0, 0, 0xf0, 0xbf],
            &[0, 0, 0, 0, 0, 0, 0xf0, 0x3f],
            &[0; 8],
        ),
        (Encoding::I16, &[0, 0x80], &[0xff, 0x7f], &[0; 2]),
        (
            Encoding::I24In32,
            &[0, 0, 0, 0x80],
            &[0, 0xff, 0xff, 0x7f],
            &[0; 4],
        ),
        (
            Encoding::I32,
            &[0, 0, 0, 0x80],
            &[0xff, 0xff, 0xff, 0x7f],
            &[0; 4],
        ),
        (Encoding::U8, &[0], &[255], &[128]),
    ];
    for &(encoding, minimum, maximum, silence) in cases {
        let width = encoding.bytes();
        let mut bytes = [0x55; 8];
        for value in [-1.0, -2.0, -f32::MAX] {
            encode(encoding, value, &mut bytes[..width]);
            assert_eq!(&bytes[..width], minimum, "{encoding:?} {value}");
        }
        for value in [1.0, 2.0, f32::MAX] {
            encode(encoding, value, &mut bytes[..width]);
            assert_eq!(&bytes[..width], maximum, "{encoding:?} {value}");
        }
        for value in [0.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            encode(encoding, value, &mut bytes[..width]);
            assert_eq!(&bytes[..width], silence, "{encoding:?} {value}");
        }
    }
}

#[test]
fn integer_rounding_valid_bit_alignment_and_f32_precision_boundaries_are_explicit() {
    let mut bytes = [0; 4];
    for (value, expected) in [
        (0.5 / 32768.0, [1, 0]),
        (-0.5 / 32768.0, [0xff, 0xff]),
        (0.49 / 32768.0, [0, 0]),
        (-0.49 / 32768.0, [0, 0]),
    ] {
        encode(Encoding::I16, value, &mut bytes[..2]);
        assert_eq!(&bytes[..2], &expected);
    }
    for (value, expected) in [
        (0.5 / 8_388_608.0, [0, 1, 0, 0]),
        (-0.5 / 8_388_608.0, [0, 0xff, 0xff, 0xff]),
        (1.0 / 8_388_608.0, [0, 1, 0, 0]),
        (-1.0 / 8_388_608.0, [0, 0xff, 0xff, 0xff]),
    ] {
        encode(Encoding::I24In32, value, &mut bytes);
        assert_eq!(bytes, expected);
        assert_eq!(bytes[0], 0, "all padding bits must be zero");
    }
    encode(
        Encoding::I32,
        f32::from_bits(1.0_f32.to_bits() - 1),
        &mut bytes,
    );
    assert_eq!(bytes, [0x80, 0xff, 0xff, 0x7f]);
    encode(
        Encoding::I32,
        -f32::from_bits(1.0_f32.to_bits() - 1),
        &mut bytes,
    );
    assert_eq!(bytes, [0x80, 0, 0, 0x80]);
    encode(Encoding::U8, 0.5 / 128.0, &mut bytes[..1]);
    assert_eq!(bytes[0], 129);
    encode(Encoding::U8, -0.5 / 128.0, &mut bytes[..1]);
    assert_eq!(bytes[0], 128); // Round the unsigned quantized value, 127.5.
}

#[test]
fn complete_chunks_cover_every_frame_and_clear_resets_progress_and_u8_equilibrium() {
    let mut bytes = [7; 24];
    let mut packet = Packet::new(layout(Encoding::U8, 4), 6, &mut bytes).unwrap();
    assert!(packet.bytes.iter().all(|x| *x == 128));
    packet.write_stereo(0, &[0.5; 2], &[-0.5; 2]).unwrap();
    packet.write_stereo(2, &[], &[]).unwrap();
    assert_eq!(packet.complete(), Err(Error::Incomplete));
    packet.write_stereo(2, &[1.0; 4], &[-1.0; 4]).unwrap();
    assert_eq!(packet.complete(), Ok(()));
    assert_eq!(&packet.bytes[..8], &[192, 64, 128, 128, 192, 64, 128, 128]);
    assert_eq!(&packet.bytes[8..], &[255, 0, 128, 128].repeat(4));
    packet.clear();
    assert!(packet.bytes.iter().all(|x| *x == 128));
    assert_eq!(packet.complete(), Err(Error::Incomplete));
}

#[test]
fn malformed_extents_are_unchanged_and_invalid_chunks_poison_completion() {
    for (channels, frames, length) in [
        (0, 1, 8),
        (1, 1, 4),
        (257, 1, 8),
        (2, 0, 0),
        (2, crate::MAX_CALLBACK_FRAMES + 1, 8),
        (2, usize::MAX, 8),
        (2, 1, 7),
        (2, 1, 9),
    ] {
        let mut bytes = [77; 16];
        assert!(matches!(
            Packet::new(
                layout(Encoding::F32, channels),
                frames,
                &mut bytes[..length]
            ),
            Err(Error::InvalidBuffer)
        ));
        assert!(bytes.iter().all(|x| *x == 77));
    }
    for (start, left, right) in [
        (1, &[0.5][..], &[-0.5][..]),
        (0, &[0.5][..], &[][..]),
        (0, &[0.5; 3][..], &[-0.5; 3][..]),
        (usize::MAX, &[][..], &[][..]),
    ] {
        let mut bytes = [77; 16];
        let mut packet = Packet::new(layout(Encoding::F32, 2), 2, &mut bytes).unwrap();
        assert_eq!(
            packet.write_stereo(start, left, right),
            Err(Error::InvalidBuffer)
        );
        assert!(packet.bytes.iter().all(|x| *x == 0));
        packet.clear();
        assert_eq!(
            packet.write_stereo(0, &[0.5; 2], &[-0.5; 2]),
            Err(Error::InvalidBuffer)
        );
        assert_eq!(packet.complete(), Err(Error::InvalidBuffer));
    }
}

#[test]
fn release_once_uses_pcm_only_for_complete_authorized_commit() {
    for (prepared, authorized, cancel, expect_flags, expected) in [
        (true, true, false, 0, Ok(())),
        (
            false,
            true,
            false,
            SILENT,
            Err(CommitError::Preparation(Error::Incomplete)),
        ),
        (
            true,
            false,
            false,
            SILENT,
            Err(CommitError::Preparation(Error::Cancelled)),
        ),
        (
            true,
            true,
            true,
            SILENT,
            Err(CommitError::Preparation(Error::Cancelled)),
        ),
    ] {
        let calls = Cell::new(0);
        let record = Cell::new(None);
        let mut bytes = [7; 16];
        let mut lease = RenderLease::new(
            2,
            layout(Encoding::F32, 2),
            Ok(&mut bytes),
            &record,
            |frames, flags| {
                calls.set(calls.get() + 1);
                assert_eq!(frames, 2);
                assert_eq!(flags, expect_flags);
                Ok::<_, i32>(())
            },
        )
        .unwrap();
        assert_eq!(lease.frames(), 2);
        lease.write_stereo(0, &[0.25], &[-0.25]).unwrap();
        if prepared {
            lease.write_stereo(1, &[0.5], &[-0.5]).unwrap();
        }
        if cancel {
            lease.cancel();
            assert_eq!(lease.write_stereo(0, &[], &[]), Err(Error::Cancelled));
        }
        assert_eq!(lease.commit(authorized), expected);
        assert_eq!(calls.get(), 1);
        assert_eq!(
            record.get(),
            Some(ReleaseRecord {
                frames: 2,
                flags: expect_flags,
                result: Ok(())
            })
        );
    }
}

#[test]
fn default_drop_and_explicit_silence_release_the_entire_acquisition_even_when_pcm_is_complete() {
    for explicit in [false, true] {
        let calls = Cell::new(0);
        let record = Cell::new(None);
        let mut bytes = [7; 4];
        let mut lease = RenderLease::new(
            2,
            layout(Encoding::U8, 2),
            Ok(&mut bytes),
            &record,
            |frames, flags| {
                calls.set(calls.get() + 1);
                assert_eq!((frames, flags), (2, SILENT));
                Ok::<_, i32>(())
            },
        )
        .unwrap();
        lease.write_stereo(0, &[0.5; 2], &[-0.5; 2]).unwrap();
        if explicit {
            lease.finish_silent().unwrap();
        } else {
            drop(lease);
        }
        assert_eq!(calls.get(), 1);
        assert_eq!(record.get().unwrap().result, Ok(()));
    }
}

#[test]
fn release_failures_remain_observable_on_commit_error_drop_and_constructor_failure_without_retry() {
    for path in 0..5 {
        let calls = Cell::new(0);
        let record = Cell::new(None);
        let mut bytes = [7; 16];
        let lease = RenderLease::new(
            2,
            layout(Encoding::F32, if path == 4 { 1 } else { 2 }),
            Ok(&mut bytes),
            &record,
            |frames, _flags| {
                calls.set(calls.get() + 1);
                assert_eq!(frames, 2);
                Err(-1234_i32)
            },
        );
        if path == 4 {
            assert!(matches!(lease, Err(Error::InvalidBuffer)));
        } else {
            let mut lease = lease.unwrap();
            match path {
                0 => {
                    lease.write_stereo(0, &[0.5; 2], &[-0.5; 2]).unwrap();
                    assert_eq!(lease.commit(true), Err(CommitError::Release(-1234)));
                }
                1 => assert_eq!(
                    lease.commit(true),
                    Err(CommitError::Preparation(Error::Incomplete))
                ),
                2 => drop(lease),
                3 => assert_eq!(lease.finish_silent(), Err(-1234)),
                _ => unreachable!(),
            }
        }
        assert_eq!(calls.get(), 1);
        assert_eq!(
            record.get().unwrap(),
            ReleaseRecord {
                frames: 2,
                flags: if path == 0 { 0 } else { SILENT },
                result: Err(-1234),
            }
        );
    }
}

#[test]
fn ignored_bad_write_and_zero_or_oversized_acquisitions_cannot_commit_partial_pcm() {
    let record = Cell::new(None);
    let mut bytes = [0; 16];
    let mut lease = RenderLease::new(
        2,
        layout(Encoding::F32, 2),
        Ok(&mut bytes),
        &record,
        |frames, flags| {
            assert_eq!((frames, flags), (2, SILENT));
            Ok::<_, i32>(())
        },
    )
    .unwrap();
    lease.write_stereo(0, &[0.5], &[-0.5]).unwrap();
    assert_eq!(
        lease.write_stereo(1, &[0.5; 2], &[-0.5; 2]),
        Err(Error::InvalidBuffer)
    );
    lease.clear();
    assert_eq!(
        lease.commit(true),
        Err(CommitError::Preparation(Error::InvalidBuffer))
    );
    for frames in [0, crate::MAX_CALLBACK_FRAMES as u32 + 1, u32::MAX] {
        let calls = Cell::new(0);
        assert!(matches!(
            RenderLease::new(
                frames,
                layout(Encoding::F32, 2),
                Ok(&mut []),
                &record,
                |count, flags| {
                    calls.set(calls.get() + 1);
                    assert_eq!((count, flags), (frames, SILENT));
                    Ok::<_, i32>(())
                }
            ),
            Err(Error::InvalidBuffer)
        ));
        assert_eq!(calls.get(), 1);
    }
}

#[test]
fn rejected_native_pointer_releases_without_forming_or_touching_a_slice() {
    let record = Cell::new(None);
    let calls = Cell::new(0);
    let result = RenderLease::new(
        4096,
        layout(Encoding::F32, 2),
        Err(Error::InvalidBuffer),
        &record,
        |frames, flags| {
            calls.set(calls.get() + 1);
            assert_eq!((frames, flags), (4096, SILENT));
            Err(-77_i32)
        },
    );
    assert!(matches!(result, Err(Error::InvalidBuffer)));
    assert_eq!(calls.get(), 1);
    assert_eq!(record.get().unwrap().result, Err(-77));
}

#[test]
fn owner_unwind_releases_silence_once_and_retains_failure() {
    let record = Cell::new(None);
    let calls = Cell::new(0);
    let mut bytes = [0; 16];
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut lease = RenderLease::new(
            2,
            layout(Encoding::F32, 2),
            Ok(&mut bytes),
            &record,
            |frames, flags| {
                calls.set(calls.get() + 1);
                assert_eq!((frames, flags), (2, SILENT));
                Err(-55_i32)
            },
        )
        .unwrap();
        lease.write_stereo(0, &[0.25; 2], &[-0.25; 2]).unwrap();
        panic!("synthetic owner unwind after final encoding chunk");
    }));
    assert!(caught.is_err());
    assert_eq!(calls.get(), 1);
    assert_eq!(record.get().unwrap().result, Err(-55));
}

#[test]
fn lease_cannot_be_sent_or_shared_across_owner_threads() {
    // Inference is deliberately ambiguous if a second impl becomes applicable.
    // The function-pointer release callback itself is Send + Sync.
    trait UnlessSend<T> {
        fn check() {}
    }
    impl<T: ?Sized> UnlessSend<()> for T {}
    impl<T: ?Sized + Send> UnlessSend<u8> for T {}
    trait UnlessSync<T> {
        fn check() {}
    }
    impl<T: ?Sized> UnlessSync<()> for T {}
    impl<T: ?Sized + Sync> UnlessSync<u8> for T {}
    type Lease = RenderLease<'static, 'static, fn(u32, u32) -> Result<(), i32>, i32>;
    let _ = <Lease as UnlessSend<_>>::check;
    let _ = <Lease as UnlessSync<_>>::check;
}

#[test]
fn neutral_sink_writes_do_not_submit_and_late_validation_error_forces_silent_release() {
    use crate::output_sink::{InvalidOutputBuffer, StereoSink};
    let record = Cell::new(None);
    let calls = Cell::new(0);
    let mut bytes = [7; 16];
    let mut lease = RenderLease::new(
        2,
        layout(Encoding::F32, 2),
        Ok(&mut bytes),
        &record,
        |frames, flags| {
            calls.set(calls.get() + 1);
            assert_eq!((frames, flags), (2, SILENT));
            Ok::<_, i32>(())
        },
    )
    .unwrap();
    {
        let sink: &mut dyn StereoSink = &mut lease;
        assert_eq!(sink.frames(), Ok(2));
        sink.clear();
        sink.write_stereo(0, &[0.5; 2], &[-0.5; 2]).unwrap();
        assert_eq!(calls.get(), 0, "encoding is not submission");
        assert_eq!(
            sink.write_stereo(2, &[0.25], &[0.25]),
            Err(InvalidOutputBuffer)
        );
    }
    assert_eq!(
        lease.commit(true),
        Err(CommitError::Preparation(Error::InvalidBuffer))
    );
    assert_eq!(calls.get(), 1);
}

#[test]
fn packet_and_lease_success_error_cancellation_drop_paths_allocate_and_free_nothing() {
    use crate::tests::{COUNTING, OPERATIONS};
    let record = Cell::new(None);
    let calls = Cell::new(0);
    let mut bytes = [7; 256];
    OPERATIONS.with(|n| n.set(0));
    COUNTING.with(|n| n.set(true));
    for encoding in [
        Encoding::F32,
        Encoding::F64,
        Encoding::I16,
        Encoding::I24In32,
        Encoding::I32,
        Encoding::U8,
    ] {
        for path in 0..5 {
            let mut lease = RenderLease::new(
                8,
                layout(encoding, 4),
                Ok(&mut bytes[..32 * encoding.bytes()]),
                &record,
                |_, _| {
                    calls.set(calls.get() + 1);
                    Ok::<_, i32>(())
                },
            )
            .unwrap();
            lease.write_stereo(0, &[0.25; 3], &[-0.25; 3]).unwrap();
            lease.write_stereo(3, &[0.5; 5], &[-0.5; 5]).unwrap();
            match path {
                0 => lease.commit(true).unwrap(),
                1 => assert_eq!(
                    lease.commit(false),
                    Err(CommitError::Preparation(Error::Cancelled))
                ),
                2 => {
                    lease.cancel();
                    drop(lease);
                }
                3 => {
                    assert_eq!(lease.write_stereo(0, &[], &[]), Err(Error::InvalidBuffer));
                    drop(lease);
                }
                4 => lease.finish_silent().unwrap(),
                _ => unreachable!(),
            }
        }
    }
    assert!(matches!(
        RenderLease::new(
            8,
            layout(Encoding::F32, 2),
            Err(Error::InvalidBuffer),
            &record,
            |_, _| {
                calls.set(calls.get() + 1);
                Err(-1_i32)
            }
        ),
        Err(Error::InvalidBuffer)
    ));
    COUNTING.with(|n| n.set(false));
    assert_eq!(OPERATIONS.with(Cell::get), 0);
    assert_eq!(calls.get(), 31);
}
