/// A fixed-duration linear amplitude ramp. State is inline in the compiled
/// graph, so preparing, retargeting and advancing it never allocate.
pub(crate) struct ControlRamp<const N: usize> {
    current: [f32; N],
    start: [f32; N],
    target: [f32; N],
    elapsed: u32,
    duration: u32,
}

impl<const N: usize> ControlRamp<N> {
    /// Compilation uses the requested controls immediately, without a fade-in.
    pub(crate) fn new(value: [f32; N]) -> Self {
        Self {
            current: value,
            start: value,
            target: value,
            elapsed: 0,
            duration: 0,
        }
    }

    pub(crate) fn retarget(&mut self, target: [f32; N], frames: u32) {
        // Hosts can publish complete banks repeatedly while another control
        // changes. An unchanged target must not postpone this ramp's endpoint.
        if target == self.target {
            return;
        }
        self.start = self.current;
        self.target = target;
        self.elapsed = 0;
        self.duration = frames;
    }

    /// A completed zero ramp can be skipped without advancing its clock. An
    /// in-flight fade must still run, even if its target is already zero.
    pub(crate) fn is_stationary_zero(&self) -> bool {
        self.elapsed == self.duration && self.current == [0.0; N]
    }

    pub(crate) fn next(&mut self) -> [f32; N] {
        if self.elapsed == self.duration {
            return self.current;
        }
        self.elapsed += 1;
        if self.elapsed == self.duration {
            // Pin the endpoint, particularly zero, instead of accumulating
            // rounding error or leaving an asymptotic mute tail.
            self.current = self.target;
        } else {
            let progress = f64::from(self.elapsed) / f64::from(self.duration);
            for index in 0..N {
                self.current[index] = (f64::from(self.start[index])
                    + (f64::from(self.target[index]) - f64::from(self.start[index])) * progress)
                    as f32;
            }
        }
        self.current
    }
}
