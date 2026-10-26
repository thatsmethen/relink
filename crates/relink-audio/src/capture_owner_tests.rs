use super::*;
use std::sync::mpsc;

#[test]
fn untouched_and_completed_tickets_cannot_be_reopened() {
    let ticket = Arc::new(CaptureCompletion::default());
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::NotStarted);
    let attempt = ticket.begin().unwrap();
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::Pending);
    assert!(ticket.begin().is_err());
    drop(attempt); // pure preflight rejected before any native work
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::Completed);
    assert!(ticket.begin().is_err());
    assert!(ticket.participant().is_err());
}

#[test]
fn blocked_owner_and_membership_worker_both_hold_completion() {
    let ticket = Arc::new(CaptureCompletion::default());
    let owner = ticket.begin().unwrap();
    let membership = ticket.participant().unwrap();
    let (owner_tx, owner_rx) = mpsc::sync_channel(0);
    let (monitor_tx, monitor_rx) = mpsc::sync_channel(0);
    let main = owner
        .spawn(Builder::new(), move || {
            owner_rx.recv().unwrap();
        })
        .unwrap();
    let monitor = membership
        .spawn(Builder::new(), move || {
            monitor_rx.recv().unwrap();
        })
        .unwrap();
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::Pending);
    owner_tx.send(()).unwrap();
    main.join().unwrap();
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::Pending);
    monitor_tx.send(()).unwrap();
    monitor.join().unwrap();
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::Completed);
}

struct Resource {
    ticket: Arc<CaptureCompletion>,
    dropped: Arc<AtomicUsize>,
}
impl Drop for Resource {
    fn drop(&mut self) {
        assert_eq!(self.ticket.snapshot(), CaptureOwnerStatus::Pending);
        self.dropped.fetch_add(1, Ordering::Release);
    }
}

#[test]
fn all_native_guards_and_reservation_drop_before_success() {
    let ticket = Arc::new(CaptureCompletion::default());
    let participant = ticket.begin().unwrap();
    let dropped = Arc::new(AtomicUsize::new(0));
    let resources: Vec<_> = (0..6)
        .map(|_| Resource {
            ticket: ticket.clone(),
            dropped: dropped.clone(),
        })
        .collect();
    participant
        .spawn(Builder::new(), move || {
            drop(resources);
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(dropped.load(Ordering::Acquire), 6);
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::Completed);
}

#[test]
fn failed_spawn_disposes_captured_resources_before_completion() {
    let ticket = Arc::new(CaptureCompletion::default());
    let participant = ticket.begin().unwrap();
    let dropped = Arc::new(AtomicUsize::new(0));
    let resource = Resource {
        ticket: ticket.clone(),
        dropped: dropped.clone(),
    };
    // Exact Task drop path used when Builder rejects/drops its uncalled closure;
    // deterministic and does not depend on exhausting OS threads.
    let task = Task {
        work: Some(move || drop(resource)),
        participant,
    };
    drop(task);
    assert_eq!(dropped.load(Ordering::Acquire), 1);
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::Completed);
}

#[test]
fn unentered_closure_cleanup_unwind_cannot_report_completion() {
    struct FailedDrop;
    impl Drop for FailedDrop {
        fn drop(&mut self) {
            panic!("fake failed constructor cleanup");
        }
    }
    let ticket = Arc::new(CaptureCompletion::default());
    let participant = ticket.begin().unwrap();
    let resource = FailedDrop;
    let task = Task {
        work: Some(move || drop(resource)),
        participant,
    };
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(task))).is_err());
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::Retained);
}

#[test]
fn retained_cleanup_is_sticky_after_every_worker_returns() {
    let ticket = Arc::new(CaptureCompletion::default());
    let owner = ticket.begin().unwrap();
    let monitor = ticket.participant().unwrap();
    ticket.retain();
    drop(owner);
    drop(monitor);
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::Retained);
    assert!(ticket.participant().is_err());
}

#[test]
fn owner_unwind_never_establishes_completion() {
    let ticket = Arc::new(CaptureCompletion::default());
    assert!(ticket
        .begin()
        .unwrap()
        .spawn(Builder::new(), || panic!("fake native owner unwind"))
        .unwrap()
        .join()
        .is_err());
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::Retained);
}

#[test]
fn closed_event_alone_does_not_finish_pending_activation_or_final_guards() {
    let ticket = Arc::new(CaptureCompletion::default());
    let owner = ticket.begin().unwrap();
    let (closed_tx, closed_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let thread = owner
        .spawn(Builder::new(), move || {
            closed_tx.send(()).unwrap(); // stand-in for an earlier successful CloseHandle
            release_rx.recv().unwrap(); // remaining COM apartment/reservation lifetime
        })
        .unwrap();
    closed_rx.recv().unwrap();
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::Pending);
    release_tx.send(()).unwrap();
    thread.join().unwrap();
    assert_eq!(ticket.snapshot(), CaptureOwnerStatus::Completed);
}
