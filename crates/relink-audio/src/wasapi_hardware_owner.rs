//! Portable reservation and teardown controls for Windows hardware capture.
//! Native calls run on the owning MTA thread, never under the registry lock.
use crate::{backend, wasapi_event::CloseRecord, AudioError, WasapiEventClose};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

pub(crate) const MAX_OWNERS: usize = 64;
pub(crate) const MAX_OWNER_BYTES: usize = 256 * 1024 * 1024;

/// A Start call that fails can still leave native state uncertain. Only a
/// successful Stop return clears the obligation, including S_FALSE.
#[derive(Default)]
pub(crate) struct StopRequirement(bool);
impl StopRequirement {
    pub(crate) fn start_entered(&mut self) {
        self.0 = true;
    }
    pub(crate) fn required(&self) -> bool {
        self.0
    }
    pub(crate) fn stop(
        &mut self,
        call: impl FnOnce() -> Result<i32, i32>,
    ) -> Result<Option<i32>, i32> {
        if !self.0 {
            return Ok(None);
        }
        let status = call()?;
        self.0 = false;
        Ok(Some(status))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StopState {
    NotRequested,
    Pending,
    Returned { hresult: i32 },
}

/// This contains no PCM, CaptureState, permit or session reference. Registry
/// ownership keeps late Stop/CloseHandle results available after graph removal.
pub(crate) struct Record {
    stop: AtomicU64,
    pub(crate) close: Arc<CloseRecord>,
}
impl Record {
    pub(crate) fn new(close: Arc<CloseRecord>) -> Self {
        Self {
            stop: AtomicU64::new(0),
            close,
        }
    }
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn bytes() -> usize {
        std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>()
    }
    pub(crate) fn stop(&self) -> StopState {
        match self.stop.load(Ordering::Acquire) {
            0 => StopState::NotRequested,
            1 => StopState::Pending,
            packed => StopState::Returned {
                hresult: (packed >> 32) as u32 as i32,
            },
        }
    }
    fn stop_proven(&self) -> bool {
        matches!(self.stop(), StopState::NotRequested)
            || matches!(self.stop(), StopState::Returned { hresult } if hresult >= 0)
    }
    pub(crate) fn releasable(&self) -> bool {
        self.stop_proven()
            && matches!(
                self.close.snapshot(),
                WasapiEventClose::NotCreated | WasapiEventClose::Succeeded
            )
    }
}

/// A failed Stop leaves the complete native owner untouched and never retries
/// Stop or CloseHandle. Successful Stop drops COM before the event according to
/// NativeResources field order. None means the client was never started.
pub(crate) fn release<N>(
    native: &mut Option<N>,
    record: &Record,
    stop: impl FnOnce(&mut N) -> Result<Option<i32>, i32>,
) -> Result<(), i32> {
    if let Some(value) = native.as_mut() {
        record.stop.store(1, Ordering::Release);
        let result = stop(value);
        let status = match result {
            Ok(None) => 0,
            Ok(Some(status)) | Err(status) => (u64::from(status as u32) << 32) | 2,
        };
        record.stop.store(status, Ordering::Release);
        if let Err(status) = result {
            std::mem::forget(native.take());
            return Err(status);
        }
    }
    drop(native.take());
    match record.stop() {
        StopState::Returned { hresult } if hresult < 0 => return Err(hresult),
        StopState::Pending => return Err(0x8000_000A_u32 as i32), // E_PENDING
        _ => {}
    }
    match record.close.snapshot() {
        WasapiEventClose::Failed { hresult } => Err(hresult),
        WasapiEventClose::Pending => Err(0x8000_000A_u32 as i32),
        _ => Ok(()),
    }
}

pub(crate) struct Reserved {
    id: String,
    bytes: usize,
    record: Arc<Record>,
}
pub(crate) struct Registry {
    slots: [Option<Reserved>; MAX_OWNERS],
    bytes: usize,
}
impl Registry {
    pub(crate) const fn new() -> Self {
        Self {
            slots: [const { None }; MAX_OWNERS],
            bytes: 0,
        }
    }
    pub(crate) fn acquire(
        &mut self,
        id: &str,
        bytes: usize,
        record: Arc<Record>,
    ) -> Result<usize, AudioError> {
        if let Some(held) = self.slots.iter().flatten().find(|held| held.id == id) {
            let detail = match held.record.stop() {
                StopState::Returned { hresult } if hresult < 0 => format!(
                    "Stop failed (HRESULT 0x{:08X}, {hresult}); its native client, event and endpoint reservation remain retained. Restart ReLink before retrying this endpoint",
                    hresult as u32
                ),
                stop => match held.record.close.snapshot() {
                    WasapiEventClose::Failed { hresult } => format!(
                        "CloseHandle failed (HRESULT 0x{:08X}, {hresult}); its endpoint reservation remains retained and the handle will not be retried. Restart ReLink before retrying this endpoint",
                        hresult as u32
                    ),
                    close => format!(
                        "endpoint is already owned or awaiting cleanup (Stop: {stop:?}; event: {close:?}); wait for cleanup or restart ReLink"
                    ),
                },
            };
            return Err(backend("WASAPI hardware owner reservation", None, detail));
        }
        let bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|&total| total <= MAX_OWNER_BYTES)
            .ok_or_else(|| {
                backend(
                    "WASAPI hardware owner reservation",
                    None,
                    "hardware owners exhausted their 256 MiB known allocation budget",
                )
            })?;
        let index = self.slots.iter().position(Option::is_none).ok_or_else(|| {
            backend(
                "WASAPI hardware owner reservation",
                None,
                "64 hardware endpoint owner reservations are already occupied",
            )
        })?;
        self.slots[index] = Some(Reserved {
            id: id.to_owned(),
            bytes: bytes - self.bytes,
            record,
        });
        self.bytes = bytes;
        Ok(index)
    }
    pub(crate) fn release(&mut self, index: usize) {
        // A missing native return, failed Stop, or failed event close cannot
        // reopen admission, including after the stream handle has been dropped.
        if self.slots[index]
            .as_ref()
            .is_some_and(|held| held.record.releasable())
        {
            self.bytes -= self.slots[index].take().unwrap().bytes;
        }
    }
}

#[cfg(test)]
#[path = "wasapi_hardware_owner_tests.rs"]
mod tests;
