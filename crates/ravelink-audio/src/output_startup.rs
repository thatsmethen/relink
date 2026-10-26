//! Control-thread startup ordering for an explicitly selected native output.
//! The program gate stays closed until native output progress and input startup
//! both succeed. A deadline can reject a late OS return, not interrupt that call.
use crate::{AudioError, OutputPermit};
use std::time::{Duration, Instant};

pub(crate) trait OutputStart {
    /// Start native IO with the program gate still closed. Success must mean the
    /// native start operation succeeded, not merely that it was queued.
    fn start(&self) -> Result<(), AudioError>;
    /// Require native callback progress while the program gate remains closed.
    /// This wait must honor its supplied remaining monotonic time budget.
    fn wait_for_progress(&self, timeout: Duration) -> Result<(), AudioError>;
    /// Open the program gate after rechecking the owner's authorization.
    fn activate(&self) -> Result<(), AudioError>;
    /// Revoke the local program gate. Must be nonblocking, idempotent and not
    /// panic; native teardown and its lifetime accounting are separate.
    fn cancel(&self);
}

struct StartupGuard<'a, O: OutputStart + ?Sized> {
    output: &'a O,
    permit: &'a OutputPermit,
    armed: bool,
}
impl<O: OutputStart + ?Sized> Drop for StartupGuard<'_, O> {
    fn drop(&mut self) {
        if self.armed {
            // Revoke every callback sharing this epoch's wanted token before
            // calling the owner. Even a misbehaving cancel cannot undo this.
            self.permit.cancel();
            self.output.cancel();
        }
    }
}

fn remaining(
    permit: &OutputPermit,
    began: Instant,
    budget: Duration,
) -> Result<Duration, AudioError> {
    if !permit.authorized() {
        return Err(AudioError::Cancelled);
    }
    budget
        .checked_sub(began.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(AudioError::Timeout {
            operation: "output startup",
        })
}

/// Coordinates one output and the inputs associated with its render epoch.
/// Any error or unwind revokes the shared permit and the local output gate.
/// The input closure must retain/clean up its native owners appropriately; this
/// helper cannot release resources or interrupt a blocked native operation.
/// Callbacks must continue checking their permit after successful startup.
pub(crate) fn start_output<O: OutputStart + ?Sized>(
    output: &O,
    permit: &OutputPermit,
    budget: Duration,
    start_inputs: impl FnOnce() -> Result<(), AudioError>,
) -> Result<(), AudioError> {
    let began = Instant::now();
    let mut guard = StartupGuard {
        output,
        permit,
        armed: true,
    };
    remaining(permit, began, budget)?;
    output.start()?;
    output.wait_for_progress(remaining(permit, began, budget)?)?;
    remaining(permit, began, budget)?;
    start_inputs()?;
    remaining(permit, began, budget)?;
    output.activate()?;
    remaining(permit, began, budget)?;
    guard.armed = false;
    Ok(())
}

#[cfg(test)]
#[path = "output_startup_tests.rs"]
mod tests;
