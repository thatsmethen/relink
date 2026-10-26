//! Platform-independent, integer-only stream timing and format rules.
pub const RATE: u64 = 48_000;
pub const CHANNELS: u32 = 2;
pub const BITS: u32 = 32;
pub const FRAME_BYTES: u32 = CHANNELS * BITS / 8;
pub const MIN_BUFFER_BYTES: u32 = FRAME_BYTES;
pub const MAX_BUFFER_BYTES: u32 = 65_536;
pub const DELAY_FRAMES: u64 = 512;
pub const CABLE_FRAMES: usize = 16_384;
pub const MAX_CLOCK_FREQUENCY: u64 = u64::MAX / RATE;

/// Split seconds and remainder before multiplying. This is integer-only and
/// avoids 128-bit division helpers that can use floating point on MSVC targets.
pub fn frames_from_ticks(ticks: u64, frequency: u64) -> u64 {
    if frequency == 0 || frequency > MAX_CLOCK_FREQUENCY {
        return 0;
    }
    (ticks / frequency)
        .saturating_mul(RATE)
        .saturating_add((ticks % frequency) * RATE / frequency)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum State {
    Stop = 0,
    Acquire = 1,
    Pause = 2,
    Run = 3,
}

#[derive(Clone, Copy, Debug)]
pub struct Timeline {
    pub state: State,
    pub frames: u64,
    pub run_ticks: u64,
    pub run_frames: u64,
}
impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}
impl Timeline {
    pub const fn new() -> Self {
        Self {
            state: State::Stop,
            frames: 0,
            run_ticks: 0,
            run_frames: 0,
        }
    }
    pub fn position(&self, now: u64, frequency: u64) -> u64 {
        if self.state != State::Run || frequency == 0 {
            return self.frames;
        }
        let elapsed = now.saturating_sub(self.run_ticks);
        self.run_frames
            .saturating_add(frames_from_ticks(elapsed, frequency))
    }
    pub fn transition(&mut self, state: State, now: u64, frequency: u64) {
        let frame = self.position(now, frequency);
        if state == State::Stop {
            *self = Self::new();
            return;
        }
        self.frames = frame;
        self.run_frames = frame;
        self.run_ticks = now;
        self.state = state;
    }
}

pub fn rounded_buffer_bytes(requested: u32) -> Option<u32> {
    if !(MIN_BUFFER_BYTES..=MAX_BUFFER_BYTES).contains(&requested) {
        return None;
    }
    let bytes = requested.checked_add(FRAME_BYTES - 1)? / FRAME_BYTES * FRAME_BYTES;
    (bytes <= MAX_BUFFER_BYTES).then_some(bytes)
}
pub fn byte_offset(frames: u64, buffer_bytes: u32) -> u32 {
    if buffer_bytes < FRAME_BYTES || !buffer_bytes.is_multiple_of(FRAME_BYTES) {
        return 0;
    }
    ((frames % u64::from(buffer_bytes / FRAME_BYTES)) * u64::from(FRAME_BYTES)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_does_not_accumulate_rounding_and_pauses_resume() {
        let mut timeline = Timeline::new();
        timeline.transition(State::Run, 100, 10_000_000);
        assert_eq!(timeline.position(10_000_100, 10_000_000), 48_000);
        timeline.transition(State::Pause, 10_000_100, 10_000_000);
        assert_eq!(timeline.position(99_000_100, 10_000_000), 48_000);
        timeline.transition(State::Run, 99_000_100, 10_000_000);
        assert_eq!(timeline.position(109_000_100, 10_000_000), 96_000);
        timeline.transition(State::Stop, 109_000_100, 10_000_000);
        assert_eq!(timeline.position(u64::MAX, 10_000_000), 0);
    }
    #[test]
    fn integer_clock_conversion_matches_wide_reference() {
        for frequency in [1, 32_768, 10_000_000, 3_579_545, MAX_CLOCK_FREQUENCY] {
            for ticks in [0, 1, 100, frequency, frequency + 1, u64::MAX] {
                let expected = (u128::from(ticks) * u128::from(RATE) / u128::from(frequency))
                    .min(u128::from(u64::MAX)) as u64;
                assert_eq!(frames_from_ticks(ticks, frequency), expected);
            }
        }
        assert_eq!(frames_from_ticks(1, 0), 0);
    }
    #[test]
    fn buffer_rounding_never_under_allocates() {
        for request in 8..=MAX_BUFFER_BYTES {
            let bytes = rounded_buffer_bytes(request).unwrap();
            assert!(bytes >= request && bytes.is_multiple_of(FRAME_BYTES));
        }
        assert_eq!(rounded_buffer_bytes(0), None);
        assert_eq!(rounded_buffer_bytes(MAX_BUFFER_BYTES + 1), None);
        assert_eq!(rounded_buffer_bytes(u32::MAX), None);
    }
    #[test]
    fn cursor_is_frame_aligned_and_wraps() {
        assert_eq!(byte_offset(0, 4096), 0);
        assert_eq!(byte_offset(511, 4096), 4088);
        assert_eq!(byte_offset(512, 4096), 0);
        assert_eq!(byte_offset(513, 4096), 8);
    }
}
