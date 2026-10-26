use super::*;
use std::sync::{Arc, Barrier};

fn metadata(index: u32) -> FaultMetadata {
    FaultMetadata {
        epoch: 100 + u64::from(index),
        observed_mach_host_time: Some(u64::MAX - u64::from(index)),
        native_object: Some(index),
        property: Some(FaultProperty {
            selector: index + 1,
            scope: index + 2,
            element: index + 3,
        }),
        status: Some(-(index as i32) - 1),
        packet_index: Some(900 + u64::from(index)),
    }
}

#[test]
fn suspended_winner_stops_audio_with_coupled_identity_and_pending_metadata() {
    let latch = FaultLatch::default();
    assert!(latch.claim(
        AudioFault::Xrun,
        FaultOrigin::NativeInputReentry,
        FaultSource::input(17)
    ));
    assert_eq!(latch.load(Ordering::Acquire), 5);
    let first = latch.snapshot().unwrap();
    assert_eq!(first.fault, AudioFault::Xrun);
    assert_eq!(first.origin, FaultOrigin::NativeInputReentry);
    assert_eq!(first.direction, FaultDirection::Input);
    assert_eq!(first.endpoint_index, Some(17));
    assert_eq!(first.metadata, None);
    latch.record(
        AudioFault::DeviceUnavailable,
        FaultOrigin::CpalOutputError,
        FaultSource::OUTPUT,
        metadata(3),
    );
    assert_eq!(latch.snapshot().unwrap(), first);
    latch.publish(metadata(17));
    assert_eq!(latch.snapshot().unwrap().metadata, Some(metadata(17)));
}

#[test]
fn concurrent_first_winner_keeps_its_code_origin_endpoint_and_raw_metadata() {
    for _ in 0..16 {
        let latch = Arc::new(FaultLatch::default());
        let barrier = Arc::new(Barrier::new(9));
        let threads: Vec<_> = (0_u32..8)
            .map(|index| {
                let latch = latch.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let (fault, origin) = if index.is_multiple_of(2) {
                        (AudioFault::Xrun, FaultOrigin::NativeInputOverload)
                    } else {
                        (
                            AudioFault::CaptureOverflow,
                            FaultOrigin::CaptureTransportProducer,
                        )
                    };
                    latch.record(
                        fault,
                        origin,
                        FaultSource::input(index as usize),
                        metadata(index),
                    );
                })
            })
            .collect();
        barrier.wait();
        // Snapshot may see no fault or pending metadata. A visible identity
        // must already agree internally, even while all writers contend.
        if let Some(record) = latch.snapshot() {
            check_winner(&record);
        }
        for thread in threads {
            thread.join().unwrap();
        }
        let record = latch.snapshot().unwrap();
        check_winner(&record);
        assert_eq!(
            record.metadata,
            Some(metadata(record.endpoint_index.unwrap()))
        );
    }
}
fn check_winner(record: &FaultAttribution) {
    let index = record.endpoint_index.unwrap();
    assert_eq!(record.direction, FaultDirection::Input);
    let (fault, origin) = if index.is_multiple_of(2) {
        (AudioFault::Xrun, FaultOrigin::NativeInputOverload)
    } else {
        (
            AudioFault::CaptureOverflow,
            FaultOrigin::CaptureTransportProducer,
        )
    };
    assert_eq!(record.fault, fault);
    assert_eq!(record.origin, origin);
    if let Some(observed) = record.metadata {
        assert_eq!(observed, metadata(index));
    }
}

#[test]
fn fault_publication_and_bounded_snapshot_allocate_nothing_and_keep_absent_fields_absent() {
    let latch = FaultLatch::default();
    crate::tests::OPERATIONS.with(|n| n.set(0));
    crate::tests::COUNTING.with(|on| on.set(true));
    latch.record(
        AudioFault::Xrun,
        FaultOrigin::CpalCoreAudioOutputOverload,
        FaultSource::OUTPUT,
        FaultMetadata::now(23),
    );
    let observed = latch.snapshot().unwrap();
    latch.record(
        AudioFault::Backend,
        FaultOrigin::Runtime,
        FaultSource::RUNTIME,
        metadata(1),
    );
    crate::tests::COUNTING.with(|on| on.set(false));
    assert_eq!(crate::tests::OPERATIONS.with(|n| n.get()), 0);
    let details = observed.metadata.unwrap();
    assert_eq!(details.epoch, 23);
    assert_eq!(details.native_object, None);
    assert_eq!(details.property, None);
    assert_eq!(details.status, None);
    assert_eq!(details.packet_index, None);
    assert_eq!(observed.endpoint_index, Some(0));
    assert_eq!(observed.direction, FaultDirection::Output);
    assert_eq!(latch.snapshot().unwrap(), observed);
}

#[test]
fn no_fault_cannot_claim_or_publish_a_record() {
    let latch = FaultLatch::default();
    latch.record(
        AudioFault::None,
        FaultOrigin::Runtime,
        FaultSource::RUNTIME,
        metadata(1),
    );
    assert_eq!(latch.load(Ordering::Acquire), 0);
    assert_eq!(latch.snapshot(), None);
}
