//! Capture lifetime proof, independent of PCM progress or event-handle closure.
//! Only control/native owner threads register or finish work; callbacks read no
//! locks and do not own these guards. A blocked owner remains Pending.
use std::{
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread::{Builder, JoinHandle},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureOwnerStatus {
    /// No native construction or activation has been attempted for this ring.
    NotStarted,
    /// At least one owner/dependency still has work or resources outstanding.
    Pending,
    /// Every registered owner returned after releasing its native resources.
    Completed,
    /// Cleanup failed or unwound; completion must not be inferred afterward.
    Retained,
}

const STATE_MASK: usize = 3;
const PENDING: usize = 1;
const COMPLETED: usize = 2;
const RETAINED: usize = 3;
const ONE: usize = 4;

#[derive(Default)]
pub(crate) struct CaptureCompletion(AtomicUsize);
impl CaptureCompletion {
    pub(crate) fn bytes() -> usize {
        std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>()
    }
    pub(crate) fn snapshot(&self) -> CaptureOwnerStatus {
        match self.0.load(Ordering::Acquire) & STATE_MASK {
            0 => CaptureOwnerStatus::NotStarted,
            PENDING => CaptureOwnerStatus::Pending,
            COMPLETED => CaptureOwnerStatus::Completed,
            _ => CaptureOwnerStatus::Retained,
        }
    }
    /// One construction attempt per capture ring. Call before reservation/spawn.
    pub(crate) fn begin(self: &Arc<Self>) -> io::Result<Participant> {
        self.0
            .compare_exchange(0, ONE | PENDING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| io::Error::other("capture owner was already attempted"))?;
        Ok(Participant::new(self.clone()))
    }
    /// Register a dependent worker before spawning it. Completed tickets cannot
    /// be reopened, and retained owners cannot admit new dependencies.
    pub(crate) fn participant(self: &Arc<Self>) -> io::Result<Participant> {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                if state & STATE_MASK == PENDING && state >= ONE {
                    state.checked_add(ONE)
                } else {
                    None
                }
            })
            .map_err(|_| io::Error::other("capture owner cannot add a dependency"))?;
        Ok(Participant::new(self.clone()))
    }
    pub(crate) fn retain(&self) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state & STATE_MASK != COMPLETED).then_some((state & !STATE_MASK) | RETAINED)
            });
    }
    fn finish(&self) {
        let result = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                if state < ONE {
                    return None;
                }
                let next = state - ONE;
                Some(if next == PENDING { COMPLETED } else { next })
            });
        debug_assert!(result.is_ok(), "capture participant finished twice");
    }
}

pub(crate) struct Participant {
    ticket: Arc<CaptureCompletion>,
    entered: bool,
    returned: bool,
}
impl Participant {
    fn new(ticket: Arc<CaptureCompletion>) -> Self {
        Self {
            ticket,
            entered: false,
            returned: false,
        }
    }
    /// The work closure is dropped BEFORE its participant on spawn failure.
    /// On success it is fully consumed (including all owned resource drops)
    /// before completion is published. An unwind latches Retained instead.
    pub(crate) fn spawn<F: FnOnce() + Send + 'static>(
        self,
        builder: Builder,
        work: F,
    ) -> io::Result<JoinHandle<()>> {
        let task = Task {
            work: Some(work),
            participant: self,
        };
        builder.spawn(move || task.run())
    }
}
impl Drop for Participant {
    fn drop(&mut self) {
        if std::thread::panicking() || (self.entered && !self.returned) {
            self.ticket.retain();
        }
        self.ticket.finish();
    }
}
struct Task<F: FnOnce()> {
    // Field order is intentional: failed spawn disposes the resource closure
    // before the unentered participant can establish completion.
    work: Option<F>,
    participant: Participant,
}
impl<F: FnOnce()> Task<F> {
    fn run(mut self) {
        self.participant.entered = true;
        (self.work.take().expect("one owner invocation"))();
        self.participant.returned = true;
    }
}

#[cfg(test)]
#[path = "capture_owner_tests.rs"]
mod tests;
