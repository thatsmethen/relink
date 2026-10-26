use crate::session::{db_to_gain, DspConfig};

pub(crate) type Frame = [f32; 2];

/// Bound corrupt finite samples as well as NaN/infinity before they can poison
/// filter state or meter accumulation. This is fault containment, not clipping
/// protection; ordinary samples above full scale remain measurable.
pub(crate) fn sanitize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1_000_000.0, 1_000_000.0)
    } else {
        0.0
    }
}

fn coefficient(ms: f32, sample_rate: u32) -> f32 {
    (-1.0 / (ms * 0.001 * sample_rate as f32)).exp()
}

fn flush_denormal(sample: f32) -> f32 {
    if sample.abs() < 1.0e-20 {
        0.0
    } else {
        sample
    }
}

/// Fixed-size built-in DSP coefficients prepared off the callback. The wire
/// representation contains no pointers, history or heap ownership. Decoding
/// checks finite, stable coefficient ranges without transcendentals/allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedDsp {
    words: [u32; Self::WORD_COUNT],
}
impl PreparedDsp {
    pub const WORD_COUNT: usize = 14;

    pub fn to_words(self) -> [u32; Self::WORD_COUNT] {
        self.words
    }
    pub fn sample_rate(&self) -> u32 {
        self.words[0]
    }
    pub fn from_words(words: [u32; Self::WORD_COUNT]) -> Result<Self, crate::ControlError> {
        let value = Self { words };
        if value.valid() {
            Ok(value)
        } else {
            Err(crate::ControlError::InvalidDsp)
        }
    }
    pub(crate) fn valid(&self) -> bool {
        if !(8000..=384000).contains(&self.sample_rate()) || self.words[1] > 15 {
            return false;
        }
        // Gate hold is an integer sample count. All other parameters are
        // finite scalar coefficients; convex interpolation stays in these ranges.
        if self.words[6] > self.sample_rate() * 5 {
            return false;
        }
        [
            (2, 0.0, 1.0),
            (3, 0.000001, 1.0),
            (4, 0.0, 1.0),
            (5, 0.0, 1.0),
            (7, 0.000001, 1.0),
            (8, 0.0, 0.99),
            (9, 0.0, 1.0),
            (10, 0.0, 1.0),
            (11, 0.063, 16.0),
            (12, 0.001, 1.0),
            (13, 0.0, 1.0),
        ]
        .into_iter()
        .all(|(index, min, max)| {
            let value = self.float(index);
            value.is_finite() && (min..=max).contains(&value)
        })
    }
    // The calling session has already validated user parameters and sample rate.
    pub(crate) fn compile(config: &DspConfig, sample_rate: u32) -> Self {
        let gate = config.gate.clone().unwrap_or_default();
        let compressor = config.compressor.clone().unwrap_or_default();
        let limiter = config.limiter.clone().unwrap_or_default();
        let mask = u32::from(config.high_pass.is_some() && !config.bypass.high_pass)
            | (u32::from(config.gate.is_some() && !config.bypass.gate) << 1)
            | (u32::from(config.compressor.is_some() && !config.bypass.compressor) << 2)
            | (u32::from(config.limiter.is_some() && !config.bypass.limiter) << 3);
        Self {
            words: [
                sample_rate,
                mask,
                config
                    .high_pass
                    .as_ref()
                    .map_or(1.0, |p| {
                        (-std::f32::consts::TAU * p.cutoff_hz / sample_rate as f32).exp()
                    })
                    .to_bits(),
                db_to_gain(gate.threshold_db).to_bits(),
                coefficient(gate.attack_ms, sample_rate).to_bits(),
                coefficient(gate.release_ms, sample_rate).to_bits(),
                (gate.hold_ms * 0.001 * sample_rate as f32).round() as u32,
                db_to_gain(compressor.threshold_db).to_bits(),
                (1.0 - 1.0 / compressor.ratio).to_bits(),
                coefficient(compressor.attack_ms, sample_rate).to_bits(),
                coefficient(compressor.release_ms, sample_rate).to_bits(),
                db_to_gain(compressor.makeup_gain_db).to_bits(),
                db_to_gain(limiter.ceiling_db).to_bits(),
                coefficient(limiter.release_ms, sample_rate).to_bits(),
            ],
        }
    }
    fn float(&self, index: usize) -> f32 {
        f32::from_bits(self.words[index])
    }
    fn enabled(&self, stage: usize) -> f32 {
        f32::from(self.words[1] & (1 << stage) != 0)
    }
    fn high_pass(&self) -> [f32; 1] {
        [self.float(2)]
    }
    fn gate(&self) -> [f32; 4] {
        [
            self.float(3),
            self.float(4),
            self.float(5),
            self.words[6] as f32,
        ]
    }
    fn compressor(&self) -> [f32; 5] {
        [
            self.float(7),
            self.float(8),
            self.float(9),
            self.float(10),
            self.float(11),
        ]
    }
    fn limiter(&self) -> [f32; 2] {
        [self.float(12), self.float(13)]
    }
}

struct Stage<P, const N: usize> {
    processor: P,
    coefficients: crate::smoothing::ControlRamp<N>,
    wet: crate::smoothing::ControlRamp<1>,
    last_wet: f32,
}
impl<P, const N: usize> Stage<P, N> {
    fn new(processor: P, coefficients: [f32; N], wet: f32) -> Self {
        Self {
            processor,
            coefficients: crate::smoothing::ControlRamp::new(coefficients),
            wet: crate::smoothing::ControlRamp::new([wet]),
            last_wet: wet,
        }
    }
    fn retarget(&mut self, coefficients: [f32; N], wet: f32, frames: u32) {
        // Disabling keeps the last parameter target while fading dry. Its
        // history/envelope is frozen only after this stage becomes fully dry.
        if wet != 0.0 {
            if self.last_wet == 0.0 {
                // At exact bypass no processed sample is audible. Use the new
                // parameters immediately, then fade in their output. Ramping
                // from unused default parameters could spuriously close a gate
                // or compress before reaching the user's requested setting.
                self.coefficients = crate::smoothing::ControlRamp::new(coefficients);
            } else {
                self.coefficients.retarget(coefficients, frames);
            }
        }
        self.wet.retarget([wet], frames);
    }
    fn next_wet(&mut self) -> f32 {
        self.last_wet = self.wet.next()[0];
        self.last_wet
    }
}
fn blend(dry: Frame, wet: Frame, amount: f32) -> Frame {
    if amount == 1.0 {
        wet // Preserve the existing settled processing bit-for-bit.
    } else {
        [
            dry[0] + (wet[0] - dry[0]) * amount,
            dry[1] + (wet[1] - dry[1]) * amount,
        ]
    }
}

pub(crate) struct Processor {
    prepared: PreparedDsp,
    high_pass: Stage<HighPass, 1>,
    gate: Stage<Gate, 4>,
    compressor: Stage<Compressor, 5>,
    limiter: Stage<Limiter, 2>,
}
impl Processor {
    pub(crate) fn new(config: &DspConfig, sample_rate: u32) -> Self {
        let p = PreparedDsp::compile(config, sample_rate);
        Self {
            prepared: p,
            high_pass: Stage::new(
                HighPass {
                    alpha: p.float(2),
                    previous_input: [0.0; 2],
                    previous_output: [0.0; 2],
                },
                p.high_pass(),
                p.enabled(0),
            ),
            gate: Stage::new(
                Gate {
                    threshold: p.float(3),
                    attack: p.float(4),
                    release: p.float(5),
                    hold_samples: p.words[6],
                    remaining_hold: 0,
                    gain: 0.0,
                },
                p.gate(),
                p.enabled(1),
            ),
            compressor: Stage::new(
                Compressor {
                    threshold: p.float(7),
                    exponent: p.float(8),
                    attack: p.float(9),
                    release: p.float(10),
                    makeup_gain: p.float(11),
                    gain: 1.0,
                },
                p.compressor(),
                p.enabled(2),
            ),
            limiter: Stage::new(
                Limiter {
                    ceiling: p.float(12),
                    release: p.float(13),
                    gain: 1.0,
                },
                p.limiter(),
                p.enabled(3),
            ),
        }
    }
    pub(crate) fn retarget(&mut self, p: PreparedDsp, frames: u32) {
        if p == self.prepared {
            return;
        }
        self.high_pass.retarget(p.high_pass(), p.enabled(0), frames);
        self.gate.retarget(p.gate(), p.enabled(1), frames);
        self.compressor
            .retarget(p.compressor(), p.enabled(2), frames);
        self.limiter.retarget(p.limiter(), p.enabled(3), frames);
        self.prepared = p;
    }
    pub(crate) fn process(&mut self, input: Frame) -> Frame {
        let mut frame = [sanitize(input[0]), sanitize(input[1])];
        let stage = &mut self.high_pass;
        let wet = stage.next_wet();
        if wet != 0.0 {
            [stage.processor.alpha] = stage.coefficients.next();
            frame = blend(frame, stage.processor.process(frame), wet);
        }
        let stage = &mut self.gate;
        let wet = stage.next_wet();
        if wet != 0.0 {
            let [threshold, attack, release, hold] = stage.coefficients.next();
            stage.processor.threshold = threshold;
            stage.processor.attack = attack;
            stage.processor.release = release;
            stage.processor.hold_samples = hold.round() as u32;
            frame = blend(frame, stage.processor.process(frame), wet);
        }
        let stage = &mut self.compressor;
        let wet = stage.next_wet();
        if wet != 0.0 {
            [
                stage.processor.threshold,
                stage.processor.exponent,
                stage.processor.attack,
                stage.processor.release,
                stage.processor.makeup_gain,
            ] = stage.coefficients.next();
            frame = blend(frame, stage.processor.process(frame), wet);
        }
        let stage = &mut self.limiter;
        let wet = stage.next_wet();
        if wet != 0.0 {
            [stage.processor.ceiling, stage.processor.release] = stage.coefficients.next();
            frame = blend(frame, stage.processor.process(frame), wet);
        }
        [
            flush_denormal(sanitize(frame[0])),
            flush_denormal(sanitize(frame[1])),
        ]
    }
}

struct HighPass {
    alpha: f32,
    previous_input: Frame,
    previous_output: Frame,
}

impl HighPass {
    fn process(&mut self, input: Frame) -> Frame {
        let output = [
            self.alpha * (self.previous_output[0] + input[0] - self.previous_input[0]),
            self.alpha * (self.previous_output[1] + input[1] - self.previous_input[1]),
        ];
        self.previous_input = input;
        self.previous_output = [flush_denormal(output[0]), flush_denormal(output[1])];
        self.previous_output
    }
}

struct Gate {
    threshold: f32,
    attack: f32,
    release: f32,
    hold_samples: u32,
    remaining_hold: u32,
    gain: f32,
}

impl Gate {
    fn process(&mut self, input: Frame) -> Frame {
        let above = input[0].abs().max(input[1].abs()) >= self.threshold;
        let open = if above {
            self.remaining_hold = self.hold_samples;
            true
        } else if self.remaining_hold > 0 {
            self.remaining_hold -= 1;
            true
        } else {
            false
        };
        let target = if open { 1.0 } else { 0.0 };
        let smoothing = if open { self.attack } else { self.release };
        self.gain = flush_denormal(target + smoothing * (self.gain - target));
        [input[0] * self.gain, input[1] * self.gain]
    }
}

struct Compressor {
    threshold: f32,
    exponent: f32,
    attack: f32,
    release: f32,
    makeup_gain: f32,
    gain: f32,
}

impl Compressor {
    fn process(&mut self, input: Frame) -> Frame {
        let peak = input[0].abs().max(input[1].abs());
        let target = if peak > self.threshold {
            (self.threshold / peak).powf(self.exponent)
        } else {
            1.0
        };
        let smoothing = if target < self.gain {
            self.attack
        } else {
            self.release
        };
        self.gain = target + smoothing * (self.gain - target);
        let gain = self.gain * self.makeup_gain;
        [input[0] * gain, input[1] * gain]
    }
}

struct Limiter {
    ceiling: f32,
    release: f32,
    gain: f32,
}

impl Limiter {
    fn process(&mut self, input: Frame) -> Frame {
        let peak = input[0].abs().max(input[1].abs());
        let required = if peak > self.ceiling {
            self.ceiling / peak
        } else {
            1.0
        };
        self.gain = if required < self.gain {
            required
        } else {
            (1.0 + self.release * (self.gain - 1.0)).min(required)
        };
        // Round-off at exactly the ceiling must not produce an over-ceiling
        // sample. This also covers the zero-lookahead first transient.
        [
            (input[0] * self.gain).clamp(-self.ceiling, self.ceiling),
            (input[1] * self.gain).clamp(-self.ceiling, self.ceiling),
        ]
    }
}

#[cfg(test)]
#[path = "dsp_tests.rs"]
mod tests;
