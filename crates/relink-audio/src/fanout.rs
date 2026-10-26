//! Multi-output transport shared by native renderers and offline controls.
//!
//! One primary producer reserves an entire callback on every route before its
//! caller advances the graph. PCM and accepted-target-version tags stay private
//! until one shared cursor is release-published. Dropped/failed transactions do
//! not publish. A terminal cancellation bit shares that same atomic word, so a
//! cancel-before-commit cannot race a later successful publication.
//!
//! Each follower starts at source frame zero with zero/versionless FIR
//! prehistory. Priming delays playback but never discards its prefix. The same
//! nominal sample rate is required; independent clocks use bounded ASRC. This
//! module has no OS calls, automatic fault recovery,
//! or hardware-playback acknowledgments.
use crate::capture_transport::{Kernel, CAPACITY, MAX_CORRECTION};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

const CANCELLED: u64 = 1 << 63;
const CURSOR_BITS: u32 = 48;
const QUANTUM_MASK: u64 = 0x7fff;
/// One word: 48 source-frame bits, 15 largest-quantum bits, one cancellation
/// bit. At 384 kHz the source clock lasts over 23 years. Overflow is rejected
/// before writes; no cursor or epoch is implicitly wrapped/reused.
const MAX_SOURCE_FRAMES: u64 = (1 << CURSOR_BITS) - 1;
const _: () = assert!(crate::MAX_CALLBACK_FRAMES <= QUANTUM_MASK as usize);
fn published_word(cursor: u64, quantum: usize) -> u64 {
    cursor | (quantum as u64) << CURSOR_BITS
}
fn published_quantum(word: u64) -> usize {
    ((word >> CURSOR_BITS) & QUANTUM_MASK) as usize
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidConfiguration,
    BudgetExceeded,
    InvalidBuffer,
    InvalidRoute,
    InvalidVersion,
    IncompleteTransaction,
    Overflow,
    RouteFull {
        route: usize,
    },
    Underflow,
    QuantumExceeded {
        required_margin: usize,
        startup_target: usize,
    },
    Cancelled,
}

struct Shared {
    committed: AtomicU64,
    epoch: u64,
}
struct Queue {
    pcm: Box<[AtomicU64]>,
    versions: Box<[AtomicU64]>,
    released: AtomicU64,
}
impl Queue {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            pcm: (0..CAPACITY).map(|_| AtomicU64::new(0)).collect(),
            versions: (0..CAPACITY).map(|_| AtomicU64::new(0)).collect(),
            released: AtomicU64::new(0),
        })
    }
    fn frame(&self, frame: u64) -> ([f32; 2], u64) {
        let index = frame as usize & (CAPACITY - 1);
        let bits = self.pcm[index].load(Ordering::Relaxed);
        (
            [
                f32::from_bits(bits as u32),
                f32::from_bits((bits >> 32) as u32),
            ],
            self.versions[index].load(Ordering::Relaxed),
        )
    }
}

pub(crate) struct Fanout {
    pub(crate) writer: Writer,
    pub(crate) readers: Vec<Reader>,
    pub(crate) control: Control,
}
impl Fanout {
    /// Counts all owned PCM/tag arrays, queue/shared/Kernel Arc control blocks,
    /// vector storage, reader/producer/control state and one transaction. The
    /// platform allocator's internal bookkeeping is not an owned payload.
    /// This constructor allocates one Kernel; a future shared-Kernel constructor
    /// must charge it only once in the encompassing graph budget.
    pub(crate) fn required_bytes(routes: usize) -> Option<usize> {
        if routes == 0 {
            return None;
        }
        let arc_header = 2 * std::mem::size_of::<usize>();
        let queue = CAPACITY
            .checked_mul(2 * std::mem::size_of::<AtomicU64>())?
            .checked_add(std::mem::size_of::<Queue>())?
            .checked_add(arc_header)?;
        let route = queue
            .checked_add(std::mem::size_of::<Reader>())?
            .checked_add(std::mem::size_of::<Arc<Queue>>())?
            .checked_add(std::mem::size_of::<usize>())?;
        routes
            .checked_mul(route)?
            .checked_add(std::mem::size_of::<Self>())?
            .checked_add(std::mem::size_of::<Transaction<'static>>())?
            .checked_add(std::mem::size_of::<Shared>())?
            .checked_add(arc_header)?
            .checked_add(std::mem::size_of::<Kernel>())?
            .checked_add(arc_header)?
            .checked_add(Kernel::bytes())
    }
    pub(crate) fn build(
        routes: usize,
        sample_rate: u32,
        epoch: u64,
        budget: usize,
    ) -> Result<Self, Error> {
        if !(8000..=384000).contains(&sample_rate) {
            return Err(Error::InvalidConfiguration);
        }
        let bytes = Self::required_bytes(routes).ok_or(Error::InvalidConfiguration)?;
        if bytes > budget {
            return Err(Error::BudgetExceeded);
        }
        let shared = Arc::new(Shared {
            committed: AtomicU64::new(0),
            epoch,
        });
        let kernel = Kernel::new();
        let mut queues = Vec::with_capacity(routes);
        let mut readers = Vec::with_capacity(routes);
        for _ in 0..routes {
            let queue = Queue::new();
            readers.push(Reader {
                shared: shared.clone(),
                queue: queue.clone(),
                kernel: kernel.clone(),
                sample_rate: f64::from(sample_rate),
                position: 0,
                fraction: 0.0,
                ratio: 1.0,
                integral: 0.0,
                filtered_error: 0.0,
                target: 0,
                primed: false,
            });
            queues.push(queue);
        }
        Ok(Self {
            writer: Writer {
                shared: shared.clone(),
                queues,
                progress: vec![0; routes],
                cursor: 0,
                quantum: 0,
                version: 0,
            },
            readers,
            control: Control { shared },
        })
    }
}

/// May be cloned on the control thread. Cancellation is irreversible; restarts
/// need a new Fanout, including fresh queue bytes, ASRC and version history.
#[derive(Clone)]
pub(crate) struct Control {
    shared: Arc<Shared>,
}
impl Control {
    pub(crate) fn cancel(&self) {
        self.shared.committed.fetch_or(CANCELLED, Ordering::AcqRel);
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.shared.epoch
    }
    pub(crate) fn committed_frames(&self) -> u64 {
        self.shared.committed.load(Ordering::Acquire) & MAX_SOURCE_FRAMES
    }
    pub(crate) fn largest_committed_quantum(&self) -> usize {
        published_quantum(self.shared.committed.load(Ordering::Acquire))
    }
    pub(crate) fn cancelled(&self) -> bool {
        self.shared.committed.load(Ordering::Acquire) & CANCELLED != 0
    }
}

pub(crate) struct Writer {
    shared: Arc<Shared>,
    queues: Vec<Arc<Queue>>,
    progress: Vec<usize>,
    cursor: u64,
    quantum: usize,
    version: u64,
}
impl Writer {
    /// Must succeed before advancing graph/capture state for this callback.
    /// Every route reserves `frames`, even when its bus duplicates another.
    /// Error returns and dropped transactions leave all new frames invisible.
    pub(crate) fn begin(
        &mut self,
        frames: usize,
        accepted_version: u64,
    ) -> Result<Transaction<'_>, Error> {
        if self.shared.committed.load(Ordering::Acquire) & CANCELLED != 0 {
            return Err(Error::Cancelled);
        }
        if frames == 0 || frames > crate::MAX_CALLBACK_FRAMES {
            return Err(Error::InvalidBuffer);
        }
        if accepted_version < self.version {
            return Err(Error::InvalidVersion);
        }
        let end = self
            .cursor
            .checked_add(frames as u64)
            .filter(|end| *end <= MAX_SOURCE_FRAMES)
            .ok_or(Error::Overflow)?;
        for (route, queue) in self.queues.iter().enumerate() {
            let released = queue.released.load(Ordering::Acquire);
            if end
                .checked_sub(released)
                .is_none_or(|n| n > CAPACITY as u64)
            {
                return Err(Error::RouteFull { route });
            }
        }
        self.progress.fill(0);
        let quantum = self.quantum.max(frames);
        Ok(Transaction {
            writer: self,
            frames,
            end,
            quantum,
            version: accepted_version,
            failure: None,
        })
    }
}

pub(crate) struct Transaction<'a> {
    writer: &'a mut Writer,
    frames: usize,
    end: u64,
    quantum: usize,
    version: u64,
    failure: Option<Error>,
}
impl Transaction<'_> {
    /// Append one graph chunk to a route. The caller may interleave route writes
    /// by graph chunk; each route must receive exactly the reserved frame count.
    /// Even a caller that ignores this Result cannot commit a failed transaction.
    pub(crate) fn write(&mut self, route: usize, frames: &[[f32; 2]]) -> Result<(), Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = self.write_inner(route, frames);
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }
    fn write_inner(&mut self, route: usize, frames: &[[f32; 2]]) -> Result<(), Error> {
        if self.writer.shared.committed.load(Ordering::Acquire) & CANCELLED != 0 {
            return Err(Error::Cancelled);
        }
        let progress = self
            .writer
            .progress
            .get_mut(route)
            .ok_or(Error::InvalidRoute)?;
        if frames.len() > self.frames - *progress {
            return Err(Error::InvalidBuffer);
        }
        let queue = &self.writer.queues[route];
        for (offset, frame) in frames.iter().enumerate() {
            let index = ((self.writer.cursor as usize & (CAPACITY - 1)) + *progress + offset)
                & (CAPACITY - 1);
            let finite = |value: f32| if value.is_finite() { value } else { 0.0 };
            let bits = u64::from(finite(frame[0]).to_bits())
                | (u64::from(finite(frame[1]).to_bits()) << 32);
            queue.pcm[index].store(bits, Ordering::Relaxed);
            queue.versions[index].store(self.version, Ordering::Relaxed);
        }
        *progress += frames.len();
        Ok(())
    }
    pub(crate) fn commit(self) -> Result<u64, Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.writer.progress.iter().any(|&n| n != self.frames) {
            return Err(Error::IncompleteTransaction);
        }
        // Single producer: only cancellation may change this word. No retry.
        self.writer
            .shared
            .committed
            .compare_exchange(
                published_word(self.writer.cursor, self.writer.quantum),
                published_word(self.end, self.quantum),
                Ordering::Release,
                Ordering::Acquire,
            )
            .map_err(|_| Error::Cancelled)?;
        self.writer.cursor = self.end;
        self.writer.quantum = self.quantum;
        self.writer.version = self.version;
        Ok(self.end)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ReadReport {
    pub(crate) rendered: bool,
    pub(crate) source_start: u64,
    pub(crate) source_end: u64,
    pub(crate) source_fraction: f64,
    pub(crate) buffered_frames: u64,
    pub(crate) target_frames: usize,
    pub(crate) producer_quantum: usize,
    pub(crate) correction_ppm: f64,
    /// Minimum target version across every FIR-support sample of this whole
    /// callback. None for empty/priming output or any zero prehistory support.
    /// This does not acknowledge ramp completion, IIR tails or skipped requests.
    pub(crate) accepted_version: Option<u64>,
}
pub(crate) struct Reader {
    shared: Arc<Shared>,
    queue: Arc<Queue>,
    kernel: Arc<Kernel>,
    sample_rate: f64,
    position: u64,
    fraction: f64,
    ratio: f64,
    integral: f64,
    filtered_error: f64,
    target: usize,
    primed: bool,
}
impl Reader {
    /// Whole-callback preflight/read, with a final cancellation check before the
    /// caller may submit these planes. An already handed-off OS buffer remains
    /// outside this offline transport's cancellation boundary.
    pub(crate) fn read(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<ReadReport, Error> {
        left.fill(0.0);
        right.fill(0.0);
        if left.len() != right.len() || left.len() > crate::MAX_CALLBACK_FRAMES {
            return Err(Error::InvalidBuffer);
        }
        let word = self.shared.committed.load(Ordering::Acquire);
        if word & CANCELLED != 0 {
            return Err(Error::Cancelled);
        }
        let written = word & MAX_SOURCE_FRAMES;
        let available = written.checked_sub(self.position).ok_or(Error::Underflow)?;
        let frames = left.len();
        let mut report = ReadReport {
            rendered: false,
            source_start: self.position,
            source_end: self.position,
            source_fraction: self.fraction,
            buffered_frames: available,
            target_frames: self.target,
            producer_quantum: published_quantum(word),
            correction_ppm: (self.ratio - 1.0) * 1_000_000.0,
            accepted_version: None,
        };
        if frames == 0 {
            return Ok(report);
        }
        let half = Kernel::half();
        let required = (frames as f64 * (1.0 + MAX_CORRECTION)).ceil() as u64 + half as u64 + 1;
        let minimum = (2 * frames.max(report.producer_quantum) + Kernel::taps()).max(1024);
        let required_margin = minimum + half;
        if !self.primed {
            if available < required.max(required_margin as u64) {
                report.target_frames = minimum;
                return Ok(report);
            }
            // Preserve all queued program material, including a delayed first
            // callback. Its initial backlog becomes its bounded occupancy target.
            // No `written - target` cursor reset and no discarded source prefix.
            self.target = available as usize;
            self.primed = true;
        }
        if required_margin > self.target {
            // Changing the callback quantum cannot silently reset a playing
            // cursor or grow its target by discarding/duplicating program PCM.
            return Err(Error::QuantumExceeded {
                required_margin,
                startup_target: self.target,
            });
        }
        if available < required {
            return Err(Error::Underflow);
        }
        let dt = frames as f64 / self.sample_rate;
        let error = available as f64 - self.target as f64;
        self.filtered_error += (1.0 - (-dt / 0.5).exp()) * (error - self.filtered_error);
        self.integral = (self.integral + self.filtered_error / self.sample_rate * dt * 0.02)
            .clamp(-MAX_CORRECTION, MAX_CORRECTION);
        let correction = (self.integral + self.filtered_error / self.sample_rate * 0.1)
            .clamp(-MAX_CORRECTION, MAX_CORRECTION);
        self.ratio = 1.0 + correction;
        let mut minimum_version = u64::MAX;
        let mut prehistory = false;
        for (left, right) in left.iter_mut().zip(right.iter_mut()) {
            let (a, b, blend) = self.kernel.phase_rows(self.fraction);
            let mut pcm = [0.0; 2];
            for (tap, (&a, &b)) in a.iter().zip(b).enumerate() {
                let Some(source) = (self.position + tap as u64).checked_sub((half - 1) as u64)
                else {
                    prehistory = true;
                    continue; // Explicit silence and no control acknowledgment.
                };
                let (frame, version) = self.queue.frame(source);
                minimum_version = minimum_version.min(version);
                let coefficient = a + blend * (b - a);
                pcm[0] += frame[0] * coefficient;
                pcm[1] += frame[1] * coefficient;
            }
            *left = pcm[0];
            *right = pcm[1];
            self.fraction += self.ratio;
            let whole = self.fraction.floor();
            self.position += whole as u64;
            self.fraction -= whole;
        }
        if self.shared.committed.load(Ordering::Acquire) & CANCELLED != 0 {
            left.fill(0.0);
            right.fill(0.0);
            return Err(Error::Cancelled);
        }
        self.queue
            .released
            .store(self.position.saturating_sub(half as u64), Ordering::Release);
        report.rendered = true;
        report.source_end = self.position;
        report.source_fraction = self.fraction;
        report.target_frames = self.target;
        report.correction_ppm = correction * 1_000_000.0;
        report.accepted_version = (!prehistory).then_some(minimum_version);
        Ok(report)
    }
}

#[cfg(test)]
#[path = "fanout_tests.rs"]
mod tests;
