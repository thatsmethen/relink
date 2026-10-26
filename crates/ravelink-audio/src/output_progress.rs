//! Output liveness observation runs independently of every native owner/control
//! call. It never queries a device or treats a timed-out teardown as complete.
use crate::{AudioError, AudioFault, FaultOrigin, OutputPermit, MAX_CALLBACK_FRAMES};
use std::{
    thread,
    time::{Duration, Instant},
};

pub(crate) const STACK_BYTES: usize = 2 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(20);

pub(crate) fn deadline(rate: u32) -> Duration {
    Duration::from_secs_f64(3.0 * MAX_CALLBACK_FRAMES as f64 / f64::from(rate.max(1)))
        .max(Duration::from_secs(2))
}

struct Progress {
    count: u64,
    observed_at: Instant,
    timeout: Duration,
}
impl Progress {
    fn new(count: u64, now: Instant, timeout: Duration) -> Self {
        Self {
            count,
            observed_at: now,
            timeout,
        }
    }
    fn observe(&mut self, count: u64, now: Instant) -> bool {
        if count < self.count {
            return false;
        }
        if count > self.count {
            self.count = count;
            self.observed_at = now;
        }
        now.saturating_duration_since(self.observed_at) < self.timeout
    }
}

/// The closure reads a progress atomic only. The supervisor is deliberately
/// independent of native owners and the control thread (which can enumerate
/// devices). No callback waits for it. Its stop is cooperative and it is never
/// joined by the GUI or control thread. OS scheduling remains outside our bound.
pub(crate) fn supervise(
    permit: OutputPermit,
    timeout: Duration,
    count: impl Fn() -> u64 + Send + 'static,
) -> Result<(), AudioError> {
    let initial = count();
    let start = Instant::now();
    thread::Builder::new()
        .name("ravelink-output-progress".into())
        .stack_size(STACK_BYTES)
        .spawn(move || {
            let mut progress = Progress::new(initial, start, timeout);
            while permit.authorized() {
                if !progress.observe(count(), Instant::now()) {
                    if permit.authorized() {
                        permit.fail_at(
                            AudioFault::StreamInvalidated,
                            FaultOrigin::NativeOutputProgress,
                            permit.metadata(),
                        );
                        permit.cancel();
                    }
                    return;
                }
                thread::sleep(POLL);
            }
        })
        .map(|_| ())
        .map_err(|error| crate::backend("output progress supervisor", None, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ravelink_core::{Bus, Session};
    use std::sync::atomic::Ordering;

    #[test]
    fn supervisor_revokes_stalled_output_while_control_mutex_is_held() {
        let mut session = Session::new("Offline progress supervisor", 48_000);
        let bus = session.add_bus(Bus::new("Mix")).unwrap();
        let permit = crate::tests::render(&session, bus, 2).permit();
        let state_guard = permit.shared.state();
        supervise(permit.clone(), Duration::from_millis(20), || 0).unwrap();

        // A device enumeration or native owner call must not prevent the
        // independent supervisor from revoking audio. Never open a device.
        let deadline = Instant::now() + Duration::from_secs(2);
        while permit.wanted.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        let revoked = !permit.wanted.load(Ordering::Acquire);
        let fault = permit.telemetry.fault.snapshot();
        permit.cancel();
        drop(state_guard);
        assert!(revoked, "supervisor waited for the held control mutex");
        let fault = fault.expect("supervisor must retain its reason before revocation");
        assert_eq!(fault.fault, AudioFault::StreamInvalidated);
        assert_eq!(fault.origin, FaultOrigin::NativeOutputProgress);
    }

    #[test]
    fn silence_and_pcm_progress_use_the_same_observed_deadline() {
        let now = Instant::now();
        let mut progress = Progress::new(3, now, Duration::from_secs(2));
        assert!(progress.observe(3, now + Duration::from_millis(1999)));
        assert!(progress.observe(4, now + Duration::from_millis(1999)));
        assert!(progress.observe(4, now + Duration::from_millis(3998)));
        assert!(!progress.observe(4, now + Duration::from_millis(3999)));
    }

    #[test]
    fn missing_first_callback_and_regressed_counter_fail() {
        let now = Instant::now();
        let mut missing = Progress::new(0, now, Duration::from_secs(2));
        assert!(!missing.observe(0, now + Duration::from_secs(2)));
        let mut regressed = Progress::new(100, now, Duration::from_secs(2));
        assert!(!regressed.observe(99, now));
    }

    #[test]
    fn progress_deadline_covers_three_maximum_callbacks_without_shortening_two_seconds() {
        assert_eq!(deadline(48000), Duration::from_secs(2));
        assert_eq!(deadline(384000), Duration::from_secs(2));
        assert!(deadline(8000) >= Duration::from_secs_f64(6.144));
    }
}
