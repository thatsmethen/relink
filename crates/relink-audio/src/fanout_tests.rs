use super::*;
use crate::tests::{COUNTING, OPERATIONS};

fn transport(routes: usize) -> Fanout {
    Fanout::build(routes, 48000, 41, Fanout::required_bytes(routes).unwrap()).unwrap()
}
fn publish(writer: &mut Writer, frames: &[[f32; 2]], routes: usize, version: u64) {
    let mut transaction = writer.begin(frames.len(), version).unwrap();
    for route in 0..routes {
        transaction.write(route, frames).unwrap();
    }
    transaction.commit().unwrap();
}

#[test]
fn checked_budget_covers_arrays_and_control_storage_and_rejects_before_allocation() {
    let one = Fanout::required_bytes(1).unwrap();
    let two = Fanout::required_bytes(2).unwrap();
    assert!(one > 2 * CAPACITY * 8 + Kernel::bytes());
    assert!(two - one > 2 * CAPACITY * 8);
    assert_eq!(Fanout::required_bytes(0), None);
    assert_eq!(Fanout::required_bytes(usize::MAX), None);
    OPERATIONS.with(|n| n.set(0));
    COUNTING.with(|on| on.set(true));
    let rejected = Fanout::build(1, 48000, 7, one - 1).err();
    let invalid = Fanout::build(usize::MAX, 48000, 7, usize::MAX).err();
    let invalid_rate = Fanout::build(1, 0, 7, one).err();
    COUNTING.with(|on| on.set(false));
    assert_eq!(OPERATIONS.with(|n| n.get()), 0);
    assert_eq!(rejected, Some(Error::BudgetExceeded));
    assert_eq!(invalid, Some(Error::InvalidConfiguration));
    assert_eq!(invalid_rate, Some(Error::InvalidConfiguration));
    let fanout = transport(2);
    assert_eq!(fanout.control.epoch(), 41);
    assert_eq!(fanout.writer.queues[0].pcm.len(), CAPACITY);
    assert_eq!(fanout.writer.queues[0].versions.len(), CAPACITY);
    assert!(Arc::ptr_eq(
        &fanout.readers[0].kernel,
        &fanout.readers[1].kernel
    ));
}

#[test]
fn delayed_first_read_retains_source_zero_impulse_and_exact_zero_prehistory() {
    let mut fanout = transport(2);
    let mut source = vec![[0.0; 2]; 8192];
    source[0] = [1.0, -0.5];
    for block in source.chunks(1024) {
        publish(&mut fanout.writer, block, 2, 7);
    }
    let mut left = [9.0; 256];
    let mut right = left;
    let (row, _, _) = fanout.readers[0].kernel.phase_rows(0.0);
    let expected = *row;
    for reader in &mut fanout.readers {
        let report = reader.read(&mut left, &mut right).unwrap();
        assert!(report.rendered);
        assert_eq!(report.source_start, 0);
        assert_eq!(report.source_end, 256);
        assert_eq!(report.target_frames, 8192);
        assert_eq!(report.accepted_version, None);
        assert!(left[0] > 0.9, "initial impulse was lost");
        for frame in 0..256 {
            let response = if frame < Kernel::half() {
                expected[Kernel::half() - 1 - frame]
            } else {
                0.0
            };
            assert_eq!(left[frame], response);
            assert_eq!(right[frame], -0.5 * response);
        }
    }
}

#[test]
fn no_route_observes_private_writes_while_producer_is_suspended_between_routes() {
    let Fanout {
        mut writer,
        mut readers,
        control,
    } = transport(2);
    let written = Arc::new(std::sync::Barrier::new(2));
    let resume = Arc::new(std::sync::Barrier::new(2));
    let producer_written = written.clone();
    let producer_resume = resume.clone();
    let thread = std::thread::spawn(move || {
        let pcm = vec![[0.25, -0.125]; 2048];
        let mut transaction = writer.begin(pcm.len(), 9).unwrap();
        transaction.write(0, &pcm).unwrap();
        producer_written.wait();
        producer_resume.wait();
        transaction.write(1, &pcm).unwrap();
        transaction.commit().unwrap();
        writer
    });
    written.wait();
    assert_eq!(control.committed_frames(), 0);
    assert_eq!(control.largest_committed_quantum(), 0);
    let mut left = [9.0; 256];
    let mut right = left;
    for reader in &mut readers {
        let report = reader.read(&mut left, &mut right).unwrap();
        assert!(!report.rendered);
        assert_eq!(report.accepted_version, None);
        assert_eq!(report.source_end, 0);
        assert!(left.iter().chain(&right).all(|&sample| sample == 0.0));
    }
    resume.wait();
    let mut writer = thread.join().unwrap();
    assert_eq!(control.committed_frames(), 2048);
    assert_eq!(control.largest_committed_quantum(), 2048);
    // Three whole source bursts provide the required producer-quantum margin.
    for _ in 0..2 {
        publish(&mut writer, &[[0.25, -0.125]; 2048], 2, 9);
    }
    for reader in &mut readers {
        assert!(reader.read(&mut left, &mut right).unwrap().rendered);
        for i in 128..256 {
            assert!((left[i] - 0.25).abs() < 1e-6);
            assert!((right[i] + 0.125).abs() < 1e-6);
        }
    }
}

#[test]
fn partial_failed_and_dropped_transactions_remain_invisible_and_shorter_retry_overwrites_bytes() {
    let mut fanout = transport(2);
    for _ in 0..4 {
        publish(&mut fanout.writer, &[[0.0; 2]; 512], 2, 1);
    }
    assert_eq!(fanout.control.largest_committed_quantum(), 512);
    {
        let mut transaction = fanout.writer.begin(1024, 2).unwrap();
        transaction.write(0, &[[7.0; 2]; 1024]).unwrap();
        transaction.write(1, &[[7.0; 2]; 512]).unwrap();
        assert_eq!(
            transaction.write(1, &[[7.0; 2]; 513]),
            Err(Error::InvalidBuffer)
        );
        assert_eq!(transaction.commit(), Err(Error::InvalidBuffer));
    }
    assert_eq!(fanout.control.committed_frames(), 2048);
    {
        let mut dropped = fanout.writer.begin(1024, 2).unwrap();
        dropped.write(0, &[[8.0; 2]; 1024]).unwrap();
    }
    assert_eq!(fanout.control.committed_frames(), 2048);
    let mut incomplete = fanout.writer.begin(512, 3).unwrap();
    incomplete.write(0, &[[9.0; 2]; 512]).unwrap();
    assert_eq!(incomplete.commit(), Err(Error::IncompleteTransaction));
    publish(&mut fanout.writer, &[[0.25, -0.25]; 512], 2, 3);
    assert_eq!(fanout.control.committed_frames(), 2560);
    assert_eq!(fanout.control.largest_committed_quantum(), 512);
    for queue in &fanout.writer.queues {
        for frame in 2048..2560 {
            assert_eq!(queue.frame(frame), ([0.25, -0.25], 3));
        }
    }
    // Remaining abandoned private values are never read through the commit bound.
    for reader in &mut fanout.readers {
        let mut left = [0.0; 128];
        let mut right = left;
        loop {
            match reader.read(&mut left, &mut right) {
                Ok(report) => {
                    assert!(report.rendered);
                    assert!(left.iter().chain(&right).all(|x| x.abs() < 0.4));
                }
                Err(Error::Underflow) => {
                    assert!(left.iter().chain(&right).all(|x| *x == 0.0));
                    break;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}

#[test]
fn cancellation_before_and_after_shared_commit_and_fresh_epoch_are_explicit() {
    for cancel_before in [true, false] {
        let mut fanout = transport(2);
        let mut transaction = fanout.writer.begin(2048, 5).unwrap();
        for route in 0..2 {
            transaction.write(route, &[[0.5; 2]; 2048]).unwrap();
        }
        if cancel_before {
            fanout.control.cancel();
        }
        assert_eq!(
            transaction.commit(),
            if cancel_before {
                Err(Error::Cancelled)
            } else {
                Ok(2048)
            }
        );
        fanout.control.cancel();
        assert!(fanout.control.cancelled());
        assert_eq!(
            fanout.control.largest_committed_quantum(),
            if cancel_before { 0 } else { 2048 }
        );
        assert_eq!(
            fanout.control.committed_frames(),
            if cancel_before { 0 } else { 2048 }
        );
        assert!(matches!(fanout.writer.begin(128, 6), Err(Error::Cancelled)));
        for reader in &mut fanout.readers {
            let mut left = [7.0; 256];
            let mut right = left;
            assert_eq!(reader.read(&mut left, &mut right), Err(Error::Cancelled));
            assert!(left.iter().chain(&right).all(|x| *x == 0.0));
        }
        let mut fresh = Fanout::build(1, 48000, 42, Fanout::required_bytes(1).unwrap()).unwrap();
        fanout.control.cancel(); // A stale handle cannot revoke a new epoch.
        assert!(!fresh.control.cancelled());
        assert_eq!(fresh.control.committed_frames(), 0);
        let mut left = [1.0; 256];
        let mut right = left;
        let report = fresh.readers[0].read(&mut left, &mut right).unwrap();
        assert!(!report.rendered);
        assert_eq!(report.accepted_version, None);
        assert!(left.iter().chain(&right).all(|x| *x == 0.0));
    }
}

#[test]
fn preflight_checks_every_route_and_cursor_overflow_before_private_writes() {
    let mut fanout = transport(2);
    for _ in 0..4 {
        publish(&mut fanout.writer, &[[0.125; 2]; 16384], 2, 1);
    }
    assert!(matches!(
        fanout.writer.begin(256, 2),
        Err(Error::RouteFull { route: 0 })
    ));
    // Route zero is read; route one's stopped reader keeps its history reserved.
    let mut left = [0.0; 512];
    let mut right = left;
    fanout.readers[0].read(&mut left, &mut right).unwrap();
    assert!(matches!(
        fanout.writer.begin(256, 2),
        Err(Error::RouteFull { route: 1 })
    ));
    assert_eq!(fanout.control.committed_frames(), CAPACITY as u64);
    assert_eq!(fanout.writer.queues[0].frame(0), ([0.125; 2], 1));
    let mut fresh = transport(1);
    fresh.writer.cursor = MAX_SOURCE_FRAMES - 1;
    fresh.writer.quantum = crate::MAX_CALLBACK_FRAMES;
    fresh.writer.shared.committed.store(
        published_word(MAX_SOURCE_FRAMES - 1, crate::MAX_CALLBACK_FRAMES),
        Ordering::Release,
    );
    fresh.writer.queues[0]
        .released
        .store(MAX_SOURCE_FRAMES - 1, Ordering::Release);
    assert!(matches!(fresh.writer.begin(2, 1), Err(Error::Overflow)));
    assert_eq!(fresh.writer.queues[0].frame(0), ([0.0; 2], 0));
    assert!(!fresh.control.cancelled());
    assert_eq!(
        fresh.control.largest_committed_quantum(),
        crate::MAX_CALLBACK_FRAMES
    );
    publish(&mut fresh.writer, &[[0.25; 2]], 1, 1);
    assert_eq!(fresh.control.committed_frames(), MAX_SOURCE_FRAMES);
    assert!(matches!(fresh.writer.begin(1, 1), Err(Error::Overflow)));
    fresh.control.cancel();
    assert_eq!(fresh.control.committed_frames(), MAX_SOURCE_FRAMES);
    assert_eq!(
        fresh.control.largest_committed_quantum(),
        crate::MAX_CALLBACK_FRAMES
    );
}

#[test]
fn callback_watermark_covers_all_fir_support_and_does_not_ack_prehistory_or_unrendered_versions() {
    let mut fanout = transport(1);
    let mut left = [0.0; 256];
    let mut right = left;
    let priming = fanout.readers[0].read(&mut left, &mut right).unwrap();
    assert_eq!(priming.accepted_version, None);
    for _ in 0..8 {
        publish(&mut fanout.writer, &[[0.25; 2]; 256], 1, 4);
    }
    // Versions 5–8 were coalesced away.
    for _ in 0..8 {
        publish(&mut fanout.writer, &[[0.5; 2]; 256], 1, 9);
    }
    let mut saw_new = false;
    for _ in 0..11 {
        let report = fanout.readers[0].read(&mut left, &mut right).unwrap();
        let oldest = report.source_start.checked_sub((Kernel::half() - 1) as u64);
        let expected = oldest.map(|oldest| if oldest < 2048 { 4 } else { 9 });
        assert_eq!(report.accepted_version, expected);
        saw_new |= expected == Some(9);
    }
    assert!(saw_new);
    assert!(matches!(
        fanout.writer.begin(128, 8),
        Err(Error::InvalidVersion)
    ));
}

#[test]
fn transactions_priming_first_render_and_fault_paths_allocate_and_free_nothing() {
    let mut fanout = transport(2);
    let block = [[0.125, -0.25]; 2048];
    let mut left = [0.0; 256];
    let mut right = left;
    OPERATIONS.with(|n| n.set(0));
    COUNTING.with(|on| on.set(true));
    for reader in &mut fanout.readers {
        reader.read(&mut left, &mut right).unwrap();
    }
    for chunk in block.chunks(512) {
        let mut transaction = fanout.writer.begin(chunk.len(), 3).unwrap();
        for route in 0..2 {
            transaction.write(route, &chunk[..73]).unwrap();
            transaction.write(route, &chunk[73..]).unwrap();
        }
        transaction.commit().unwrap();
    }
    for reader in &mut fanout.readers {
        assert!(reader.read(&mut left, &mut right).unwrap().rendered);
    }
    {
        let mut failed = fanout.writer.begin(32, 4).unwrap();
        assert_eq!(failed.write(2, &block[..32]), Err(Error::InvalidRoute));
        assert_eq!(failed.commit(), Err(Error::InvalidRoute));
    }
    fanout.control.cancel();
    assert!(matches!(fanout.writer.begin(32, 4), Err(Error::Cancelled)));
    for reader in &mut fanout.readers {
        assert_eq!(reader.read(&mut left, &mut right), Err(Error::Cancelled));
    }
    COUNTING.with(|on| on.set(false));
    assert_eq!(OPERATIONS.with(|n| n.get()), 0);
}

fn expected_pcm(frame: f64) -> [f32; 2] {
    [
        (frame * std::f64::consts::TAU * 440.0 / 48000.0).sin() as f32 * 0.25,
        (frame * std::f64::consts::TAU * 997.0 / 48000.0).sin() as f32 * 0.125,
    ]
}

#[test]
fn startup_uses_committed_primary_quantum_and_supports_unequal_variable_partitions_within_margin() {
    let mut fanout = transport(1);
    let block = [[0.25, -0.125]; 4096];
    let mut left = [0.0; 2048];
    let mut right = left;
    for burst in 1..=2 {
        publish(&mut fanout.writer, &block, 1, 2);
        let report = fanout.readers[0]
            .read(&mut left[..512], &mut right[..512])
            .unwrap();
        assert!(
            !report.rendered,
            "burst {burst} started with insufficient producer margin"
        );
        assert_eq!(report.producer_quantum, 4096);
        assert_eq!(report.source_end, 0);
        assert_eq!(report.accepted_version, None);
    }
    publish(&mut fanout.writer, &block, 1, 2);
    let mut previous_end = 0;
    for cycle in 0..128 {
        if cycle != 0 {
            // Whole primary callback sizes vary, but never exceed the committed
            // startup quantum. Follower packet partitions are unrelated.
            publish(&mut fanout.writer, &block[..1024], 1, 2);
            publish(&mut fanout.writer, &block[1024..], 1, 2);
        }
        for frames in [127, 385, 512, 1024, 2048] {
            let report = fanout.readers[0]
                .read(&mut left[..frames], &mut right[..frames])
                .unwrap();
            assert!(report.rendered);
            assert_eq!(report.source_start, previous_end);
            assert_eq!(report.producer_quantum, 4096);
            assert_eq!(report.target_frames, 12288);
            previous_end = report.source_end;
            if cycle != 0 {
                assert!(left[..frames].iter().all(|x| (*x - 0.25).abs() < 1e-6));
                assert!(right[..frames].iter().all(|x| (*x + 0.125).abs() < 1e-6));
            }
        }
    }
    assert!(previous_end > CAPACITY as u64 * 7);
}

#[test]
fn post_start_quantum_growth_beyond_target_is_explicit_and_cannot_skip_or_reprime() {
    let mut fanout = transport(1);
    for _ in 0..8 {
        publish(&mut fanout.writer, &[[0.25; 2]; 256], 1, 1);
    }
    let mut left = [0.0; 512];
    let mut right = left;
    let initial = fanout.readers[0].read(&mut left, &mut right).unwrap();
    assert!(initial.rendered);
    assert_eq!(initial.target_frames, 2048);
    let position = fanout.readers[0].position;
    publish(&mut fanout.writer, &[[0.25; 2]; 4096], 1, 2);
    assert_eq!(
        fanout.readers[0].read(&mut left, &mut right),
        Err(Error::QuantumExceeded {
            required_margin: 2 * 4096 + Kernel::taps() + Kernel::half(),
            startup_target: 2048,
        })
    );
    assert_eq!(fanout.readers[0].position, position);
    assert_eq!(fanout.readers[0].target, 2048);
    assert!(left.iter().chain(&right).all(|x| *x == 0.0));
}

#[test]
fn distinct_route_pcm_is_published_once_per_source_frame_and_stays_separate() {
    let mut fanout = transport(2);
    for _ in 0..8 {
        let mut transaction = fanout.writer.begin(256, 17).unwrap();
        transaction.write(1, &[[0.125, 0.5]; 256]).unwrap();
        transaction.write(0, &[[0.25, -0.125]; 256]).unwrap();
        transaction.commit().unwrap();
    }
    assert_eq!(fanout.control.committed_frames(), 2048);
    assert_eq!(fanout.control.largest_committed_quantum(), 256);
    let mut left = [0.0; 256];
    let mut right = left;
    for (route, expected) in [[0.25, -0.125], [0.125, 0.5]].into_iter().enumerate() {
        let report = fanout.readers[route].read(&mut left, &mut right).unwrap();
        assert_eq!(report.source_start, 0);
        assert_eq!(report.source_end, 256);
        for i in 128..256 {
            assert!((left[i] - expected[0]).abs() < 1e-6);
            assert!((right[i] - expected[1]).abs() < 1e-6);
        }
    }
}

#[test]
fn two_independent_clocks_preserve_continuous_expected_pcm_and_one_source_sequence_through_wraps() {
    let mut fanout = transport(2);
    let mut generated = 0;
    let mut block = [[0.0; 2]; 256];
    for _ in 0..8 {
        for (offset, frame) in block.iter_mut().enumerate() {
            *frame = expected_pcm((generated + offset) as f64);
        }
        publish(&mut fanout.writer, &block, 2, 1);
        generated += block.len();
    }
    let mut clocks = [0.0_f64; 2];
    let mut device_frames = [0_usize; 2];
    let mut previous = [[0.0_f32; 2]; 2];
    let mut worst_step = [0.0_f32; 2];
    let mut last_report = [None; 2];
    let mut left = [0.0; 258];
    let mut right = left;
    // Sixty seconds, two unrelated follower clocks simultaneously +/−1000 ppm.
    // Source frames are generated once per primary callback and fanned to both.
    for callback in 0..11250 {
        if callback % 257 == 0 {
            // Poison free slots across many wraps, then abort a larger callback.
            // The shorter valid callback below must replace all visible bytes;
            // neither its version nor largest quantum may inherit this attempt.
            {
                let mut aborted = fanout.writer.begin(1024, 99).unwrap();
                aborted.write(0, &[[7.0; 2]; 1024]).unwrap();
                aborted.write(1, &[[7.0; 2]; 73]).unwrap();
            }
            assert_eq!(fanout.control.largest_committed_quantum(), 256);
        }
        for (offset, frame) in block.iter_mut().enumerate() {
            *frame = expected_pcm((generated + offset) as f64);
        }
        let mut transaction = fanout.writer.begin(block.len(), 1).unwrap();
        for route in 0..2 {
            transaction.write(route, &block[..73]).unwrap();
            transaction.write(route, &block[73..]).unwrap();
        }
        transaction.commit().unwrap();
        generated += block.len();
        for (route, ppm) in [-1000.0, 1000.0].into_iter().enumerate() {
            clocks[route] += 256.0 * (1.0 + ppm / 1_000_000.0);
            let frames = clocks[route].floor() as usize - device_frames[route];
            device_frames[route] += frames;
            let reader = &mut fanout.readers[route];
            let fraction = reader.fraction;
            let report = reader
                .read(&mut left[..frames], &mut right[..frames])
                .unwrap();
            assert!(report.rendered, "route {route} callback {callback}");
            assert!(report.correction_ppm.abs() <= 2000.0 + 1e-9);
            if callback != 0 {
                for sample in 0..frames {
                    let frame = [left[sample], right[sample]];
                    for side in 0..2 {
                        worst_step[side] =
                            worst_step[side].max((frame[side] - previous[route][side]).abs());
                    }
                    previous[route] = frame;
                }
                // Independent analytical tone at the reported continuous read
                // phase detects source skips/repeats beyond mere smooth output.
                for offset in [0, frames / 2, frames - 1] {
                    let position = report.source_start as f64
                        + fraction
                        + offset as f64 * (1.0 + report.correction_ppm / 1_000_000.0);
                    let expected = expected_pcm(position);
                    assert!(
                        (left[offset] - expected[0]).abs() < 0.00001,
                        "left route {route} callback {callback}"
                    );
                    assert!(
                        (right[offset] - expected[1]).abs() < 0.00001,
                        "right route {route} callback {callback}"
                    );
                }
            } else {
                previous[route] = [left[frames - 1], right[frames - 1]];
            }
            last_report[route] = Some(report);
        }
    }
    assert_eq!(fanout.control.committed_frames(), generated as u64);
    assert!(generated > CAPACITY * 40);
    assert!(worst_step[0] < 0.015, "left step {}", worst_step[0]);
    assert!(worst_step[1] < 0.017, "right step {}", worst_step[1]);
    for (route, ppm) in [-1000.0, 1000.0].into_iter().enumerate() {
        let report = last_report[route].unwrap();
        let expected_correction = (1.0 / (1.0 + ppm / 1_000_000.0) - 1.0) * 1_000_000.0;
        assert!(
            (report.correction_ppm - expected_correction).abs() < 80.0,
            "route {route}: requested {expected_correction}, observed {}",
            report.correction_ppm
        );
        assert!(
            (report.buffered_frames as i64 - report.target_frames as i64).abs() < 140,
            "route {route}: buffered {} target {}",
            report.buffered_frames,
            report.target_frames
        );
        assert_eq!(report.accepted_version, Some(1));
    }
}
