use super::*;
use crate::wasapi_event::{prepare_resources, NativeResources, OwnedEvent};
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
    record: Arc<Record>,
    log: Arc<Mutex<Vec<&'static str>>>,
    attempts: Arc<AtomicUsize>,
}
impl Fixture {
    fn new() -> Self {
        Self {
            record: Arc::new(Record::new(Arc::new(CloseRecord::default()))),
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
            self.record.close.clone(),
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
fn stop_requirement_covers_failed_start_and_preserves_success_codes() {
    let mut requirement = StopRequirement::default();
    assert_eq!(
        requirement.stop(|| panic!("no Start was entered")),
        Ok(None)
    );
    // This transition occurs before native Start, so failure of that call does
    // not cancel the obligation. A failed Stop cannot clear it either.
    requirement.start_entered();
    assert!(requirement.required());
    assert_eq!(requirement.stop(|| Err(-77)), Err(-77));
    assert!(requirement.required());
    assert_eq!(requirement.stop(|| Ok(1)), Ok(Some(1))); // S_FALSE
    assert!(!requirement.required());
    assert_eq!(requirement.stop(|| panic!("no duplicate Stop")), Ok(None));
}

#[test]
fn success_releases_native_before_endpoint_and_allows_reacquisition() {
    for status in [0, 1] {
        let f = Fixture::new();
        let mut registry = Registry::new();
        let slot = registry.acquire("endpoint", 100, f.record.clone()).unwrap();
        let mut native = Some(f.native(Ok(())));
        let mut stops = 0;
        release(&mut native, &f.record, |_| {
            stops += 1;
            assert_eq!(f.record.stop(), StopState::Pending);
            registry.release(slot);
            assert_eq!(registry.bytes, 100);
            f.log.lock().unwrap().push("stop");
            Ok(Some(status))
        })
        .unwrap();
        assert_eq!(f.record.stop(), StopState::Returned { hresult: status });
        assert_eq!(f.record.close.snapshot(), WasapiEventClose::Succeeded);
        assert_eq!(
            *f.log.lock().unwrap(),
            [
                "stop",
                "capture",
                "client",
                "event",
                "realtime",
                "apartment"
            ]
        );
        registry.release(slot);
        assert_eq!(registry.bytes, 0);
        registry
            .acquire("endpoint", 100, Fixture::new().record)
            .unwrap();
        release(&mut native, &f.record, |_| panic!("no duplicate Stop")).unwrap();
        assert_eq!(stops, 1);
        assert_eq!(f.attempts.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn failed_stop_retains_native_and_endpoint_without_retry_or_false_success() {
    let f = Fixture::new();
    let mut registry = Registry::new();
    let slot = registry.acquire("endpoint", 123, f.record.clone()).unwrap();
    let mut native = Some(f.native(Ok(())));
    assert_eq!(release(&mut native, &f.record, |_| Err(-456)), Err(-456));
    assert_eq!(f.record.stop(), StopState::Returned { hresult: -456 });
    assert_eq!(f.record.close.snapshot(), WasapiEventClose::Pending);
    assert!(native.is_none());
    assert!(f.log.lock().unwrap().is_empty());
    assert_eq!(f.attempts.load(Ordering::Relaxed), 0);
    registry.release(slot);
    assert_eq!(registry.bytes, 123);
    let error = registry
        .acquire("endpoint", 1, Fixture::new().record)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Stop failed"));
    assert!(error.contains("0xFFFFFE38, -456"));
    assert_eq!(
        release(&mut native, &f.record, |_| panic!("no retry")),
        Err(-456)
    );
    assert!(f.log.lock().unwrap().is_empty());
}

#[test]
fn missing_stop_return_and_unclosed_event_never_reopen_admission() {
    let f = Fixture::new();
    let mut registry = Registry::new();
    let slot = registry.acquire("endpoint", 123, f.record.clone()).unwrap();
    let event = f.event(Ok(()));
    f.record.stop.store(1, Ordering::Release);
    registry.release(slot);
    assert_eq!(registry.bytes, 123);
    assert_eq!(
        release::<()>(&mut None, &f.record, |_| unreachable!()),
        Err(0x8000_000A_u32 as i32)
    );
    drop(event);
    registry.release(slot);
    assert_eq!(registry.bytes, 123); // event closure cannot prove Stop returned
    assert!(registry
        .acquire("endpoint", 1, Fixture::new().record)
        .is_err());
}

#[test]
fn close_failure_remains_separate_after_successful_stop() {
    let f = Fixture::new();
    let mut registry = Registry::new();
    let slot = registry.acquire("endpoint", 111, f.record.clone()).unwrap();
    let mut native = Some(f.native(Err(-78)));
    assert_eq!(release(&mut native, &f.record, |_| Ok(Some(0))), Err(-78));
    assert_eq!(f.record.stop(), StopState::Returned { hresult: 0 });
    assert_eq!(
        f.record.close.snapshot(),
        WasapiEventClose::Failed { hresult: -78 }
    );
    assert_eq!(
        *f.log.lock().unwrap(),
        ["capture", "client", "event", "realtime", "apartment"]
    );
    registry.release(slot);
    assert_eq!(registry.bytes, 111);
    let error = registry
        .acquire("endpoint", 1, Fixture::new().record)
        .unwrap_err()
        .to_string();
    assert!(error.contains("CloseHandle failed"));
    assert!(error.contains("0xFFFFFFB2, -78"));
    assert_eq!(
        release(&mut native, &f.record, |_| panic!("no retry")),
        Err(-78)
    );
    assert_eq!(f.attempts.load(Ordering::Relaxed), 1);
}

#[test]
fn constructor_unwind_retains_only_failed_close_and_never_fabricates_stop() {
    for close_result in [Ok(()), Err(-90)] {
        let f = Fixture::new();
        let mut registry = Registry::new();
        let slot = registry.acquire("endpoint", 123, f.record.clone()).unwrap();
        let result: Result<(_, ()), i32> = prepare_resources(
            f.tracked("client"),
            f.tracked("apartment"),
            || Ok(f.event(close_result)),
            |resources| {
                resources.capture = Some(f.tracked("capture"));
                resources.realtime = Some(f.tracked("realtime"));
                Err(-800)
            },
        );
        assert!(result.is_err());
        assert_eq!(f.record.stop(), StopState::NotRequested);
        assert_eq!(
            *f.log.lock().unwrap(),
            ["capture", "client", "event", "realtime", "apartment"]
        );
        registry.release(slot);
        assert_eq!(registry.bytes, if close_result.is_ok() { 0 } else { 123 });
        assert_eq!(f.attempts.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn cancellation_before_and_after_construction_without_start_releases_reservation() {
    for constructed in [false, true] {
        let f = Fixture::new();
        let mut registry = Registry::new();
        let slot = registry.acquire("endpoint", 123, f.record.clone()).unwrap();
        let mut native = constructed.then(|| f.native(Ok(())));
        let mut requirement = StopRequirement::default();
        release(&mut native, &f.record, |_| {
            requirement.stop(|| panic!("no Start"))
        })
        .unwrap();
        assert_eq!(f.record.stop(), StopState::NotRequested);
        registry.release(slot);
        assert_eq!(registry.bytes, 0);
        assert_eq!(f.attempts.load(Ordering::Relaxed), usize::from(constructed));
    }
}

#[test]
fn retained_diagnostics_outlive_graph_and_preserve_prior_fault() {
    use crate::{AudioFault, FaultMetadata, FaultOrigin, FaultSource, Telemetry};
    let telemetry = Arc::new(Telemetry::new(&relink_core::Session::new(
        "stop controls",
        48000,
    )));
    telemetry.record_fault(
        AudioFault::Xrun,
        FaultOrigin::WasapiInput,
        FaultSource::input(0),
        FaultMetadata::now(1),
    );
    let ring = crate::capture_transport::Ring::new();
    let weak_ring = Arc::downgrade(&ring);
    let weak_telemetry = Arc::downgrade(&telemetry);
    let record = Arc::new(Record::new(ring.event_close.clone()));
    let mut registry = Registry::new();
    let slot = registry.acquire("endpoint", 123, record.clone()).unwrap();
    drop(ring);
    assert!(weak_ring.upgrade().is_none());
    let mut native = Some(());
    assert_eq!(release(&mut native, &record, |_| Err(-99)), Err(-99));
    telemetry.record_fault(
        AudioFault::Backend,
        FaultOrigin::WasapiInput,
        FaultSource::input(0),
        FaultMetadata {
            status: Some(-99),
            ..FaultMetadata::now(1)
        },
    );
    assert_eq!(telemetry.fault.snapshot().unwrap().fault, AudioFault::Xrun);
    drop(telemetry);
    assert!(weak_telemetry.upgrade().is_none());
    drop(record);
    registry.release(slot);
    let error = registry
        .acquire("endpoint", 1, Fixture::new().record)
        .unwrap_err()
        .to_string();
    assert!(error.contains("0xFFFFFF9D, -99"));
}

#[test]
fn endpoint_identity_is_exact_and_live_ownership_blocks_duplicate() {
    let mut registry = Registry::new();
    let slot = registry
        .acquire("endpoint", 1, Fixture::new().record)
        .unwrap();
    assert!(registry
        .acquire("endpoint", 1, Fixture::new().record)
        .is_err());
    registry
        .acquire("different-endpoint", 1, Fixture::new().record)
        .unwrap();
    registry.release(slot);
    registry
        .acquire("endpoint", 1, Fixture::new().record)
        .unwrap();
}

#[test]
fn count_byte_and_overflow_boundaries_reject_without_partial_admission() {
    let mut registry = Registry::new();
    for index in 0..MAX_OWNERS {
        registry
            .acquire(&format!("endpoint-{index}"), 1, Fixture::new().record)
            .unwrap();
    }
    assert!(registry
        .acquire("over-capacity", 1, Fixture::new().record)
        .is_err());
    assert_eq!(registry.bytes, MAX_OWNERS);
    registry.release(0);
    registry
        .acquire("replacement", 1, Fixture::new().record)
        .unwrap();

    let mut registry = Registry::new();
    registry
        .acquire("endpoint", MAX_OWNER_BYTES, Fixture::new().record)
        .unwrap();
    assert!(registry
        .acquire("over-budget", 1, Fixture::new().record)
        .is_err());
    assert!(registry
        .acquire("overflow", usize::MAX, Fixture::new().record)
        .is_err());
    assert_eq!(registry.bytes, MAX_OWNER_BYTES);
    assert_eq!(registry.slots.iter().flatten().count(), 1);
    registry.release(0);
    assert_eq!(registry.bytes, 0);
}
