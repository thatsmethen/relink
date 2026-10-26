use super::native_packet::*;
use super::{
    AudioFault, NativeCallbackCompletion, NativeCallbackOutcome, NativeCapturePacket,
    NativeLifecycleEvent, NativeLifecyclePhase,
};
use std::sync::{Arc, Barrier};

fn float_format(channels: u32, planar: bool) -> Format {
    let bytes = if planar { 4 } else { 4 * channels };
    Format {
        rate: 48000.0,
        id: LINEAR_PCM,
        flags: FLOAT | PACKED | if planar { NON_INTERLEAVED } else { 0 },
        packet_bytes: bytes,
        packet_frames: 1,
        frame_bytes: bytes,
        channels,
        bits: 32,
    }
}
fn packet(index: u64, sample: f64, frames: u32) -> NativeCapturePacket {
    NativeCapturePacket {
        packet_index: index,
        queue_frame: (index - 1) * 512,
        sample_time: sample,
        host_time: index * 1_000_000,
        observed_host_time: index * 1_000_000 + 123,
        rate_scalar: 1.0,
        flags: 7,
        frames,
        epoch: 42,
    }
}
fn completion(index: u64) -> NativeCallbackCompletion {
    NativeCallbackCompletion {
        packet_index: index,
        entry_host_time: index * 100,
        after_pcm_host_time: index * 100 + 1,
        completion_host_time: index * 100 + 2,
        os_now_sample_time: index as f64 * 512.0,
        os_now_host_time: index * 100 - 1,
        os_now_flags: 3,
        outcome: if index.is_multiple_of(2) {
            NativeCallbackOutcome::Published
        } else {
            NativeCallbackOutcome::Rejected(AudioFault::Xrun)
        },
    }
}

#[test]
fn native_pcm_layout_accepts_exact_rates_and_rejects_ambiguous_formats() {
    assert!(Layout::new(&[float_format(2, false)], 48000).is_ok());
    assert!(Layout::new(&[float_format(2, false)], 44100).is_err());
    for case in 0..11 {
        let mut f = float_format(2, false);
        match case {
            0 => f.flags |= SIGNED,
            1 => f.flags |= 1 << 20,
            2 => f.id = 0,
            3 => f.packet_frames = 2,
            4 => f.packet_bytes = 16,
            5 => f.frame_bytes = 0,
            6 => f.channels = 0,
            7 => f.channels = 257,
            8 => f.bits = 24,
            9 => {
                f.channels = 1;
                f.frame_bytes = u32::MAX;
                f.packet_bytes = u32::MAX;
            }
            10 => f.flags |= 64, // Nonmixable formats may automatically acquire hog mode.
            _ => unreachable!(),
        }
        assert!(Layout::new(&[f], 48000).is_err(), "case {case}");
    }
    assert!(Layout::new(&[], 48000).is_err());
}

#[test]
fn native_stereo_mapping_handles_interleaved_planar_mono_and_multiple_formats() {
    let interleaved: Vec<u8> = [0.25_f32, -0.5, 99.0, 0.75, -1.0, 88.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let layout = Layout::new(&[float_format(3, false)], 48000).unwrap();
    assert_eq!(
        layout.validate_sizes([(3, interleaved.len(), true)].into_iter()),
        Ok(2)
    );
    let reader = layout.stereo_reader(|_| &interleaved);
    assert_eq!(reader.frame(0), [0.25, -0.5]);
    assert_eq!(reader.frame(1), [0.75, -1.0]);

    let left: Vec<u8> = [0.25_f32, 0.75]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let right: Vec<u8> = [-0.5_f32, -1.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let planar = Layout::new(&[float_format(2, true)], 48000).unwrap();
    assert_eq!(
        planar.validate_sizes([(1, 8, true), (1, 8, true)].into_iter()),
        Ok(2)
    );
    assert_eq!(
        planar
            .stereo_reader(|i| if i == 0 { &left } else { &right })
            .frame(1),
        [0.75, -1.0]
    );
    let mono = Layout::new(&[float_format(1, false)], 48000).unwrap();
    assert_eq!(mono.stereo_reader(|_| &left).frame(1), [0.75; 2]);

    let int_format = Format {
        flags: SIGNED | PACKED,
        packet_bytes: 2,
        frame_bytes: 2,
        bits: 16,
        ..float_format(1, false)
    };
    let mixed = Layout::new(&[int_format, float_format(1, false)], 48000).unwrap();
    let signed = 16384_i16.to_le_bytes();
    assert_eq!(
        mixed.validate_sizes([(1, 2, true), (1, 4, true)].into_iter()),
        Ok(1)
    );
    assert_eq!(
        mixed
            .stereo_reader(|i| if i == 0 { &signed } else { &right[..4] })
            .frame(0),
        [0.5, -0.5]
    );
}

#[test]
fn native_integer_alignment_endianness_and_nonfinite_float_decoding_are_explicit() {
    for high in [false, true] {
        for big in [false, true] {
            let f = Format {
                flags: SIGNED
                    | if high { HIGH_ALIGNED } else { 0 }
                    | if big { BIG_ENDIAN } else { 0 },
                bits: 24,
                ..float_format(1, false)
            };
            let layout = Layout::new(&[f], 48000).unwrap();
            let value = if high {
                -1_073_741_824_i32
            } else {
                -4_194_304_i32
            };
            let bytes = if big {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            };
            assert_eq!(layout.stereo_reader(|_| &bytes).frame(0), [-0.5; 2]);
        }
    }
    let f64_format = Format {
        frame_bytes: 8,
        packet_bytes: 8,
        bits: 64,
        ..float_format(1, false)
    };
    let f64_layout = Layout::new(&[f64_format], 48000).unwrap();
    let finite = 0.125_f64.to_le_bytes();
    assert_eq!(f64_layout.stereo_reader(|_| &finite).frame(0), [0.125; 2]);
    let nan = f64::NAN.to_le_bytes();
    assert_eq!(f64_layout.stereo_reader(|_| &nan).frame(0), [0.0; 2]);
}

#[test]
fn bounded_callback_size_changes_do_not_allocate_or_require_format_changes() {
    let layout = Layout::new(&[float_format(2, true)], 48000).unwrap();
    for frames in [1, 127, 512, 4096, super::MAX_CALLBACK_FRAMES] {
        assert_eq!(
            layout.validate_sizes([(1, frames * 4, true), (1, frames * 4, true)].into_iter()),
            Ok(frames)
        );
    }
    for sizes in [
        vec![(1, 4, true)],
        vec![(1, 4, false), (1, 4, true)],
        vec![(1, 4, true), (1, 8, true)],
        vec![(2, 4, true), (1, 4, true)],
        vec![(1, 3, true), (1, 3, true)],
        vec![(1, 0, true), (1, 0, true)],
        vec![(1, (super::MAX_CALLBACK_FRAMES + 1) * 4, true); 2],
    ] {
        assert_eq!(
            layout.validate_sizes(sizes.into_iter()),
            Err(AudioFault::InvalidBuffer)
        );
    }
}

#[test]
fn native_sample_continuity_is_independent_of_real_drifting_host_clocks() {
    for ppm in [-2000.0_f64, -1000.0, 1000.0, 2000.0] {
        let mut tracker = Tracker::default();
        let mut frame = -512.25;
        for index in 1..=30000 {
            let frames = [127, 512, 1024][index as usize % 3];
            let mut current = packet(index, frame, frames);
            current.host_time =
                ((frame + 1000.0) / (48000.0 * (1.0 + ppm / 1_000_000.0)) * 1e9) as u64 + 1;
            current.rate_scalar = 1.0 / (1.0 + ppm / 1_000_000.0);
            assert_eq!(tracker.check(&current), Ok(()));
            tracker.commit(current);
            frame += f64::from(frames);
        }
    }
}

#[test]
fn first_native_sample_gap_overlap_duplicate_and_bad_clock_are_rejected_without_reanchoring() {
    let mut tracker = Tracker::default();
    let first = packet(1, 50944.0, 512);
    tracker.check(&first).unwrap();
    tracker.commit(first);
    for displacement in [1.0, 568.0, -1.0, -512.0] {
        let next = packet(2, 50944.0 + 512.0 + displacement, 512);
        assert_eq!(tracker.check(&next), Err(AudioFault::Xrun));
        assert_eq!(tracker.previous, Some(first));
    }
    let normal = packet(2, 50944.0 + 512.0, 256);
    for index in 0..7 {
        let mut invalid = normal;
        match index {
            0 => invalid.flags = 0,
            1 => invalid.sample_time = f64::NAN,
            2 => invalid.host_time = first.host_time,
            3 => invalid.rate_scalar = f64::NAN,
            4 => invalid.epoch += 1,
            5 => invalid.sample_time = f64::MAX,
            6 => invalid.host_time = 0,
            _ => unreachable!(),
        }
        assert_eq!(tracker.check(&invalid), Err(AudioFault::CaptureTimestamp));
    }
    assert_eq!(tracker.check(&normal), Ok(()));
}

#[test]
fn native_fault_pair_is_preserved_independently_of_subsequent_packets() {
    let ring = super::capture_transport::Ring::new();
    let evidence = &ring.native_timing;
    assert!(evidence.snapshot().is_none());
    let first = packet(1, 50944.0, 512);
    let gap = packet(2, 52024.0, 512);
    evidence.record(first, None, None);
    evidence.record(gap, Some(first), Some(AudioFault::Xrun));
    evidence.record(
        packet(3, 52536.0, 512),
        Some(gap),
        Some(AudioFault::CaptureTimestamp),
    );
    let observed = evidence.snapshot().unwrap();
    assert_eq!(observed.packets_seen, 3);
    assert_eq!(observed.first_fault.unwrap().previous, Some(first));
    assert_eq!(observed.first_fault.unwrap().current, gap);
    assert_eq!(observed.first_fault.unwrap().fault, AudioFault::Xrun);
}

#[test]
fn concurrent_native_metadata_snapshots_never_accept_a_torn_packet() {
    let evidence = Arc::new(Evidence::new());
    let barrier = Arc::new(Barrier::new(2));
    let worker = evidence.clone();
    let start = barrier.clone();
    let writer = std::thread::spawn(move || {
        start.wait();
        for index in 1..20000 {
            worker.record(
                packet(index, index as f64, 512),
                None,
                (index == 10000).then_some(AudioFault::Xrun),
            );
            worker.complete(completion(index));
        }
    });
    barrier.wait();
    for _ in 0..20000 {
        if let Some(snapshot) = evidence.snapshot() {
            if let Some(p) = snapshot.last_packet {
                assert_eq!(p, packet(p.packet_index, p.packet_index as f64, 512));
            }
            if let Some(fault) = snapshot.first_fault {
                assert_eq!(fault.current, packet(10000, 10000.0, 512));
            }
            if let Some(c) = snapshot.last_completed_callback {
                assert_eq!(c, completion(c.packet_index));
            }
        }
    }
    writer.join().unwrap();
    assert_eq!(
        evidence
            .snapshot()
            .unwrap()
            .last_packet
            .unwrap()
            .packet_index,
        19999
    );
    assert_eq!(
        evidence.snapshot().unwrap().last_completed_callback,
        Some(completion(19999))
    );
}

#[test]
fn native_lifecycle_records_preserve_first_status_and_expose_unreturned_operations() {
    let evidence = Evidence::new();
    let request = NativeLifecycleEvent {
        phase: NativeLifecyclePhase::CreateRequested,
        host_time: 1000,
        status: None,
        callbacks_in_flight: Some(0),
    };
    evidence.lifecycle(request);
    let observed = evidence.snapshot().unwrap();
    assert_eq!(
        observed.lifecycle[NativeLifecyclePhase::CreateRequested as usize],
        Some(request)
    );
    assert_eq!(
        observed.lifecycle[NativeLifecyclePhase::CreateReturned as usize],
        None
    );
    assert_eq!(observed.last_packet, None);
    let failed = NativeLifecycleEvent {
        phase: NativeLifecyclePhase::CreateReturned,
        host_time: 2000,
        status: Some(-10001),
        callbacks_in_flight: Some(0),
    };
    evidence.lifecycle(failed);
    evidence.lifecycle(NativeLifecycleEvent {
        host_time: 3000,
        status: Some(0),
        ..failed
    });
    assert_eq!(
        evidence.snapshot().unwrap().lifecycle[NativeLifecyclePhase::CreateReturned as usize],
        Some(failed)
    );
    let cancel = NativeLifecycleEvent {
        phase: NativeLifecyclePhase::HandleCancelled,
        host_time: 4000,
        status: None,
        callbacks_in_flight: None,
    };
    evidence.lifecycle(cancel);
    assert_eq!(
        evidence.snapshot().unwrap().lifecycle[NativeLifecyclePhase::HandleCancelled as usize],
        Some(cancel)
    );
    evidence.complete(NativeCallbackCompletion {
        outcome: NativeCallbackOutcome::Cancelled,
        ..completion(1)
    });
    assert_eq!(
        evidence
            .snapshot()
            .unwrap()
            .last_completed_callback
            .unwrap()
            .outcome,
        NativeCallbackOutcome::Cancelled
    );
}
