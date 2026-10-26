//! Per-start authorization shared by every output and input owner. The fanout
//! cancellation bit is the same atomic word used by its publication CAS.
use crate::fanout;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
    Arc, OnceLock,
};

const OPEN: u8 = 1;
const CANCELLED: u8 = 2;

pub(crate) struct EpochGate {
    pub(crate) epoch: AtomicU64,
    pub(crate) wanted: Arc<AtomicBool>,
    program: AtomicU8,
    fanout: OnceLock<fanout::Control>,
}
impl EpochGate {
    pub(crate) fn new(wanted: Arc<AtomicBool>) -> Self {
        Self {
            epoch: AtomicU64::new(u64::MAX),
            wanted,
            program: AtomicU8::new(0),
            fanout: OnceLock::new(),
        }
    }
    pub(crate) fn install_fanout(&self, control: fanout::Control) {
        assert!(self.fanout.set(control).is_ok(), "one fanout per epoch");
        // A cancellation before installation cannot be lost. No owner exists
        // until this installation and recheck are complete.
        if !self.wanted.load(Ordering::Acquire) {
            self.cancel();
        }
    }
    pub(crate) fn cancel(&self) {
        self.program.fetch_or(CANCELLED, Ordering::AcqRel);
        self.wanted.store(false, Ordering::Release);
        if let Some(control) = self.fanout.get() {
            control.cancel();
        }
    }
    pub(crate) fn activate(&self) -> bool {
        if !self.wanted.load(Ordering::Acquire) {
            return false;
        }
        if self
            .program
            .compare_exchange(0, OPEN, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        if !self.wanted.load(Ordering::Acquire) {
            self.cancel();
            return false;
        }
        true
    }
    #[cfg(test)]
    pub(crate) fn fanout_control(&self) -> Option<&fanout::Control> {
        self.fanout.get()
    }
    pub(crate) fn program(&self) -> bool {
        self.program.load(Ordering::Acquire) == OPEN && self.wanted.load(Ordering::Acquire)
    }
}
