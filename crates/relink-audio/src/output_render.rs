//! One graph owner and independent follower readers. Native owners alone publish
//! submission acknowledgments; preparing PCM is a separate observation.
use crate::{
    fanout, AudioFault, FaultOrigin, OutputId, OutputPermit, OutputSnapshot, OutputTimingSnapshot,
    RenderOutcome, RenderState, RunningOutput, StereoSink, MAX_CALLBACK_FRAMES,
};
use relink_core::BusHandle;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

#[derive(Default)]
pub(crate) struct OutputTelemetry {
    rendered: AtomicU64,
    submitted: AtomicU64,
    rendered_version: AtomicU64,
    rendered_version_present: AtomicBool,
    submitted_version: AtomicU64,
    submitted_version_present: AtomicBool,
    pub(crate) ready: AtomicBool,
    source: AtomicU64,
    buffered: AtomicU64,
    target: AtomicU64,
    correction: AtomicU64,
}
impl OutputTelemetry {
    fn prepared(&self, frames: usize, version: Option<u64>) {
        self.rendered.fetch_add(frames as u64, Ordering::Relaxed);
        if let Some(version) = version {
            self.rendered_version.store(version, Ordering::Release);
            self.rendered_version_present.store(true, Ordering::Release);
        }
    }
    pub(crate) fn submitted(&self, frames: usize, version: Option<u64>) {
        self.submitted.fetch_add(frames as u64, Ordering::Relaxed);
        if let Some(version) = version {
            self.submitted_version.store(version, Ordering::Release);
            self.submitted_version_present
                .store(true, Ordering::Release);
        }
        self.ready.store(true, Ordering::Release);
    }
    pub(crate) fn snapshot(
        &self,
        route_index: usize,
        route: &RunningOutput,
        is_clock: bool,
        timing: OutputTimingSnapshot,
    ) -> OutputSnapshot {
        OutputSnapshot {
            route_index,
            output: OutputId(route.output.id.0.clone()),
            bus_id: route.bus_id,
            is_clock,
            ready: self.ready.load(Ordering::Acquire),
            rendered_frames: self.rendered.load(Ordering::Acquire),
            submitted_frames: self.submitted.load(Ordering::Acquire),
            rendered_control_version: self
                .rendered_version_present
                .load(Ordering::Acquire)
                .then(|| self.rendered_version.load(Ordering::Acquire)),
            submitted_control_version: self
                .submitted_version_present
                .load(Ordering::Acquire)
                .then(|| self.submitted_version.load(Ordering::Acquire)),
            source_position: self.source.load(Ordering::Acquire),
            buffered_frames: self.buffered.load(Ordering::Acquire),
            target_frames: self.target.load(Ordering::Acquire) as usize,
            clock_correction_ppm: f64::from_bits(self.correction.load(Ordering::Acquire)),
            timing,
        }
    }
}

pub(crate) struct FanoutRender {
    pub(crate) writer: fanout::Writer,
    /// Follower queue index maps to the original output-plan index and bus.
    pub(crate) routes: Vec<(usize, BusHandle)>,
}

// Keep the graph inline in the native context. Its payload is explicitly
// charged by owner_bytes; boxing it adds another owned allocation.
#[allow(clippy::large_enum_variant)]
pub(crate) enum OutputRenderer {
    Primary(RenderState),
    Follower(FollowerState),
}
impl From<RenderState> for OutputRenderer {
    fn from(mut value: RenderState) -> Self {
        value.output_permit = Some(value.permit());
        Self::Primary(value)
    }
}
impl OutputRenderer {
    pub(crate) fn permit(&self) -> OutputPermit {
        match self {
            Self::Primary(state) => state.permit(),
            Self::Follower(state) => state.permit.clone(),
        }
    }
    pub(crate) fn write_to(&mut self, sink: &mut impl StereoSink) -> RenderOutcome {
        // Owners also inspect this gate before work and at native handoff. This
        // check makes the portable dispatcher obey the identical startup rule.
        let permit = match self {
            Self::Primary(state) => state
                .output_permit
                .as_ref()
                .expect("installed primary permit"),
            Self::Follower(state) => &state.permit,
        };
        if !permit.program_authorized() {
            sink.clear();
            return RenderOutcome::Silence;
        }
        let outcome = match self {
            Self::Primary(state) => state.write_to(sink),
            Self::Follower(state) => state.write_to(sink),
        };
        if let RenderOutcome::Rendered {
            frames,
            accepted_version,
        } = outcome
        {
            match self {
                Self::Primary(state) => {
                    state
                        .output_permit
                        .as_ref()
                        .expect("installed primary permit")
                        .output_telemetry
                        .source
                        .store(
                            state.telemetry.frames.load(Ordering::Acquire),
                            Ordering::Release,
                        );
                    state
                        .output_permit
                        .as_ref()
                        .expect("installed primary permit")
                        .output_telemetry
                        .prepared(frames, accepted_version);
                }
                Self::Follower(state) => state
                    .permit
                    .output_telemetry
                    .prepared(frames, accepted_version),
            }
        }
        outcome
    }
}

pub(crate) struct FollowerState {
    reader: fanout::Reader,
    permit: OutputPermit,
    left: Vec<f32>,
    right: Vec<f32>,
}
impl FollowerState {
    pub(crate) fn new(reader: fanout::Reader, permit: OutputPermit) -> Self {
        Self {
            reader,
            permit,
            left: vec![0.0; MAX_CALLBACK_FRAMES],
            right: vec![0.0; MAX_CALLBACK_FRAMES],
        }
    }
    fn fail(&self, error: fanout::Error) {
        if error != fanout::Error::Cancelled {
            self.permit.fail_at(
                AudioFault::StreamInvalidated,
                FaultOrigin::OutputFanoutConsumer,
                self.permit.metadata(),
            );
        }
    }
    fn write_to(&mut self, sink: &mut impl StereoSink) -> RenderOutcome {
        sink.clear();
        if !self.permit.program_authorized() {
            return RenderOutcome::Silence;
        }
        let frames = match sink.frames() {
            Ok(frames) if frames <= MAX_CALLBACK_FRAMES => frames,
            _ => {
                self.permit.fail_at(
                    AudioFault::InvalidBuffer,
                    FaultOrigin::OutputBuffer,
                    self.permit.metadata(),
                );
                return RenderOutcome::Silence;
            }
        };
        let report = match self
            .reader
            .read(&mut self.left[..frames], &mut self.right[..frames])
        {
            Ok(report) => report,
            Err(error) => {
                self.fail(error);
                return RenderOutcome::Silence;
            }
        };
        let telemetry = &self.permit.output_telemetry;
        telemetry.source.store(report.source_end, Ordering::Release);
        telemetry
            .buffered
            .store(report.buffered_frames, Ordering::Release);
        telemetry
            .target
            .store(report.target_frames as u64, Ordering::Release);
        telemetry
            .correction
            .store(report.correction_ppm.to_bits(), Ordering::Release);
        if !report.rendered {
            return RenderOutcome::Silence;
        }
        if sink
            .write_stereo(0, &self.left[..frames], &self.right[..frames])
            .is_err()
        {
            self.permit.fail_at(
                AudioFault::InvalidBuffer,
                FaultOrigin::OutputBuffer,
                self.permit.metadata(),
            );
            sink.clear();
            return RenderOutcome::Silence;
        }
        if !self.permit.program_authorized() {
            sink.clear();
            return RenderOutcome::Silence;
        }
        RenderOutcome::Rendered {
            frames,
            accepted_version: report.accepted_version,
        }
    }
}

/// All route payloads are charged before constructing a mixer or any owner.
pub(crate) fn extra_bytes(
    configuration: &crate::RunningConfiguration,
    followers: usize,
) -> Result<usize, crate::AudioError> {
    let overflow =
        || crate::AudioError::InvalidSession("output route memory budget overflow".into());
    let fanout = if followers == 0 {
        0
    } else {
        fanout::Fanout::required_bytes(followers).ok_or_else(overflow)?
    };
    let per_follower = 2 * MAX_CALLBACK_FRAMES * std::mem::size_of::<f32>()
        + std::mem::size_of::<FollowerState>()
        + std::mem::size_of::<(usize, BusHandle)>();
    let mut bytes = followers
        .checked_mul(per_follower)
        .and_then(|n| n.checked_add(fanout))
        .ok_or_else(overflow)?;
    for route in &configuration.outputs {
        let strings = route
            .output
            .id
            .0
            .capacity()
            .checked_add(route.output.name.capacity())
            .and_then(|n| n.checked_add(route.sample_format.capacity()))
            .ok_or_else(overflow)?;
        let route_bytes = strings
            .checked_add(std::mem::size_of::<RunningOutput>())
            .and_then(|n| n.checked_mul(3))
            .and_then(|n| {
                n.checked_add(
                    std::mem::size_of::<OutputTelemetry>()
                        + 2 * std::mem::size_of::<usize>()
                        + 3 * std::mem::size_of::<OutputPermit>(),
                )
            })
            .ok_or_else(overflow)?;
        bytes = bytes.checked_add(route_bytes).ok_or_else(overflow)?;
    }
    bytes
        .checked_add(
            std::mem::size_of::<crate::EpochGate>() + std::mem::size_of::<FanoutRender>() + 1024,
        )
        .ok_or_else(overflow)
}

/// Shared construction path for real owners and offline dispatcher controls.
/// The caller has already validated the plan and reserved all payload bytes.
pub(crate) fn configure(
    render: &mut RenderState,
    plan: &crate::OutputPlan,
    clock_index: usize,
    gate: Arc<crate::EpochGate>,
    budget: usize,
) -> Result<Vec<FollowerState>, crate::AudioError> {
    let telemetry = Arc::get_mut(&mut render.telemetry).expect("unshared render construction");
    telemetry.gate = gate.clone();
    telemetry.outputs = (0..plan.routes.len())
        .map(|_| Arc::new(OutputTelemetry::default()))
        .collect();
    render.output_index = clock_index;
    let mut readers = Vec::new();
    if plan.routes.len() > 1 {
        let fanout = fanout::Fanout::build(
            plan.routes.len() - 1,
            render.sample_rate,
            render.epoch,
            budget,
        )
        .map_err(|e| crate::AudioError::InvalidSession(format!("output fanout: {e:?}")))?;
        let routes = plan
            .routes
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != clock_index)
            .map(|(index, route)| {
                render
                    .mixer
                    .resolve_bus(route.bus_id)
                    .map(|bus| (index, bus))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| crate::AudioError::InvalidSession(e.to_string()))?;
        gate.install_fanout(fanout.control);
        render.fanout = Some(FanoutRender {
            writer: fanout.writer,
            routes,
        });
        readers = fanout.readers;
    }
    let permit = render.permit();
    render.output_permit = Some(permit.clone());
    Ok(plan
        .routes
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != clock_index)
        .zip(readers)
        .map(|((index, _), reader)| {
            let mut permit = permit.clone();
            permit.route_index = index;
            permit.output_telemetry = render.telemetry.outputs[index].clone();
            FollowerState::new(reader, permit)
        })
        .collect())
}

#[cfg(test)]
#[path = "multioutput_tests.rs"]
mod tests;

/// A cheap lower bound prevents an enormous endpoint list from reaching device
/// discovery. Exact selected native owner costs are checked before allocation.
pub(crate) fn validate_plan_budget(plan: &crate::OutputPlan) -> Result<(), crate::AudioError> {
    let overflow = || {
        crate::AudioError::InvalidSession("output routes exceed the runtime memory budget".into())
    };
    let routes = plan.routes.len();
    let fanout = if routes <= 1 {
        0
    } else {
        fanout::Fanout::required_bytes(routes - 1).ok_or_else(overflow)?
    };
    let mut lower = routes
        .checked_mul(crate::output_progress::STACK_BYTES)
        .and_then(|n| n.checked_add(fanout))
        .ok_or_else(overflow)?;
    for route in &plan.routes {
        lower = lower
            .checked_add(route.output.0.len())
            .ok_or_else(overflow)?;
    }
    if lower >= crate::GRAPH_BYTES {
        return Err(overflow());
    }
    Ok(())
}
