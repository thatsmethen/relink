use crate::AudioError;
use relink_core::{PreparedControls, PreparedDsp, ResolvedControl, Uuid};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

struct AtomicControl {
    id: Uuid,
    left: AtomicU32,
    right: AtomicU32,
    active: AtomicBool,
}

struct AtomicDsp {
    words: [AtomicU32; PreparedDsp::WORD_COUNT],
}
impl AtomicDsp {
    fn new(dsp: &PreparedDsp) -> Self {
        Self {
            words: dsp.to_words().map(AtomicU32::new),
        }
    }
    fn store(&self, dsp: &PreparedDsp) {
        for (target, value) in self.words.iter().zip(dsp.to_words()) {
            target.store(value, Ordering::SeqCst);
        }
    }
    fn read(&self) -> Option<PreparedDsp> {
        let words = std::array::from_fn(|index| self.words[index].load(Ordering::SeqCst));
        PreparedDsp::from_words(words).ok()
    }
}
impl AtomicControl {
    fn new(control: &ResolvedControl) -> Self {
        Self {
            id: control.id,
            left: AtomicU32::new(control.gains[0].to_bits()),
            right: AtomicU32::new(control.gains[1].to_bits()),
            active: AtomicBool::new(control.active),
        }
    }
    fn store(&self, control: &ResolvedControl) {
        self.left
            .store(control.gains[0].to_bits(), Ordering::SeqCst);
        self.right
            .store(control.gains[1].to_bits(), Ordering::SeqCst);
        self.active.store(control.active, Ordering::SeqCst);
    }
    fn read_into(&self, control: &mut ResolvedControl) {
        control.gains = [
            f32::from_bits(self.left.load(Ordering::SeqCst)),
            f32::from_bits(self.right.load(Ordering::SeqCst)),
        ];
        control.active = self.active.load(Ordering::SeqCst);
    }
}

/// One control-thread writer; any callback readers use their own preallocated
/// snapshots. Every field and the version uses SeqCst: if any write interleaves
/// the copy, the total atomic order makes the final version odd or different.
/// No plain data is concurrently mutated, no reader retries/spins, and an
/// incomplete bank is never applied to the graph. A stalled writer preserves
/// the last complete controls instead of blocking the callback.
pub(crate) struct ControlBank {
    version: AtomicU64,
    sample_rate: u32,
    inputs: Vec<AtomicControl>,
    sends: Vec<AtomicControl>,
    buses: Vec<AtomicControl>,
    input_dsp: Vec<AtomicDsp>,
    bus_dsp: Vec<AtomicDsp>,
}
impl ControlBank {
    pub(crate) fn estimated_bytes(records: usize, dsp_channels: usize) -> Option<usize> {
        records
            .checked_mul(
                std::mem::size_of::<AtomicControl>() + 2 * std::mem::size_of::<ResolvedControl>(),
            )
            .and_then(|n| {
                dsp_channels
                    .checked_mul(
                        std::mem::size_of::<AtomicDsp>() + 2 * std::mem::size_of::<PreparedDsp>(),
                    )
                    .and_then(|dsp| n.checked_add(dsp))
            })
            .and_then(|n| {
                n.checked_add(
                    std::mem::size_of::<Self>() + 2 * std::mem::size_of::<PreparedControls>(),
                )
            })
    }
    pub(crate) fn new(controls: &PreparedControls) -> Self {
        Self {
            version: AtomicU64::new(0),
            sample_rate: controls.sample_rate,
            inputs: controls.inputs.iter().map(AtomicControl::new).collect(),
            sends: controls.sends.iter().map(AtomicControl::new).collect(),
            buses: controls.buses.iter().map(AtomicControl::new).collect(),
            input_dsp: controls.input_dsp.iter().map(AtomicDsp::new).collect(),
            bus_dsp: controls.bus_dsp.iter().map(AtomicDsp::new).collect(),
        }
    }
    pub(crate) fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }
    pub(crate) fn publish(&self, controls: &PreparedControls) -> Result<u64, AudioError> {
        if controls.sample_rate != self.sample_rate
            || controls.input_dsp.len() != self.input_dsp.len()
            || controls.bus_dsp.len() != self.bus_dsp.len()
            || controls
                .input_dsp
                .iter()
                .chain(&controls.bus_dsp)
                .any(|dsp| dsp.sample_rate() != self.sample_rate)
        {
            return Err(AudioError::InvalidSession(
                "live effects do not match the active graph and sample rate".into(),
            ));
        }
        for (bank, next) in [
            (&self.inputs, &controls.inputs),
            (&self.sends, &controls.sends),
            (&self.buses, &controls.buses),
        ] {
            if bank.len() != next.len() || bank.iter().zip(next).any(|(a, b)| a.id != b.id) {
                return Err(AudioError::InvalidSession(
                    "live controls do not match the active graph".into(),
                ));
            }
        }
        let old = self.version();
        let next = old
            .checked_add(2)
            .filter(|_| old.is_multiple_of(2))
            .ok_or_else(|| {
                AudioError::InvalidSession(
                    "the live control version is exhausted or already being written".into(),
                )
            })?;
        self.version.store(old + 1, Ordering::SeqCst);
        for (bank, values) in [
            (&self.inputs, &controls.inputs),
            (&self.sends, &controls.sends),
            (&self.buses, &controls.buses),
        ] {
            for (target, value) in bank.iter().zip(values) {
                target.store(value);
            }
        }
        for (bank, values) in [
            (&self.input_dsp, &controls.input_dsp),
            (&self.bus_dsp, &controls.bus_dsp),
        ] {
            for (target, value) in bank.iter().zip(values) {
                target.store(value);
            }
        }
        self.version.store(next, Ordering::SeqCst);
        Ok(next)
    }
    pub(crate) fn copy_if_new(&self, scratch: &mut PreparedControls, applied: u64) -> Option<u64> {
        let version = self.version();
        if !version.is_multiple_of(2)
            || version == applied
            || scratch.sample_rate != self.sample_rate
        {
            return None;
        }
        for (bank, destination) in [
            (&self.inputs, &mut scratch.inputs),
            (&self.sends, &mut scratch.sends),
            (&self.buses, &mut scratch.buses),
        ] {
            if bank.len() != destination.len() {
                return None;
            }
            for (source, target) in bank.iter().zip(destination) {
                source.read_into(target);
            }
        }
        for (bank, destination) in [
            (&self.input_dsp, &mut scratch.input_dsp),
            (&self.bus_dsp, &mut scratch.bus_dsp),
        ] {
            if bank.len() != destination.len() {
                return None;
            }
            for (source, target) in bank.iter().zip(destination) {
                *target = source.read()?;
            }
        }
        (self.version() == version).then_some(version)
    }

    #[cfg(test)]
    pub(crate) fn begin_incomplete_test_write(&self, controls: &PreparedControls) {
        self.version.store(1, Ordering::SeqCst);
        for (bank, values) in [
            (&self.inputs, &controls.inputs),
            (&self.sends, &controls.sends),
            (&self.buses, &controls.buses),
        ] {
            for (target, value) in bank.iter().zip(values) {
                target.store(value);
            }
        }
        for (bank, values) in [
            (&self.input_dsp, &controls.input_dsp),
            (&self.bus_dsp, &controls.bus_dsp),
        ] {
            for (target, value) in bank.iter().zip(values) {
                target.store(value);
            }
        }
    }
}
