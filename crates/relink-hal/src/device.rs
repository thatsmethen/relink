use crate::{abi::*, ring::Ring};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

pub const SAMPLE_RATE: u64 = 48_000;
pub const PERIOD: u64 = 16_384;
pub const CAPACITY: usize = 16_384;
pub const MAX_IO_FRAMES: u32 = 8_192;
pub const LOOPBACK_DELAY: u64 = 512;
const CLIENT_CAPACITY: usize = 64;

#[derive(Clone, Copy)]
struct Client {
    id: Option<u32>,
    running: bool,
}
impl Client {
    const EMPTY: Self = Self {
        id: None,
        running: false,
    };
}

pub struct Device {
    pub id: ObjectId,
    pub input: ObjectId,
    pub output: ObjectId,
    pub name: &'static str,
    pub uid: &'static str,
    pub running: AtomicU32,
    pub anchor: AtomicU64,
    pub ring: Ring<CAPACITY>,
    clients: Mutex<[Client; CLIENT_CAPACITY]>,
}
impl Device {
    pub const fn new(id: ObjectId, name: &'static str, uid: &'static str) -> Self {
        Self {
            id,
            input: id + 1,
            output: id + 2,
            name,
            uid,
            running: AtomicU32::new(0),
            anchor: AtomicU64::new(0),
            ring: Ring::new(),
            clients: Mutex::new([Client::EMPTY; CLIENT_CAPACITY]),
        }
    }
    pub fn add_client(&self, id: u32) -> Status {
        if id == 0 {
            return ILLEGAL;
        }
        let Ok(mut clients) = self.clients.lock() else {
            return ILLEGAL;
        };
        if clients.iter().any(|c| c.id == Some(id)) {
            return ILLEGAL;
        }
        let Some(client) = clients.iter_mut().find(|c| c.id.is_none()) else {
            return ILLEGAL;
        };
        client.id = Some(id);
        OK
    }
    pub fn remove_client(&self, id: u32) -> Status {
        let Ok(mut clients) = self.clients.lock() else {
            return ILLEGAL;
        };
        let Some(client) = clients.iter_mut().find(|c| c.id == Some(id)) else {
            return ILLEGAL;
        };
        // The host must stop a client's IO before removing that client.
        if client.running {
            return ILLEGAL;
        }
        *client = Client::EMPTY;
        OK
    }
    pub fn start(&self, id: u32) -> Result<bool, Status> {
        let mut clients = self.clients.lock().map_err(|_| ILLEGAL)?;
        let client = clients
            .iter_mut()
            .find(|c| c.id == Some(id))
            .ok_or(ILLEGAL)?;
        if client.running {
            return Err(ILLEGAL);
        }
        let was_stopped = self.running.load(Ordering::Acquire) == 0;
        if was_stopped {
            self.ring.clear();
        }
        client.running = true;
        self.running.fetch_add(1, Ordering::Release);
        Ok(was_stopped)
    }
    pub fn stop(&self, id: u32) -> Result<bool, Status> {
        let mut clients = self.clients.lock().map_err(|_| ILLEGAL)?;
        let client = clients
            .iter_mut()
            .find(|c| c.id == Some(id))
            .ok_or(ILLEGAL)?;
        if !client.running {
            return Err(ILLEGAL);
        }
        client.running = false;
        Ok(self.running.fetch_sub(1, Ordering::AcqRel) == 1)
    }
}

// Add independent entries here to publish additional cables. Object IDs reserve a
// three-object block per device. No IO path uses a shared sample ring across devices.
pub static DEVICES: [Device; 2] = [
    Device::new(100, "ReLink", "au.textual.ravelink.cable.1"),
    Device::new(200, "ReLink 2", "au.textual.ravelink.cable.2"),
];

pub fn find(id: ObjectId) -> Option<&'static Device> {
    DEVICES.iter().find(|d| d.id == id)
}
pub fn owner(id: ObjectId) -> Option<&'static Device> {
    DEVICES
        .iter()
        .find(|d| d.id == id || d.input == id || d.output == id)
}

/// Integer arithmetic keeps the clock on a single monotonic Mach timeline without
/// accumulating rounding error. Timebase is nanoseconds per Mach tick.
pub fn timestamp(now: u64, anchor: u64, numerator: u32, denominator: u32) -> (f64, u64) {
    let elapsed = u128::from(now.saturating_sub(anchor));
    let frames = elapsed * u128::from(numerator) * u128::from(SAMPLE_RATE)
        / (u128::from(denominator) * 1_000_000_000);
    let sample = frames / u128::from(PERIOD) * u128::from(PERIOD);
    let ticks = sample * 1_000_000_000 * u128::from(denominator)
        / (u128::from(SAMPLE_RATE) * u128::from(numerator));
    (sample as f64, anchor.saturating_add(ticks as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_is_monotonic_and_period_quantized() {
        let mut previous = (0.0, 1000);
        for ticks in (1000..30_000_000).step_by(997) {
            let current = timestamp(ticks, 1000, 125, 3);
            assert!(current.0 >= previous.0 && current.1 >= previous.1);
            assert!(current.1 <= ticks);
            assert_eq!(current.0 % PERIOD as f64, 0.0);
            previous = current;
        }
        assert_eq!(timestamp(1_000_000_000, 0, 1, 1), (32_768.0, 682_666_666));
    }
    #[test]
    fn clients_are_balanced_and_devices_are_isolated() {
        let a = Device::new(4, "a", "a");
        let b = Device::new(7, "b", "b");
        assert_eq!(a.start(3), Err(ILLEGAL));
        assert_eq!(a.add_client(3), OK);
        assert_eq!(a.add_client(4), OK);
        assert_eq!(a.start(3), Ok(true));
        assert_eq!(a.start(3), Err(ILLEGAL));
        assert_eq!(a.start(4), Ok(false));
        assert_eq!(a.remove_client(3), ILLEGAL);
        assert_eq!(a.stop(3), Ok(false));
        assert_eq!(a.stop(4), Ok(true));
        assert_eq!(a.stop(4), Err(ILLEGAL));
        assert_eq!(a.remove_client(3), OK);
        assert!(a.ring.write(15, 0.5, 0.75));
        assert_eq!(b.ring.read(15), [0.0; 2]);
    }
}
