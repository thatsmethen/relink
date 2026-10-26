use super::*;
use crate::tests::{COUNTING, OPERATIONS};
use std::sync::Arc;

fn p(index: u64, sample: f64, frames: u32) -> NativeOutputPacket {
    NativeOutputPacket {
        callback_index: index,
        epoch: 4,
        frames,
        entry_host_time: index * 100 + 1,
        now: NativeOutputStamp {
            sample_time: sample - 12.0,
            host_time: index * 100,
            rate_scalar: 1.0,
            flags: 7,
        },
        presentation: NativeOutputStamp {
            sample_time: sample,
            host_time: index * 100 + 30,
            rate_scalar: 1.0,
            flags: 7,
        },
    }
}

#[test]
fn output_continuity_uses_presentation_frames_without_assuming_host_clock_rate() {
    let mut tracker = Tracker::default();
    let mut at = -512.0;
    for index in 1..1000 {
        let mut value = p(index, at, if index % 3 == 0 { 128 } else { 512 });
        value.presentation.host_time = index * 93 + index % 7;
        value.now.sample_time += (index % 11) as f64;
        tracker.check(value).unwrap();
        tracker.commit(value);
        at += f64::from(value.frames);
    }
    let previous = tracker.previous.unwrap();
    let mut bad = p(1000, at + 1.0, 512);
    bad.presentation.host_time = previous.presentation.host_time + 1;
    assert_eq!(
        tracker.check(bad),
        Err((
            AudioFault::Xrun,
            NativeOutputFaultReason::SampleDiscontinuity
        ))
    );
    assert_eq!(tracker.previous, Some(previous));
    bad.presentation.sample_time = at;
    bad.presentation.host_time = previous.presentation.host_time;
    assert_eq!(
        tracker.check(bad),
        Err((AudioFault::Xrun, NativeOutputFaultReason::HostDiscontinuity))
    );
    bad.presentation.host_time += 1;
    bad.epoch += 1;
    assert_eq!(
        tracker.check(bad),
        Err((
            AudioFault::StreamInvalidated,
            NativeOutputFaultReason::EpochChanged
        ))
    );
}

#[test]
fn malformed_output_clock_flags_extents_and_precision_are_not_capture_faults() {
    for case in 0..9 {
        let mut value = p(1, 0.0, 512);
        match case {
            0 => value.now.flags = 2,
            1 => value.presentation.flags = 1,
            2 => value.presentation.sample_time = f64::NAN,
            3 => value.presentation.sample_time = 2.0_f64.powi(53),
            4 => value.presentation.host_time = 0,
            5 => value.now.rate_scalar = 0.0,
            6 => value.presentation.rate_scalar = f64::INFINITY,
            7 => value.frames = 0,
            _ => value.frames = MAX_CALLBACK_FRAMES as u32 + 1,
        }
        let (fault, _) = Tracker::default().check(value).unwrap_err();
        assert!(matches!(
            fault,
            AudioFault::StreamInvalidated | AudioFault::InvalidBuffer
        ));
    }
}

#[test]
fn first_output_fault_pair_and_lifecycle_status_remain_after_later_work() {
    let timing = NativeOutputTiming::new();
    let first = NativeOutputFault {
        fault: AudioFault::Xrun,
        reason: NativeOutputFaultReason::SampleDiscontinuity,
        previous: Some(p(1, 0.0, 512)),
        current: p(2, 513.0, 512),
    };
    timing.reject(first);
    timing.reject(NativeOutputFault {
        fault: AudioFault::Backend,
        reason: NativeOutputFaultReason::Reentry,
        previous: None,
        current: p(3, 900.0, 1),
    });
    timing.accept(p(3, 1024.0, 512));
    timing.lifecycle(NativeOutputLifecycleEvent {
        phase: NativeOutputLifecyclePhase::StopReturned,
        host_time: 99,
        status: Some(-50),
        callbacks_in_flight: Some(1),
    });
    timing.lifecycle(NativeOutputLifecycleEvent {
        phase: NativeOutputLifecyclePhase::StopReturned,
        host_time: 100,
        status: Some(0),
        callbacks_in_flight: Some(0),
    });
    let snapshot = timing.snapshot();
    assert_eq!(snapshot.first_fault, Some(first));
    assert!(!snapshot.first_fault_pending);
    assert_eq!(
        snapshot.lifecycle[NativeOutputLifecyclePhase::StopReturned as usize]
            .unwrap()
            .status,
        Some(-50)
    );
    assert!(snapshot.lifecycle[NativeOutputLifecyclePhase::OwnerCompleted as usize].is_none());
    // A winner suspended before data publication never exposes a torn pair.
    let pending = NativeOutputTiming::new();
    pending.fault.identity.store(
        AudioFault::Xrun as u32 | (NativeOutputFaultReason::SampleDiscontinuity as u32) << 8,
        Ordering::Release,
    );
    assert!(pending.snapshot().first_fault_pending);
    assert!(pending.snapshot().first_fault.is_none());
}

#[test]
fn concurrent_output_reads_are_bounded_coherent_and_preserve_raw_bits() {
    let timing = Arc::new(NativeOutputTiming::new());
    let writer = timing.clone();
    let thread = std::thread::spawn(move || {
        for index in 1..20_000 {
            let mut value = p(index, index as f64 * 512.0, 512);
            value.now.rate_scalar = f64::from_bits(0x7ff8000000000066);
            writer.accept(value);
            writer.complete(
                NativeOutputCompletion {
                    packet: value,
                    after_render_host_time: index + 17,
                    completion_host_time: index + 33,
                    outcome: NativeOutputOutcome::Encoded {
                        accepted_version: Some(index + 1000),
                    },
                },
                true,
            );
        }
    });
    while !thread.is_finished() {
        let snapshot = timing.snapshot();
        if let Some(value) = snapshot.last_packet {
            assert_eq!(
                value.presentation.sample_time,
                value.callback_index as f64 * 512.0
            );
            assert_eq!(value.now.rate_scalar.to_bits(), 0x7ff8000000000066);
        }
        if let Some(value) = snapshot.last_completion {
            assert_eq!(
                value.after_render_host_time,
                value.packet.callback_index + 17
            );
            assert_eq!(value.completion_host_time, value.packet.callback_index + 33);
            assert_eq!(
                value.outcome,
                NativeOutputOutcome::Encoded {
                    accepted_version: Some(value.packet.callback_index + 1000)
                }
            );
        }
    }
    thread.join().unwrap();
    assert_eq!(timing.progress_count(), 19_999);
}

#[test]
fn output_journal_first_fault_completion_and_snapshots_allocate_nothing() {
    let timing = NativeOutputTiming::new();
    let mut tracker = Tracker::default();
    OPERATIONS.with(|n| n.set(0));
    COUNTING.with(|on| on.set(true));
    let value = p(timing.enter(), 0.0, 512);
    tracker.check(value).unwrap();
    tracker.commit(value);
    timing.accept(value);
    timing.complete(
        NativeOutputCompletion {
            packet: value,
            after_render_host_time: 20,
            completion_host_time: 30,
            outcome: NativeOutputOutcome::InactiveSilence,
        },
        true,
    );
    timing.reject(NativeOutputFault {
        fault: AudioFault::InvalidBuffer,
        reason: NativeOutputFaultReason::InvalidBuffer,
        previous: Some(value),
        current: p(2, 512.0, 0),
    });
    timing.lifecycle(NativeOutputLifecycleEvent {
        phase: NativeOutputLifecyclePhase::HandleCancelled,
        host_time: 40,
        status: None,
        callbacks_in_flight: Some(1),
    });
    let snapshot = timing.snapshot();
    COUNTING.with(|on| on.set(false));
    assert_eq!(OPERATIONS.with(|n| n.get()), 0);
    assert_eq!(snapshot.progress_count, 1);
    assert_eq!(snapshot.callbacks_seen, 1);
    assert_eq!(
        snapshot.last_completion.unwrap().outcome,
        NativeOutputOutcome::InactiveSilence
    );
}
