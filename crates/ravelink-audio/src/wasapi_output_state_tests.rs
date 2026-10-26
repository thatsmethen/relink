use super::*;
use std::sync::{Arc, Barrier};

#[test]
fn activation_requires_ready_native_start_and_post_start_progress() {
    let gates = Gates::default();
    assert!(!gates.request_start());
    assert!(!gates.activate(1));
    gates.ready();
    assert!(gates.request_start());
    assert!(!gates.activate(1));
    gates.started();
    assert!(!gates.activate(0));
    assert!(gates.activate(1));
    assert!(gates.program());
    gates.cancel();
    assert!(!gates.program());
    assert!(!gates.activate(10));
}

#[test]
fn cancellation_before_or_during_late_start_is_terminal() {
    for point in 0..3 {
        let gates = Gates::default();
        if point == 0 {
            gates.cancel();
        }
        gates.ready();
        let requested = gates.request_start();
        if point == 1 {
            gates.cancel();
        }
        gates.started(); // Simulates a late successful native return.
        if point == 2 {
            gates.cancel();
        }
        assert_eq!(requested, point != 0);
        assert!(gates.is_started(), "diagnostic success remains visible");
        assert!(!gates.alive());
        assert!(!gates.activate(1));
        assert!(!gates.program());
    }
}

#[test]
fn competing_activation_never_resurrects_cancellation() {
    for _ in 0..32 {
        let gates = Arc::new(Gates::default());
        gates.ready();
        gates.started();
        let barrier = Arc::new(Barrier::new(2));
        let other = gates.clone();
        let start = barrier.clone();
        let worker = std::thread::spawn(move || {
            start.wait();
            other.activate(1)
        });
        barrier.wait();
        gates.cancel();
        let _ = worker.join().unwrap();
        assert!(!gates.program());
        assert!(!gates.activate(1));
    }
}

#[test]
fn bounded_reservations_preserve_pending_owner_and_unproven_tombstones() {
    let mut registry = Reservations::new();
    let first = registry.reserve("exact-output-uid").unwrap();
    assert!(registry.reserve("exact-output-uid").is_none());
    // No release while native Stop/COM Release is blocked, or after failed Stop.
    for index in 1..MAX_RESERVATIONS {
        assert!(registry.reserve(&format!("other-{index}")).is_some());
    }
    assert!(registry.reserve("overflow").is_none());
    assert!(registry.reserve("exact-output-uid").is_none());
    registry.release(first); // Only after proven native destruction.
    assert_eq!(registry.reserve("exact-output-uid"), Some(first));
}

#[test]
fn raw_status_first_failure_and_unreturned_operation_are_distinct() {
    let timing = WasapiOutputTiming::new();
    timing.entered(WasapiOutputOperation::Start);
    let pending = timing.snapshot();
    assert_eq!(
        pending.last_call,
        Some(WasapiOutputCall {
            operation: WasapiOutputOperation::Start,
            status: None
        })
    );
    assert_eq!(pending.start_status, None);
    assert_eq!(pending.completion, WasapiOutputCompletion::Pending);
    timing.returned(WasapiOutputOperation::Start, 0);
    timing.returned(WasapiOutputOperation::Release, -200);
    timing.returned(WasapiOutputOperation::Stop, 1); // S_FALSE is retained.
    assert_eq!(timing.snapshot().stop_status, Some(1));
    assert_eq!(
        timing.failure(),
        Some(WasapiOutputCall {
            operation: WasapiOutputOperation::Release,
            status: Some(-200)
        })
    );
    timing.returned(WasapiOutputOperation::NativeRelease, -201);
    assert_eq!(timing.failure().unwrap().status, Some(-200));
    timing.released(false);
    assert_eq!(
        timing.snapshot().completion,
        WasapiOutputCompletion::ReleasedUnproven
    );
}

#[test]
fn submission_acknowledges_release_success_not_preparation_or_prefill() {
    let timing = WasapiOutputTiming::new();
    timing.configure(9, 10_000_000, 1024);
    timing.submitted(1024, None, true, 0);
    assert_eq!(timing.progress_count(), 0);
    assert_eq!(timing.snapshot().prefill_frames, 1024);
    timing.submitted(256, Some(7), false, -1);
    assert_eq!(timing.progress_count(), 0);
    assert_eq!(timing.snapshot().submitted_version, None);
    timing.submitted(512, None, false, 0);
    assert_eq!(timing.progress_count(), 1);
    assert_eq!(timing.snapshot().submitted_version, None);
    timing.submitted(256, Some(7), false, 0);
    timing.submitted(128, Some(8), false, 0);
    let snapshot = timing.snapshot();
    assert_eq!(snapshot.progress_count, 3);
    assert_eq!(snapshot.submitted_silent_frames, 512);
    assert_eq!(snapshot.submitted_pcm_frames, 384);
    assert_eq!(snapshot.submitted_version, Some(8));
    assert_eq!(
        (
            snapshot.epoch,
            snapshot.qpc_frequency,
            snapshot.maximum_frames
        ),
        (9, 10_000_000, 1024)
    );
}

#[test]
fn work_pending_and_immutable_lifecycle_slots_preserve_boundaries() {
    let timing = WasapiOutputTiming::new();
    timing.event(WasapiOutputPhase::StopRequested, 100, None);
    assert_eq!(
        timing.snapshot().lifecycle[WasapiOutputPhase::StopReturned as usize],
        None
    );
    timing.begin_work(120);
    assert_eq!(timing.snapshot().last_work, None);
    assert!(timing.snapshot().work_pending_or_contended);
    timing.finish_work(150, 256, false, Some(-5));
    assert_eq!(
        timing.snapshot().last_work,
        Some(WasapiOutputWork {
            serial: 1,
            entry_qpc_ticks: 120,
            exit_qpc_ticks: 150,
            frames: 256,
            silent: Some(false),
            release_status: Some(-5)
        })
    );
    timing.event(WasapiOutputPhase::StopReturned, 160, Some(-99));
    timing.event(WasapiOutputPhase::StopReturned, 170, Some(0));
    assert_eq!(
        timing.snapshot().lifecycle[WasapiOutputPhase::StopReturned as usize]
            .unwrap()
            .status,
        Some(-99)
    );
    timing.released(true);
    assert_eq!(
        timing.snapshot().completion,
        WasapiOutputCompletion::Completed
    );
}

#[test]
fn concurrent_work_snapshots_are_coherent_or_explicitly_unavailable() {
    let timing = Arc::new(WasapiOutputTiming::new());
    let writer = timing.clone();
    let thread = std::thread::spawn(move || {
        for index in 1..=20_000 {
            writer.begin_work(index * 10);
            writer.finish_work(
                index * 10 + 7,
                (index % 100) as u32,
                index % 2 == 0,
                Some(0),
            );
        }
    });
    while !thread.is_finished() {
        let snapshot = timing.snapshot();
        if let Some(work) = snapshot.last_work {
            assert_eq!(work.entry_qpc_ticks, work.serial * 10);
            assert_eq!(work.exit_qpc_ticks, work.entry_qpc_ticks + 7);
            assert_eq!(work.frames, (work.serial % 100) as u32);
            assert_eq!(work.silent, Some(work.serial % 2 == 0));
        }
    }
    thread.join().unwrap();
    assert_eq!(timing.snapshot().last_work.unwrap().serial, 20_000);
}

#[test]
fn gates_submission_and_snapshot_path_do_not_allocate_or_free() {
    use crate::tests::{COUNTING, OPERATIONS};
    let timing = WasapiOutputTiming::new();
    let gates = Gates::default();
    OPERATIONS.with(|count| count.set(0));
    COUNTING.with(|count| count.set(true));
    gates.ready();
    gates.request_start();
    gates.started();
    for index in 1..=100 {
        timing.begin_work(index * 10);
        timing.entered(WasapiOutputOperation::Release);
        timing.returned(WasapiOutputOperation::Release, 0);
        timing.finish_work(index * 10 + 9, 512, false, Some(0));
        timing.submitted(512, Some(index), false, 0);
        gates.activate(timing.progress_count());
        std::hint::black_box(timing.snapshot());
    }
    gates.cancel();
    COUNTING.with(|count| count.set(false));
    assert_eq!(OPERATIONS.with(|count| count.get()), 0);
}

#[test]
fn real_portable_lease_acks_only_after_release_and_preserves_unwind_failure() {
    use crate::wasapi_packet::{Encoding, Layout, SILENT};
    use crate::wasapi_render_packet::RenderLease;
    use std::cell::Cell;
    let timing = WasapiOutputTiming::new();
    let mut bytes = [0; 16];
    for (authorized, release_status) in [(true, 0), (false, 0), (true, -9)] {
        let record = Cell::new(None);
        let before = timing.progress_count();
        let mut lease = RenderLease::new(
            2,
            Layout {
                channels: 2,
                encoding: Encoding::F32,
            },
            Ok(&mut bytes),
            &record,
            |frames, flags| {
                assert_eq!(
                    timing.progress_count(),
                    before,
                    "preparation cannot acknowledge submission"
                );
                assert_eq!(frames, 2);
                assert_eq!(flags, if authorized { 0 } else { SILENT });
                timing.returned(WasapiOutputOperation::Release, release_status);
                if release_status < 0 {
                    Err(release_status)
                } else {
                    Ok(())
                }
            },
        )
        .unwrap();
        lease.write_stereo(0, &[0.25; 2], &[-0.25; 2]).unwrap();
        let result = lease.commit(authorized);
        let released = record.get().unwrap();
        if released.result.is_ok() {
            timing.submitted(
                released.frames,
                (result.is_ok() && released.flags == 0).then_some(6),
                false,
                release_status,
            );
        }
        assert_eq!(
            timing.progress_count(),
            before + u64::from(release_status >= 0)
        );
    }
    assert_eq!(timing.snapshot().submitted_version, Some(6));
    assert_eq!(timing.snapshot().submitted_pcm_frames, 2);
    assert_eq!(timing.snapshot().submitted_silent_frames, 2);
    assert!(
        !timing.cleanup_proven(),
        "failed release retains reservation even if unwinding skips packet bookkeeping"
    );
}

#[test]
fn early_open_handle_cleanup_failure_prevents_completed_reservation_release() {
    let timing = WasapiOutputTiming::new();
    assert!(timing.cleanup_proven());
    timing.cleanup_error(-15);
    timing.returned(WasapiOutputOperation::Open, -16);
    assert!(!timing.cleanup_proven());
    assert_eq!(
        timing.failure().unwrap().operation,
        WasapiOutputOperation::NativeRelease
    );
    assert_eq!(timing.failure().unwrap().status, Some(-15));
    timing.released(timing.cleanup_proven());
    assert_eq!(
        timing.snapshot().completion,
        WasapiOutputCompletion::ReleasedUnproven
    );
}

#[test]
fn acquisition_failure_does_not_fabricate_a_release_hresult_or_submission() {
    let timing = WasapiOutputTiming::new();
    timing.begin_work(5);
    timing.entered(WasapiOutputOperation::Acquire);
    timing.returned(WasapiOutputOperation::Acquire, -123);
    timing.finish_work(8, 512, true, None);
    let snapshot = timing.snapshot();
    assert_eq!(snapshot.last_work.unwrap().release_status, None);
    assert_eq!(snapshot.last_work.unwrap().silent, None);
    assert_eq!(snapshot.release_status, None);
    assert_eq!(
        snapshot.first_failure.unwrap().operation,
        WasapiOutputOperation::Acquire
    );
    assert_eq!(snapshot.first_failure.unwrap().status, Some(-123));
    assert_eq!(snapshot.progress_count, 0);
    assert!(timing.cleanup_proven(), "no acquired lease needs releasing");
}
