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

pub(crate) struct Processor {
    high_pass: Option<HighPass>,
    gate: Option<Gate>,
    compressor: Option<Compressor>,
    limiter: Option<Limiter>,
}

impl Processor {
    pub(crate) fn new(config: &DspConfig, sample_rate: u32) -> Self {
        Self {
            high_pass: config.high_pass.as_ref().map(|p| HighPass {
                alpha: (-std::f32::consts::TAU * p.cutoff_hz / sample_rate as f32).exp(),
                previous_input: [0.0; 2],
                previous_output: [0.0; 2],
            }),
            gate: config.gate.as_ref().map(|p| Gate {
                threshold: db_to_gain(p.threshold_db),
                attack: coefficient(p.attack_ms, sample_rate),
                release: coefficient(p.release_ms, sample_rate),
                hold_samples: (p.hold_ms * 0.001 * sample_rate as f32).round() as u32,
                remaining_hold: 0,
                gain: 0.0,
            }),
            compressor: config.compressor.as_ref().map(|p| Compressor {
                threshold: db_to_gain(p.threshold_db),
                exponent: 1.0 - 1.0 / p.ratio,
                attack: coefficient(p.attack_ms, sample_rate),
                release: coefficient(p.release_ms, sample_rate),
                makeup_gain: db_to_gain(p.makeup_gain_db),
                gain: 1.0,
            }),
            limiter: config.limiter.as_ref().map(|p| Limiter {
                ceiling: db_to_gain(p.ceiling_db),
                release: coefficient(p.release_ms, sample_rate),
                gain: 1.0,
            }),
        }
    }

    pub(crate) fn process(&mut self, input: Frame) -> Frame {
        let mut frame = [sanitize(input[0]), sanitize(input[1])];
        if let Some(stage) = &mut self.high_pass {
            frame = stage.process(frame);
        }
        if let Some(stage) = &mut self.gate {
            frame = stage.process(frame);
        }
        if let Some(stage) = &mut self.compressor {
            frame = stage.process(frame);
        }
        if let Some(stage) = &mut self.limiter {
            frame = stage.process(frame);
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
