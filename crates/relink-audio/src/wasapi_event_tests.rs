use super::*;
use crate::{AudioFault, FaultMetadata, FaultOrigin, FaultSource, Telemetry};
use relink_core::Session;
use std::sync::{atomic::AtomicUsize, Mutex};

struct Tracked(&'static str, Arc<Mutex<Vec<&'static str>>>);
impl Drop for Tracked {
    fn drop(&mut self) {
        self.1.lock().unwrap().push(self.0);
    }
}
struct Handle {
    log: Arc<Mutex<Vec<&'static str>>>,
    attempts: Arc<AtomicUsize>,
    status: Result<(), i32>,
}
fn close(handle: Handle) -> Result<(), i32> {
    handle.attempts.fetch_add(1, Ordering::Relaxed);
    handle.log.lock().unwrap().push("event");
    handle.status
}
struct Fixture {
    record: Arc<CloseRecord>,
    log: Arc<Mutex<Vec<&'static str>>>,
    attempts: Arc<AtomicUsize>,
}
impl Fixture {
    fn new() -> Self {
        Self {
            record: Arc::new(CloseRecord::default()),
            log: Arc::default(),
            attempts: Arc::default(),
        }
    }
    fn tracked(&self, name: &'static str) -> Tracked {
        Tracked(name, self.log.clone())
    }
    fn event(&self, status: Result<(), i32>) -> OwnedEvent<Handle> {
        OwnedEvent::new(
            Handle {
                log: self.log.clone(),
                attempts: self.attempts.clone(),
                status,
            },
            close,
            self.record.clone(),
        )
    }
    fn native(
        &self,
        status: Result<(), i32>,
    ) -> NativeResources<Tracked, Tracked, OwnedEvent<Handle>, Tracked, Tracked> {
        NativeResources {
            capture: self.tracked("capture"),
            client: self.tracked("client"),
            event: self.event(status),
            realtime: self.tracked("realtime"),
            apartment: self.tracked("apartment"),
        }
    }
}
#[test]
fn successful_stop_releases_com_before_one_event_close_and_apartment_last() {
    let f = Fixture::new();
    assert_eq!(f.record.snapshot(), WasapiEventClose::NotCreated);
    let mut native = Some(f.native(Ok(())));
    assert_eq!(f.record.snapshot(), WasapiEventClose::Pending);
    assert_eq!(
        native
            .as_ref()
            .unwrap()
            .event
            .handle()
            .attempts
            .load(Ordering::Relaxed),
        0
    );
    let guard = CleanupReservation::new(
        f.tracked("identity"),
        Arc::new(f.tracked("reservation")),
        f.record.clone(),
    );
    release_native(&mut native, &f.record, |_| {
        f.log.lock().unwrap().push("stop");
        Ok(())
    })
    .unwrap();
    assert_eq!(f.record.snapshot(), WasapiEventClose::Succeeded);
    drop(guard);
    assert_eq!(
        *f.log.lock().unwrap(),
        [
            "stop",
            "capture",
            "client",
            "event",
            "realtime",
            "apartment",
            "identity",
            "reservation"
        ]
    );
    assert_eq!(f.attempts.load(Ordering::Relaxed), 1);
}
#[test]
fn failed_close_retains_identity_and_reservation_without_retry_even_after_prior_fault() {
    let f = Fixture::new();
    let telemetry = Telemetry::new(&Session::new("close controls", 48000));
    telemetry.record_fault(
        AudioFault::Xrun,
        FaultOrigin::WasapiInput,
        FaultSource::input(0),
        FaultMetadata::now(1),
    );
    let mut native = Some(f.native(Err(-123)));
    let guard = CleanupReservation::new(
        f.tracked("identity"),
        Arc::new(f.tracked("reservation")),
        f.record.clone(),
    );
    assert_eq!(
        release_native(&mut native, &f.record, |_| Ok(())),
        Err(-123)
    );
    telemetry.record_fault(
        AudioFault::Backend,
        FaultOrigin::WasapiInput,
        FaultSource::input(0),
        FaultMetadata {
            status: Some(-123),
            ..FaultMetadata::now(1)
        },
    );
    drop(guard);
    assert_eq!(telemetry.fault.snapshot().unwrap().fault, AudioFault::Xrun);
    assert_eq!(
        f.record.snapshot(),
        WasapiEventClose::Failed { hresult: -123 }
    );
    assert_eq!(
        *f.log.lock().unwrap(),
        ["capture", "client", "event", "realtime", "apartment"]
    );
    assert_eq!(
        release_native(&mut native, &f.record, |_| panic!("no retry")),
        Err(-123)
    );
    assert_eq!(f.attempts.load(Ordering::Relaxed), 1);
}
#[test]
fn stop_failure_quarantines_native_event_and_selection_without_closing() {
    let f = Fixture::new();
    let mut native = Some(f.native(Ok(())));
    let mut guard = CleanupReservation::new(
        f.tracked("identity"),
        Arc::new(f.tracked("reservation")),
        f.record.clone(),
    );
    assert_eq!(
        release_native(&mut native, &f.record, |_| Err(-456)),
        Err(-456)
    );
    guard.retain();
    drop(guard);
    assert_eq!(f.record.snapshot(), WasapiEventClose::Pending);
    assert!(f.log.lock().unwrap().is_empty());
    assert_eq!(f.attempts.load(Ordering::Relaxed), 0);
}
#[test]
fn partial_constructor_unwind_closes_once_after_client_and_retains_on_failure() {
    for stage in 0..3 {
        for status in [Ok(()), Err(-78)] {
            let f = Fixture::new();
            let guard = CleanupReservation::new(
                f.tracked("identity"),
                Arc::new(f.tracked("reservation")),
                f.record.clone(),
            );
            let result: Result<(_, ()), i32> = prepare_resources(
                f.tracked("client"),
                f.tracked("apartment"),
                || Ok(f.event(status)),
                |resources| {
                    if stage >= 1 {
                        resources.capture = Some(f.tracked("capture"));
                    }
                    if stage >= 2 {
                        resources.realtime = Some(f.tracked("realtime"));
                    }
                    Err(-999)
                },
            );
            assert!(result.is_err());
            drop(guard);
            let mut expected = Vec::new();
            if stage >= 1 {
                expected.push("capture");
            }
            expected.extend(["client", "event"]);
            if stage >= 2 {
                expected.push("realtime");
            }
            expected.push("apartment");
            if status.is_ok() {
                expected.extend(["identity", "reservation"]);
            }
            assert_eq!(*f.log.lock().unwrap(), expected);
            assert_eq!(f.attempts.load(Ordering::Relaxed), 1);
        }
    }
}
#[test]
fn cancelled_before_event_creation_keeps_no_graph_and_releases_reservation() {
    let f = Fixture::new();
    let ring = crate::capture_transport::Ring::new();
    let weak_ring = Arc::downgrade(&ring);
    let close = ring.event_close.clone();
    let guard = CleanupReservation::new(
        f.tracked("identity"),
        Arc::new(f.tracked("reservation")),
        close.clone(),
    );
    drop(ring);
    assert!(
        weak_ring.upgrade().is_none(),
        "pending activation retains diagnostics only"
    );
    assert_eq!(close.snapshot(), WasapiEventClose::NotCreated);
    drop(guard);
    assert_eq!(*f.log.lock().unwrap(), ["identity", "reservation"]);
}
#[test]
fn late_cancel_after_creation_before_configure_observes_close_and_quarantine() {
    for status in [Ok(()), Err(-91)] {
        let f = Fixture::new();
        let guard = CleanupReservation::new(
            f.tracked("identity"),
            Arc::new(f.tracked("reservation")),
            f.record.clone(),
        );
        let mut native = Some(f.native(status)); // No CaptureState or graph has been sent.
        assert_eq!(release_native(&mut native, &f.record, |_| Ok(())), status);
        drop(guard);
        assert_eq!(f.attempts.load(Ordering::Relaxed), 1);
        let log = f.log.lock().unwrap();
        assert_eq!(log.contains(&"reservation"), status.is_ok());
        assert_eq!(log.contains(&"identity"), status.is_ok());
    }
}
#[test]
fn event_creation_failure_releases_client_before_apartment_without_close_attempt() {
    let f = Fixture::new();
    type Resources =
        NativeResources<Option<Tracked>, Tracked, OwnedEvent<Handle>, Option<Tracked>, Tracked>;
    let result: Result<(Resources, ()), i32> = prepare_resources(
        f.tracked("client"),
        f.tracked("apartment"),
        || Err(-8),
        |_| panic!("no event"),
    );
    assert!(result.is_err());
    assert_eq!(*f.log.lock().unwrap(), ["client", "apartment"]);
    assert_eq!(f.record.snapshot(), WasapiEventClose::NotCreated);
    assert_eq!(f.attempts.load(Ordering::Relaxed), 0);
}

#[test]
fn retained_runtime_snapshot_reports_close_failure_separately_from_first_xrun() {
    let ring = crate::capture_transport::Ring::new();
    let f = Fixture::new();
    let mut telemetry = Telemetry::new(&Session::new("retained close", 48000));
    telemetry
        .captures
        .push((crate::InputId("application:selected".into()), ring.clone()));
    telemetry.record_fault(
        AudioFault::Xrun,
        FaultOrigin::WasapiInput,
        FaultSource::input(0),
        FaultMetadata::now(1),
    );
    let runtime = crate::AudioRuntime::new().unwrap(); // Stopped; no discovery or capture.
    runtime.shared.state().telemetry = Some(Arc::new(telemetry));
    assert_eq!(
        runtime.snapshot().captures[0].event_close,
        Some(WasapiEventClose::NotCreated)
    );
    let event = OwnedEvent::new(
        Handle {
            log: f.log.clone(),
            attempts: f.attempts.clone(),
            status: Err(-701),
        },
        close,
        ring.event_close.clone(),
    );
    assert_eq!(
        runtime.snapshot().captures[0].event_close,
        Some(WasapiEventClose::Pending)
    );
    drop(event);
    let snapshot = runtime.snapshot();
    assert_eq!(
        snapshot.captures[0].event_close,
        Some(WasapiEventClose::Failed { hresult: -701 })
    );
    assert_eq!(snapshot.first_fault.unwrap().fault, AudioFault::Xrun);
    assert!(!snapshot.running);
    assert_eq!(f.attempts.load(Ordering::Relaxed), 1);
}

#[test]
fn reservations_reject_overlap_and_retained_owner_budget() {
    let mut registry = Registry::new();
    let process = ProcessInstance { pid: 1, started: 2 };
    let record = || Arc::new(CloseRecord::default());
    registry
        .acquire("a".into(), &[process], 100, record())
        .unwrap();
    let overlap = registry
        .acquire("b".into(), &[process], 100, record())
        .unwrap_err();
    assert!(overlap.to_string().contains("NotCreated"));
    assert!(registry
        .acquire(
            "c".into(),
            &[ProcessInstance { pid: 3, started: 4 }],
            MAX_RESERVED_BYTES,
            record(),
        )
        .is_err());
    assert_eq!(registry.bytes, 100);
    registry.remove("a");
    assert_eq!(registry.bytes, 0);
    registry
        .acquire("b".into(), &[process], 100, record())
        .unwrap();
}

// Models the native Reservation's exact registry-removal responsibility without
// invoking Windows process discovery, COM, activation or audio capture.
struct TestReservation {
    registry: Arc<Mutex<Registry>>,
    key: String,
}
impl Drop for TestReservation {
    fn drop(&mut self) {
        self.registry.lock().unwrap().remove(&self.key);
    }
}

#[test]
fn late_close_after_telemetry_replacement_remains_queryable_without_old_ring() {
    for status in [Ok(()), Err(0x8007_0006u32 as i32)] {
        let f = Fixture::new();
        let registry = Arc::new(Mutex::new(Registry::new()));
        let process = ProcessInstance {
            pid: 91,
            started: 701,
        };
        let old_ring = crate::capture_transport::Ring::new();
        let weak_ring = Arc::downgrade(&old_ring);
        let close_record = old_ring.event_close.clone();
        let weak_record = Arc::downgrade(&close_record);
        registry
            .lock()
            .unwrap()
            .acquire("cancelled-A".into(), &[process], 100, close_record.clone())
            .unwrap();
        let cleanup = CleanupReservation::new(
            f.tracked("identity"),
            Arc::new(TestReservation {
                registry: registry.clone(),
                key: "cancelled-A".into(),
            }),
            close_record.clone(),
        );
        let event = OwnedEvent::new(
            Handle {
                log: f.log.clone(),
                attempts: f.attempts.clone(),
                status,
            },
            close,
            close_record,
        );
        let mut old_telemetry = Telemetry::new(&Session::new("cancelled A", 48000));
        old_telemetry
            .captures
            .push((crate::InputId("application:A".into()), old_ring));
        old_telemetry.record_fault(
            AudioFault::Xrun,
            FaultOrigin::WasapiInput,
            FaultSource::input(0),
            FaultMetadata::now(1),
        );
        let runtime = crate::AudioRuntime::new().unwrap(); // Stopped; no native IO.
        runtime.shared.state().telemetry = Some(Arc::new(old_telemetry));
        assert_eq!(
            runtime.snapshot().captures[0].event_close,
            Some(WasapiEventClose::Pending)
        );

        // A timed out; B replaces shared telemetry while A has not closed yet.
        let mut new_telemetry = Telemetry::new(&Session::new("replacement B", 48000));
        new_telemetry.captures.push((
            crate::InputId("application:B".into()),
            crate::capture_transport::Ring::new(),
        ));
        runtime.shared.state().telemetry = Some(Arc::new(new_telemetry));
        assert!(
            weak_ring.upgrade().is_none(),
            "the late owner cannot keep A's PCM ring"
        );
        drop(event); // The late constructor/cancel path finally returns and closes.
        drop(cleanup);
        assert_eq!(f.attempts.load(Ordering::Relaxed), 1);
        assert_eq!(
            runtime.snapshot().captures[0].event_close,
            Some(WasapiEventClose::NotCreated)
        );
        assert!(runtime.snapshot().first_fault.is_none());
        let retry = registry.lock().unwrap().acquire(
            "retry-A".into(),
            &[process],
            100,
            Arc::new(CloseRecord::default()),
        );
        if let Err(hresult) = status {
            let retained = weak_record
                .upgrade()
                .expect("registry retains late close evidence");
            assert_eq!(retained.snapshot(), WasapiEventClose::Failed { hresult });
            let error = retry.unwrap_err().to_string();
            assert!(error.contains("CloseHandle failed"));
            assert!(error.contains("0x80070006"));
            assert!(error.contains(&hresult.to_string()));
            assert_eq!(registry.lock().unwrap().bytes, 100);
            assert!(!f.log.lock().unwrap().contains(&"identity"));
        } else {
            assert!(
                weak_record.upgrade().is_none(),
                "successful cleanup frees its diagnostic record"
            );
            retry.unwrap();
            assert!(f.log.lock().unwrap().contains(&"identity"));
        }
        assert_eq!(
            f.attempts.load(Ordering::Relaxed),
            1,
            "retry never closes an old numeric handle"
        );
    }
}

#[test]
fn overlapping_failure_is_reported_even_when_another_owner_is_still_pending() {
    let mut registry = Registry::new();
    let a = ProcessInstance { pid: 1, started: 1 };
    let b = ProcessInstance { pid: 2, started: 2 };
    let pending = Fixture::new();
    let _pending_event = pending.event(Ok(()));
    registry
        .acquire("pending".into(), &[a], 100, pending.record.clone())
        .unwrap();
    let failed = Fixture::new();
    registry
        .acquire("failed".into(), &[b], 100, failed.record.clone())
        .unwrap();
    drop(failed.event(Err(-701)));
    let error = registry
        .acquire(
            "overlap-both".into(),
            &[a, b],
            100,
            Arc::new(CloseRecord::default()),
        )
        .unwrap_err();
    assert!(error.to_string().contains("CloseHandle failed"));
    assert!(error.to_string().contains("-701"));
    assert_eq!(registry.bytes, 200);
}
