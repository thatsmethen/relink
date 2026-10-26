//! Single-owner event disposal and separately retained cleanup evidence.
//! No Windows API is called here; the owning adapter supplies CloseHandle.
use crate::{application::ProcessInstance, backend, AudioError};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WasapiEventClose {
    NotCreated,
    Pending,
    Succeeded,
    Failed { hresult: i32 },
}

#[derive(Default)]
pub(crate) struct CloseRecord(AtomicU64);
impl CloseRecord {
    pub(crate) fn snapshot(&self) -> WasapiEventClose {
        match self.0.load(Ordering::Acquire) {
            0 => WasapiEventClose::NotCreated,
            1 => WasapiEventClose::Pending,
            2 => WasapiEventClose::Succeeded,
            packed => WasapiEventClose::Failed {
                hresult: (packed >> 32) as u32 as i32,
            },
        }
    }
    pub(crate) fn bytes() -> usize {
        std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>()
    }
}

/// The numeric handle is moved into exactly one owner. A failed close is never
/// retried: Windows may already have invalidated/reused that numeric value.
pub(crate) struct OwnedEvent<H> {
    handle: Option<H>,
    close: fn(H) -> Result<(), i32>,
    record: Arc<CloseRecord>,
}
impl<H> OwnedEvent<H> {
    pub(crate) fn new(
        handle: H,
        close: fn(H) -> Result<(), i32>,
        record: Arc<CloseRecord>,
    ) -> Self {
        assert_eq!(
            record.0.swap(1, Ordering::AcqRel),
            0,
            "an event record cannot be reused"
        );
        Self {
            handle: Some(handle),
            close,
            record,
        }
    }
    pub(crate) fn handle(&self) -> &H {
        self.handle.as_ref().expect("event owner is alive")
    }
}
impl<H> Drop for OwnedEvent<H> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let packed = match (self.close)(handle) {
                Ok(()) => 2,
                Err(status) => (u64::from(status as u32) << 32) | 3,
            };
            self.record.0.store(packed, Ordering::Release);
        }
    }
}

/// Rust drops fields in declaration order, including after NativeCapture::drop.
/// Keep capture/client references gone before the event and COM apartment.
#[allow(dead_code)] // realtime/apartment exist for ordered destruction.
pub(crate) struct NativeResources<C, L, E, R, A> {
    pub(crate) capture: C,
    pub(crate) client: L,
    pub(crate) event: E,
    pub(crate) realtime: R,
    pub(crate) apartment: A,
}

/// Establish ordered ownership before any fallible client configuration. The
/// same holder releases partial services before the attached event on unwind.
#[allow(clippy::type_complexity)]
pub(crate) fn prepare_resources<C, L, E, R, A, T, Error>(
    client: L,
    apartment: A,
    create_event: impl FnOnce() -> Result<E, Error>,
    configure: impl FnOnce(&mut NativeResources<Option<C>, L, E, Option<R>, A>) -> Result<T, Error>,
) -> Result<(NativeResources<Option<C>, L, E, Option<R>, A>, T), Error> {
    let owner_apartment = apartment;
    let owned_client = client;
    let event = create_event()?;
    let mut resources = NativeResources {
        capture: None,
        client: owned_client,
        event,
        realtime: None,
        apartment: owner_apartment,
    };
    let configured = configure(&mut resources)?;
    Ok((resources, configured))
}

/// This guard is established before activation/configuration. It has no graph,
/// CaptureState or PCM reference, and covers constructor unwind and late cancel.
pub(crate) struct CleanupReservation<I, R> {
    identity: Option<I>,
    reservation: Option<Arc<R>>,
    close: Arc<CloseRecord>,
}
impl<I, R> CleanupReservation<I, R> {
    pub(crate) fn new(identity: I, reservation: Arc<R>, close: Arc<CloseRecord>) -> Self {
        Self {
            identity: Some(identity),
            reservation: Some(reservation),
            close,
        }
    }
    pub(crate) fn retain(&mut self) {
        std::mem::forget(self.identity.take());
        std::mem::forget(self.reservation.take());
    }
}
impl<I, R> Drop for CleanupReservation<I, R> {
    fn drop(&mut self) {
        if matches!(self.close.snapshot(), WasapiEventClose::Failed { .. }) {
            self.retain();
        }
    }
}

/// Stop failure keeps all native resources alive. Successful Stop releases them
/// completely before reading the final event result. Never retry a failed close.
pub(crate) fn release_native<N>(
    native: &mut Option<N>,
    close: &CloseRecord,
    stop: impl FnOnce(&mut N) -> Result<(), i32>,
) -> Result<(), i32> {
    if let Some(value) = native.as_mut() {
        if let Err(status) = stop(value) {
            std::mem::forget(native.take());
            return Err(status);
        }
    }
    drop(native.take());
    match close.snapshot() {
        WasapiEventClose::Failed { hresult } => Err(hresult),
        _ => Ok(()),
    }
}

// The bounded registry owns the close evidence separately from graph telemetry.
// Cancelled owners can finish after a new session has replaced the old PCM ring.
pub(crate) const MAX_RESERVED_BYTES: usize = 256 * 1024 * 1024;
pub(crate) struct Reserved {
    key: String,
    scope: Vec<ProcessInstance>,
    bytes: usize,
    close: Arc<CloseRecord>,
}
pub(crate) struct Registry {
    slots: Vec<Reserved>,
    bytes: usize,
}
impl Registry {
    pub(crate) const fn new() -> Self {
        Self {
            slots: Vec::new(),
            bytes: 0,
        }
    }
    pub(crate) fn acquire(
        &mut self,
        key: String,
        scope: &[ProcessInstance],
        bytes: usize,
        close: Arc<CloseRecord>,
    ) -> Result<(), AudioError> {
        let mut overlap = None;
        for reserved in &self.slots {
            if reserved.key == key || reserved.scope.iter().any(|process| scope.contains(process)) {
                let status = reserved.close.snapshot();
                if let WasapiEventClose::Failed { hresult } = status {
                    return Err(backend("application owner reservation", None, format!(
                        "a selected process tree overlaps a retained owner whose CloseHandle failed (HRESULT 0x{:08X}, {hresult}); its process identity and reservation remain retained. Restart ReLink before retrying this application", hresult as u32)));
                }
                overlap = Some(status);
            }
        }
        if let Some(status) = overlap {
            return Err(backend("application owner reservation", None, format!(
                "a selected process tree overlaps a live or retained owner (event close status: {status:?}); wait for cleanup or restart ReLink")));
        }
        let total = self.bytes.checked_add(bytes).filter(|&n| n <= MAX_RESERVED_BYTES).ok_or_else(|| backend("application owner reservation", None, "Windows application owners exhausted their 256 MiB explicit allocation budget; no partial tree was selected"))?;
        self.slots.push(Reserved {
            key,
            scope: scope.to_vec(),
            bytes,
            close,
        });
        self.bytes = total;
        Ok(())
    }
    pub(crate) fn remove(&mut self, key: &str) {
        if let Some(index) = self.slots.iter().position(|reserved| reserved.key == key) {
            self.bytes -= self.slots.remove(index).bytes;
        }
    }
}

#[cfg(test)]
#[path = "wasapi_event_tests.rs"]
mod tests;
