//! Fixed, independently recorded two-output native acceptance instrument.
//! No local-device mode: execution requires the disposable macOS CI guards.
//! The existing single-output probe and its frozen protocol remain unchanged.

#[cfg(target_os = "macos")]
mod native {
    use relink_audio::{
        AudioFault, AudioRuntime, AudioSnapshot, FaultAttribution, NativeOutputLifecyclePhase,
        NativeOutputOutcome, NativeOutputPacket, NativeOutputStamp, OutputId, OutputPlan,
        OutputRoute, OutputSnapshot, OutputTimingSnapshot,
    };
    use relink_core::{Bus, DspConfig, Input, InputKind, Send, Session, Uuid};
    use serde_json::{json, Value};
    use std::{
        env,
        fs::{self, File, OpenOptions},
        io::{BufWriter, Write},
        path::{Path, PathBuf},
        thread,
        time::{Duration, Instant},
    };

    const PROTOCOL: &str = "relink-native-multioutput-tones-v1";
    const OUTPUTS: [&str; 2] = [
        "coreaudio:au.textual.ravelink.cable.1",
        "coreaudio:au.textual.ravelink.cable.2",
    ];
    const FREQUENCIES: [f32; 2] = [997.0, 1543.0];
    const RATE: u32 = 48_000;
    const CLOCK: usize = 1;
    const POLL: Duration = Duration::from_millis(10);
    const ACK_WAIT: Duration = Duration::from_secs(2);
    const CLEANUP_WAIT: Duration = Duration::from_secs(2);
    // (gain A, mute A, gain B, mute B, hold seconds after both native ACKs).
    const PHASES: [(f32, bool, f32, bool, u64); 5] = [
        (-6.0, false, -12.0, false, 4),
        (-12.0, false, -6.0, false, 4),
        (-12.0, true, -6.0, false, 3),
        (-12.0, false, -6.0, true, 3),
        (-6.0, false, -12.0, false, 4),
    ];

    extern "C" {
        fn mach_absolute_time() -> u64;
    }
    fn ticks() -> u64 {
        unsafe { mach_absolute_time() }
    }
    fn fault_json(record: &FaultAttribution) -> Value {
        json!({
            "fault": record.fault as u32, "origin": format!("{:?}", record.origin),
            "direction": format!("{:?}", record.direction), "endpoint_index":record.endpoint_index,
            "endpoint_id":record.endpoint_id, "metadata_pending":record.metadata.is_none(),
            "metadata":record.metadata.map(|m| json!({
                "epoch":m.epoch, "observed_mach_host_time":m.observed_mach_host_time,
                "native_object":m.native_object, "status":m.status, "packet_index":m.packet_index,
                "property":m.property.map(|p| json!({"selector":p.selector,"scope":p.scope,"element":p.element})),
            }))
        })
    }
    fn output_stamp_json(stamp: NativeOutputStamp) -> Value {
        json!({
            "sample_time":stamp.sample_time, "sample_time_bits":stamp.sample_time.to_bits(),
            "host_time":stamp.host_time, "flags":stamp.flags,
            "rate_scalar":stamp.rate_scalar, "rate_scalar_bits":stamp.rate_scalar.to_bits(),
        })
    }
    fn output_packet_json(packet: NativeOutputPacket) -> Value {
        json!({
            "callback_index":packet.callback_index, "epoch":packet.epoch, "frames":packet.frames,
            "entry_host_time":packet.entry_host_time, "now":output_stamp_json(packet.now),
            "presentation":output_stamp_json(packet.presentation),
        })
    }
    fn output_timing_json(timing: &OutputTimingSnapshot) -> Value {
        let OutputTimingSnapshot::CoreAudio(timing) = timing;
        let lifecycle: Vec<_> = timing
            .lifecycle
            .iter()
            .flatten()
            .map(|event| {
                json!({
                    "phase":format!("{:?}",event.phase), "host_time":event.host_time,
                    "status":event.status, "callbacks_in_flight":event.callbacks_in_flight,
                })
            })
            .collect();
        let completion = timing.last_completion.map(|completion| {
            let outcome = match completion.outcome {
                NativeOutputOutcome::InactiveSilence => json!({"kind":"inactive_silence"}),
                NativeOutputOutcome::PrimingSilence => json!({"kind":"priming_silence"}),
                NativeOutputOutcome::Encoded { accepted_version } => {
                    json!({"kind":"encoded", "accepted_version":accepted_version})
                }
                NativeOutputOutcome::Cancelled => json!({"kind":"cancelled"}),
                NativeOutputOutcome::Rejected(fault) => {
                    json!({"kind":"rejected", "fault":fault as u32})
                }
            };
            json!({"packet":output_packet_json(completion.packet),
                "after_render_host_time":completion.after_render_host_time,
                "completion_host_time":completion.completion_host_time, "outcome":outcome})
        });
        json!({"backend":"coreaudio", "callbacks_seen":timing.callbacks_seen,
            "progress_count":timing.progress_count, "last_packet":timing.last_packet.map(output_packet_json),
            "last_packet_contended":timing.last_packet_contended,
            "first_fault":timing.first_fault.map(|fault| json!({"fault":fault.fault as u32,
                "reason":format!("{:?}",fault.reason), "previous":fault.previous.map(output_packet_json),
                "current":output_packet_json(fault.current)})),
            "first_fault_pending":timing.first_fault_pending, "last_completion":completion,
            "completion_contended":timing.completion_contended, "lifecycle":lifecycle})
    }

    fn exact_routes(snapshot: &AudioSnapshot) -> bool {
        snapshot.outputs.len() == OUTPUTS.len()
            && snapshot.outputs.iter().enumerate().all(|(index, route)| {
                route.route_index == index
                    && route.output.0 == OUTPUTS[index]
                    && route.is_clock == (index == CLOCK)
            })
    }
    fn route_shutdown_observed(route: &OutputSnapshot) -> bool {
        let OutputTimingSnapshot::CoreAudio(timing) = &route.timing;
        timing.lifecycle[NativeOutputLifecyclePhase::OwnerCompleted as usize].is_some()
            || timing.lifecycle[NativeOutputLifecyclePhase::ContextRetained as usize].is_some()
    }
    fn route_shutdown_proven(route: &OutputSnapshot) -> bool {
        let OutputTimingSnapshot::CoreAudio(timing) = &route.timing;
        let event = |phase: NativeOutputLifecyclePhase| {
            timing.lifecycle[phase as usize].filter(|event| event.phase == phase)
        };
        timing.first_fault.is_none()
            && !timing.first_fault_pending
            && timing.lifecycle[NativeOutputLifecyclePhase::ContextRetained as usize].is_none()
            && [
                NativeOutputLifecyclePhase::CreateReturned,
                NativeOutputLifecyclePhase::StartReturned,
                NativeOutputLifecyclePhase::StopReturned,
                NativeOutputLifecyclePhase::DestroyReturned,
                NativeOutputLifecyclePhase::ListenerRemovalReturned,
            ]
            .into_iter()
            .all(|phase| event(phase).is_some_and(|event| event.status == Some(0)))
            && [
                NativeOutputLifecyclePhase::QuiescenceChecked,
                NativeOutputLifecyclePhase::OwnerCompleted,
            ]
            .into_iter()
            .all(|phase| event(phase).is_some_and(|event| event.callbacks_in_flight == Some(0)))
            && [
                NativeOutputLifecyclePhase::StopReturned,
                NativeOutputLifecyclePhase::DestroyReturned,
                NativeOutputLifecyclePhase::ListenerRemovalReturned,
                NativeOutputLifecyclePhase::QuiescenceChecked,
                NativeOutputLifecyclePhase::OwnerCompleted,
            ]
            .into_iter()
            .all(|phase| event(phase).is_some_and(|event| event.host_time > 0))
    }
    fn snapshot_fault(snapshot: &AudioSnapshot) -> Option<String> {
        if snapshot.fault != AudioFault::None
            || snapshot.first_fault.is_some()
            || snapshot.error.is_some()
        {
            return Some(format!(
                "Runtime fault: {:?}; attribution={:?}; error={:?}",
                snapshot.fault, snapshot.first_fault, snapshot.error
            ));
        }
        for route in &snapshot.outputs {
            let OutputTimingSnapshot::CoreAudio(timing) = &route.timing;
            if timing.first_fault.is_some() || timing.first_fault_pending {
                return Some(format!(
                    "Route {} native fault: {:?}; pending={}",
                    route.route_index, timing.first_fault, timing.first_fault_pending
                ));
            }
        }
        None
    }
    fn shutdown_failed(snapshot: &AudioSnapshot) -> bool {
        snapshot_fault(snapshot).is_some()
            || snapshot.outputs.iter().any(|route| {
                let OutputTimingSnapshot::CoreAudio(timing) = &route.timing;
                timing.lifecycle[NativeOutputLifecyclePhase::ContextRetained as usize].is_some()
                    || [
                        NativeOutputLifecyclePhase::CreateReturned,
                        NativeOutputLifecyclePhase::StartReturned,
                        NativeOutputLifecyclePhase::StopReturned,
                        NativeOutputLifecyclePhase::DestroyReturned,
                        NativeOutputLifecyclePhase::ListenerRemovalReturned,
                    ]
                    .into_iter()
                    .any(|phase| {
                        timing.lifecycle[phase as usize]
                            .is_some_and(|event| event.status.is_some_and(|status| status != 0))
                    })
                    || [
                        NativeOutputLifecyclePhase::QuiescenceChecked,
                        NativeOutputLifecyclePhase::OwnerCompleted,
                    ]
                    .into_iter()
                    .any(|phase| {
                        timing.lifecycle[phase as usize].is_some_and(|event| {
                            event.callbacks_in_flight.is_some_and(|count| count != 0)
                        })
                    })
            })
    }
    fn shutdown_proven(snapshot: &AudioSnapshot) -> bool {
        !snapshot.running
            && snapshot_fault(snapshot).is_none()
            && exact_routes(snapshot)
            && snapshot.outputs.iter().all(route_shutdown_proven)
    }
    fn route_json(route: &OutputSnapshot) -> Value {
        json!({"route_index":route.route_index,"output_id":route.output.0,
            "bus_id":route.bus_id,"is_clock":route.is_clock,"ready":route.ready,
            "rendered_frames":route.rendered_frames,"submitted_frames":route.submitted_frames,
            "rendered_control_version":route.rendered_control_version,
            "submitted_control_version":route.submitted_control_version,
            "source_position":route.source_position,"buffered_frames":route.buffered_frames,
            "target_frames":route.target_frames,"clock_correction_ppm":route.clock_correction_ppm,
            "native_timing":output_timing_json(&route.timing),
            "shutdown_observed":route_shutdown_observed(route),"shutdown_proven":route_shutdown_proven(route)})
    }
    fn write_new(path: &Path, value: &Value) -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        writeln!(file, "{value}").map_err(|e| e.to_string())
    }
    struct Evidence {
        events: BufWriter<File>,
        telemetry: BufWriter<File>,
        shutdown_fault_observed: bool,
        outputs_shutdown_proven: [bool; 2],
    }
    impl Evidence {
        fn new(directory: &Path) -> Result<Self, String> {
            let create = |name| {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(directory.join(name))
                    .map_err(|e| e.to_string())
            };
            Ok(Self {
                events: BufWriter::new(create("multioutput-events.jsonl")?),
                telemetry: BufWriter::new(create("multioutput-telemetry.jsonl")?),
                shutdown_fault_observed: false,
                outputs_shutdown_proven: [false; 2],
            })
        }
        fn event(&mut self, value: Value) -> Result<(), String> {
            writeln!(self.events, "{value}")
                .and_then(|_| self.events.flush())
                .map_err(|e| e.to_string())
        }
        fn record(
            &mut self,
            snapshot: &AudioSnapshot,
            observation: &str,
            host_time: u64,
        ) -> Result<(), String> {
            let value = json!({"schema":1,"protocol":PROTOCOL,"observation":observation,
                "observed_host_time":host_time,"running":snapshot.running,"fault":snapshot.fault as u32,
                "first_fault":snapshot.first_fault.as_ref().map(fault_json),
                "error":snapshot.error.as_ref().map(ToString::to_string),
                "rendered_frames":snapshot.rendered_frames,"callback_count":snapshot.callback_count,
                "applied_control_version":snapshot.applied_control_version,
                "outputs":snapshot.outputs.iter().map(route_json).collect::<Vec<_>>(),
                "native_outputs_shutdown_proven":!self.shutdown_fault_observed && shutdown_proven(snapshot),
                "shutdown_fault_observed":self.shutdown_fault_observed});
            writeln!(self.telemetry, "{value}")
                .and_then(|_| self.telemetry.flush())
                .map_err(|e| e.to_string())
        }
        fn active(&mut self, runtime: &AudioRuntime) -> Result<(AudioSnapshot, u64), String> {
            let snapshot = runtime.snapshot();
            let observed = ticks();
            self.record(&snapshot, "running", observed)?;
            if let Some(error) = snapshot_fault(&snapshot) {
                return Err(error);
            }
            if !snapshot.running || !exact_routes(&snapshot) || !snapshot.captures.is_empty() {
                return Err(
                    "Runtime is not running the two exact output routes without capture".into(),
                );
            }
            Ok((snapshot, observed))
        }
        fn observe_shutdown(&mut self, runtime: &AudioRuntime) -> Result<(), String> {
            let deadline = Instant::now() + CLEANUP_WAIT;
            self.observe_shutdown_with(
                || runtime.snapshot(),
                || Instant::now() >= deadline,
                || thread::sleep(POLL),
            )
        }
        fn observe_shutdown_with(
            &mut self,
            mut next: impl FnMut() -> AudioSnapshot,
            mut expired: impl FnMut() -> bool,
            mut poll: impl FnMut(),
        ) -> Result<(), String> {
            let mut first_error = None;
            loop {
                let snapshot = next();
                if first_error.is_none() {
                    first_error = snapshot_fault(&snapshot).or_else(|| {
                        shutdown_failed(&snapshot).then(|| {
                            "A native output reported failed cleanup or retained context".into()
                        })
                    });
                }
                self.shutdown_fault_observed |= first_error.is_some();
                let proven = shutdown_proven(&snapshot);
                let observed =
                    exact_routes(&snapshot) && snapshot.outputs.iter().all(route_shutdown_observed);
                let final_observation =
                    (observed && (proven || shutdown_failed(&snapshot))) || expired();
                self.record(
                    &snapshot,
                    if final_observation {
                        "post-stop-final"
                    } else {
                        "post-stop"
                    },
                    ticks(),
                )?;
                if final_observation {
                    if exact_routes(&snapshot) && !snapshot.running && !self.shutdown_fault_observed
                    {
                        self.outputs_shutdown_proven = std::array::from_fn(|index| {
                            route_shutdown_proven(&snapshot.outputs[index])
                        });
                    }
                    return if let Some(error) = first_error {
                        Err(error)
                    } else if proven {
                        Ok(())
                    } else {
                        Err("Both native output owners did not prove successful cleanup and zero callbacks in flight".into())
                    };
                }
                poll();
            }
        }
    }
    fn acknowledge(
        snapshot: &AudioSnapshot,
        version: u64,
        observed: u64,
        first: &mut [Option<u64>; 2],
    ) -> Result<bool, String> {
        if !exact_routes(snapshot) {
            return Err("Output route identity/order changed during ACK".into());
        }
        if snapshot.applied_control_version > version {
            return Err("Graph version overtook fixed phase".into());
        }
        for (index, route) in snapshot.outputs.iter().enumerate() {
            if route.submitted_control_version.is_some_and(|v| v > version) {
                return Err(format!(
                    "Route {index} submitted version overtook fixed phase"
                ));
            }
            if route.ready && route.submitted_control_version == Some(version) {
                first[index].get_or_insert(observed);
            }
        }
        Ok(snapshot.applied_control_version == version && first.iter().all(Option::is_some))
    }
    fn build_session() -> Result<(Session, [Uuid; 2]), String> {
        let mut session = Session::new("Installed two-output tone acceptance", RATE);
        let mut buses = Vec::new();
        for (index, frequency_hz) in FREQUENCIES.into_iter().enumerate() {
            let input = session
                .add_input(Input::new(
                    format!("Route {index} tone"),
                    InputKind::TestTone {
                        frequency_hz,
                        amplitude_db: -18.0,
                    },
                ))
                .map_err(|e| e.to_string())?;
            let mut bus = Bus::new(format!("Route {index} measured mix"));
            bus.dsp = DspConfig::default();
            let bus = session.add_bus(bus).map_err(|e| e.to_string())?;
            let mut send = Send::new(input, bus);
            send.controls
                .set_gain_db(if index == 0 { -6.0 } else { -12.0 });
            session.add_send(send).map_err(|e| e.to_string())?;
            buses.push(bus);
        }
        Ok((session, [buses[0], buses[1]]))
    }
    fn execute_active(
        runtime: &AudioRuntime,
        directory: &Path,
        evidence: &mut Evidence,
    ) -> Result<(), String> {
        let discovered = runtime
            .enumerate_output_devices()
            .map_err(|e| e.to_string())?;
        for id in OUTPUTS {
            if discovered.iter().filter(|device| device.id.0 == id).count() != 1 {
                return Err(format!(
                    "Explicit output {id} absent or ambiguous; no default substitution"
                ));
            }
        }
        let (mut session, buses) = build_session()?;
        let plan = OutputPlan {
            clock_output: OutputId(OUTPUTS[CLOCK].into()),
            routes: OUTPUTS
                .iter()
                .zip(buses)
                .map(|(id, bus_id)| OutputRoute {
                    output: OutputId((*id).into()),
                    bus_id,
                })
                .collect(),
        };
        let configuration = runtime
            .start_outputs(session.clone(), plan)
            .map_err(|e| e.to_string())?;
        if configuration.clock_output_index != CLOCK
            || configuration.outputs.len() != 2
            || !configuration.inputs.is_empty()
            || configuration
                .outputs
                .iter()
                .enumerate()
                .any(|(index, route)| {
                    route.output.id.0 != OUTPUTS[index]
                        || route.bus_id != buses[index]
                        || route.sample_rate != RATE
                        || route.device_channels != 2
                })
        {
            return Err(format!(
                "Unexpected native configuration: {configuration:?}"
            ));
        }
        let routes:Vec<_>=configuration.outputs.iter().enumerate().map(|(index,route)|json!({
            "route_index":index,"output_id":route.output.id.0,"bus_id":route.bus_id,
            "frequency_hz":FREQUENCIES[index],"tone_amplitude_db":-18.0,"is_clock":index==CLOCK,
            "sample_rate":route.sample_rate,"device_channels":route.device_channels,"sample_format":route.sample_format})).collect();
        evidence.event(
            json!({"schema":1,"protocol":PROTOCOL,"event":"started","host_time":ticks(),
            "sample_rate":RATE,"clock_output_index":CLOCK,"routes":routes}),
        )?;
        let ready_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (snapshot, observed) = evidence.active(runtime)?;
            if snapshot.outputs.iter().all(|route| {
                route.ready
                    && route.submitted_frames >= 4096
                    && route.submitted_control_version == Some(0)
            }) {
                let temporary = directory.join("multioutput-ready.pending");
                write_new(
                    &temporary,
                    &json!({"schema":1,"protocol":PROTOCOL,"ready":true,
                    "observed_host_time":observed,"outputs":snapshot.outputs.iter().map(|route|json!({
                        "route_index":route.route_index,"output_id":route.output.0,"bus_id":route.bus_id,
                        "is_clock":route.is_clock,"submitted_frames":route.submitted_frames,
                        "submitted_control_version":route.submitted_control_version})).collect::<Vec<_>>()}),
                )?;
                fs::hard_link(&temporary, directory.join("multioutput-ready.json"))
                    .map_err(|e| e.to_string())?;
                fs::remove_file(temporary).map_err(|e| e.to_string())?;
                break;
            }
            if Instant::now() >= ready_deadline {
                return Err("Both native outputs did not become ready within five seconds".into());
            }
            thread::sleep(POLL);
        }
        let go_deadline = Instant::now() + Duration::from_secs(15);
        while !directory.join("multioutput.go").exists() {
            evidence.active(runtime)?;
            if Instant::now() >= go_deadline {
                return Err("Independent recorders did not arm phase sequence".into());
            }
            thread::sleep(POLL);
        }
        for (phase, (gain_a, mute_a, gain_b, mute_b, seconds)) in PHASES.into_iter().enumerate() {
            let gains = [gain_a, gain_b];
            let muted = [mute_a, mute_b];
            for index in 0..2 {
                session.sends[index].controls.set_gain_db(gains[index]);
                session.sends[index].controls.muted = muted[index];
            }
            let requested = ticks();
            evidence.event(
                json!({"schema":1,"protocol":PROTOCOL,"event":"phase-request","phase":phase,
                "requested_host_time":requested}),
            )?;
            let version = runtime
                .update_controls(session.clone())
                .map_err(|e| e.to_string())?;
            let deadline = Instant::now() + ACK_WAIT;
            let mut first = [None; 2];
            let (snapshot, recorded) = loop {
                let (snapshot, observed) = evidence.active(runtime)?;
                if acknowledge(&snapshot, version, observed, &mut first)? {
                    break (snapshot, observed);
                }
                if Instant::now() >= deadline {
                    return Err(
                        "Both native outputs did not acknowledge phase within two seconds".into(),
                    );
                }
                thread::sleep(Duration::from_millis(5));
            };
            // A snapshot reads aggregate and per-route atomics separately. If
            // the aggregate watermark catches up in a later observation, keep
            // the exact first native-route ACK times rather than inventing a
            // later combined submission timestamp.
            let acknowledged = first
                .iter()
                .flatten()
                .copied()
                .max()
                .expect("both route ACKs");
            evidence.event(json!({"schema":1,"protocol":PROTOCOL,"event":"phase","phase":phase,
                "requested_host_time":requested,"acknowledged_host_time":acknowledged,"version":version,
                "acknowledgment_recorded_host_time":recorded,
                "hold_seconds":seconds,"rendered_frames":snapshot.rendered_frames,"callback_count":snapshot.callback_count,
                "routes":(0..2).map(|index|json!({"route_index":index,"output_id":OUTPUTS[index],
                    "frequency_hz":FREQUENCIES[index],"gain_db":gains[index],"muted":muted[index],
                    "submitted_version":snapshot.outputs[index].submitted_control_version,
                    "submitted_observed_host_time":first[index]})).collect::<Vec<_>>()}))?;
            let end = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < end {
                evidence.active(runtime)?;
                thread::sleep(Duration::from_millis(20));
            }
        }
        let (snapshot, _) = evidence.active(runtime)?;
        let requested = ticks();
        runtime.stop().map_err(|e| e.to_string())?;
        evidence.event(json!({"schema":1,"protocol":PROTOCOL,"event":"stopped",
            "requested_host_time":requested,"returned_host_time":ticks(),
            "rendered_frames":snapshot.rendered_frames,"callback_count":snapshot.callback_count}))?;
        Ok(())
    }
    fn execute(directory: &Path, evidence: &mut Evidence) -> Result<(), String> {
        let runtime = AudioRuntime::new().map_err(|e| e.to_string())?;
        let result = execute_active(&runtime, directory, evidence);
        if result.is_err() {
            runtime.cancel_output();
            let _ = runtime.stop();
        }
        let cleanup = evidence.observe_shutdown(&runtime);
        result.and(cleanup)
    }
    pub fn main() -> Result<(), String> {
        if env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
            || env::var("RUNNER_OS").as_deref() != Ok("macOS")
            || env::var("RAVELINK_MULTIOUTPUT_ACCEPTANCE").as_deref() != Ok("1")
        {
            return Err(
                "This audio probe runs only in the explicitly enabled disposable macOS CI protocol"
                    .into(),
            );
        }
        let mut directory = None;
        let mut outputs = [None, None];
        let mut args = env::args().skip(1);
        while let Some(key) = args.next() {
            let value = args.next().ok_or("Missing option value")?;
            match key.as_str() {
                "--directory" => directory = Some(PathBuf::from(value)),
                "--output-a-id" => outputs[0] = Some(value),
                "--output-b-id" => outputs[1] = Some(value),
                _ => return Err(format!("Unknown option {key}")),
            }
        }
        for index in 0..2 {
            if outputs[index].as_deref() != Some(OUTPUTS[index]) {
                return Err(format!(
                    "Fixed protocol requires route {index} explicit ID {}",
                    OUTPUTS[index]
                ));
            }
        }
        let directory = directory.ok_or("Missing --directory")?;
        let mut evidence = Evidence::new(&directory)?;
        let result = execute(&directory, &mut evidence);
        write_new(
            &directory.join("multioutput-result.json"),
            &json!({"schema":1,"protocol":PROTOCOL,
            "pass":result.is_ok(),"native_outputs_shutdown_proven":evidence.outputs_shutdown_proven.iter().all(|v|*v)
                &&!evidence.shutdown_fault_observed,
            "outputs_shutdown_proven":evidence.outputs_shutdown_proven,
            "shutdown_fault_observed":evidence.shutdown_fault_observed,"error":result.as_ref().err()}),
        )?;
        result
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use relink_audio::{
            NativeOutputLifecycleEvent, NativeOutputTimingSnapshot, NATIVE_OUTPUT_LIFECYCLE_PHASES,
        };

        fn snapshot() -> AudioSnapshot {
            AudioSnapshot {
                outputs: (0..2)
                    .map(|index| OutputSnapshot {
                        route_index: index,
                        output: OutputId(OUTPUTS[index].into()),
                        bus_id: Uuid::new_v4(),
                        is_clock: index == CLOCK,
                        ready: true,
                        rendered_frames: 4096,
                        submitted_frames: 4096,
                        rendered_control_version: Some(2),
                        submitted_control_version: Some(2),
                        source_position: 4096,
                        buffered_frames: 1024,
                        target_frames: 1024,
                        clock_correction_ppm: 0.0,
                        timing: OutputTimingSnapshot::CoreAudio(NativeOutputTimingSnapshot {
                            callbacks_seen: 8,
                            progress_count: 8,
                            last_packet: None,
                            last_packet_contended: false,
                            first_fault: None,
                            first_fault_pending: false,
                            last_completion: None,
                            completion_contended: false,
                            lifecycle: [None; NATIVE_OUTPUT_LIFECYCLE_PHASES],
                        }),
                    })
                    .collect(),
                running: false,
                configuration: None,
                rendered_frames: 4096,
                callback_count: 8,
                applied_control_version: 2,
                captures: Vec::new(),
                output_timing: None,
                input_meters: Vec::new(),
                bus_meters: Vec::new(),
                fault: AudioFault::None,
                first_fault: None,
                error: None,
            }
        }
        fn complete(snapshot: &mut AudioSnapshot) {
            for route in &mut snapshot.outputs {
                let OutputTimingSnapshot::CoreAudio(timing) = &mut route.timing;
                for phase in [
                    NativeOutputLifecyclePhase::CreateReturned,
                    NativeOutputLifecyclePhase::StartReturned,
                    NativeOutputLifecyclePhase::StopReturned,
                    NativeOutputLifecyclePhase::DestroyReturned,
                    NativeOutputLifecyclePhase::ListenerRemovalReturned,
                    NativeOutputLifecyclePhase::QuiescenceChecked,
                    NativeOutputLifecyclePhase::OwnerCompleted,
                ] {
                    timing.lifecycle[phase as usize] = Some(NativeOutputLifecycleEvent {
                        phase,
                        host_time: 100 + phase as u64,
                        status: Some(0),
                        callbacks_in_flight: Some(0),
                    });
                }
            }
        }
        fn timing(snapshot: &mut AudioSnapshot, index: usize) -> &mut NativeOutputTimingSnapshot {
            let OutputTimingSnapshot::CoreAudio(timing) = &mut snapshot.outputs[index].timing;
            timing
        }
        #[test]
        fn acknowledgments_require_both_native_routes_not_graph_or_preparation() {
            let mut snapshot = snapshot();
            let mut seen = [None; 2];
            snapshot.outputs[0].submitted_control_version = None;
            assert!(!acknowledge(&snapshot, 2, 100, &mut seen).unwrap());
            assert_eq!(seen, [None, Some(100)]);
            snapshot.outputs[0].submitted_control_version = Some(1);
            assert!(!acknowledge(&snapshot, 2, 200, &mut seen).unwrap());
            snapshot.outputs[0].submitted_control_version = Some(2);
            assert!(acknowledge(&snapshot, 2, 300, &mut seen).unwrap());
            assert_eq!(seen, [Some(300), Some(100)]);
        }
        #[test]
        fn acknowledgment_rejects_overtaking_and_changed_route_identity() {
            let mut snapshot = snapshot();
            snapshot.outputs[0].submitted_control_version = Some(3);
            assert!(acknowledge(&snapshot, 2, 100, &mut [None; 2]).is_err());
            snapshot.outputs[0].submitted_control_version = Some(2);
            snapshot.outputs.swap(0, 1);
            assert!(acknowledge(&snapshot, 2, 100, &mut [None; 2]).is_err());
        }
        #[test]
        fn later_aggregate_observation_preserves_first_native_ack_timestamps() {
            let mut snapshot = snapshot();
            let mut seen = [None; 2];
            snapshot.applied_control_version = 1;
            assert!(!acknowledge(&snapshot, 2, 100, &mut seen).unwrap());
            assert_eq!(seen, [Some(100), Some(100)]);
            snapshot.applied_control_version = 2;
            assert!(acknowledge(&snapshot, 2, 200, &mut seen).unwrap());
            assert_eq!(seen.iter().flatten().copied().max(), Some(100));
        }
        #[test]
        fn strict_cleanup_requires_every_successful_operation_for_each_owner() {
            let mut valid = snapshot();
            complete(&mut valid);
            assert!(shutdown_proven(&valid));
            for index in 0..2 {
                for phase in [
                    NativeOutputLifecyclePhase::CreateReturned,
                    NativeOutputLifecyclePhase::StartReturned,
                    NativeOutputLifecyclePhase::StopReturned,
                    NativeOutputLifecyclePhase::DestroyReturned,
                    NativeOutputLifecyclePhase::ListenerRemovalReturned,
                    NativeOutputLifecyclePhase::QuiescenceChecked,
                    NativeOutputLifecyclePhase::OwnerCompleted,
                ] {
                    let mut missing = valid.clone();
                    timing(&mut missing, index).lifecycle[phase as usize] = None;
                    assert!(!shutdown_proven(&missing));
                    let mut failed = valid.clone();
                    let event = timing(&mut failed, index).lifecycle[phase as usize]
                        .as_mut()
                        .unwrap();
                    event.status = Some(-1);
                    event.callbacks_in_flight = Some(1);
                    assert!(!shutdown_proven(&failed));
                    assert!(shutdown_failed(&failed));
                }
                let mut retained = valid.clone();
                let phase = NativeOutputLifecyclePhase::ContextRetained;
                timing(&mut retained, index).lifecycle[phase as usize] =
                    Some(NativeOutputLifecycleEvent {
                        phase,
                        host_time: 1000,
                        status: None,
                        callbacks_in_flight: Some(0),
                    });
                assert!(!shutdown_proven(&retained));
                let mut pending = valid.clone();
                timing(&mut pending, index).first_fault_pending = true;
                assert!(!shutdown_proven(&pending));
            }
            let mut missing_route = valid.clone();
            missing_route.outputs.pop();
            assert!(!shutdown_proven(&missing_route));
            let mut running = valid;
            running.running = true;
            assert!(!shutdown_proven(&running));
        }
        fn with_evidence(run: impl FnOnce(&mut Evidence, &Path)) {
            let path =
                env::temp_dir().join(format!("relink-multioutput-probe-test-{}", Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            {
                let mut evidence = Evidence::new(&path).unwrap();
                run(&mut evidence, &path);
            }
            fs::remove_dir_all(path).unwrap();
        }
        #[test]
        fn partial_shutdown_snapshot_can_complete_within_original_window() {
            with_evidence(|evidence, _| {
                let mut final_snapshot = snapshot();
                complete(&mut final_snapshot);
                let mut partial = final_snapshot.clone();
                timing(&mut partial, 0).lifecycle
                    [NativeOutputLifecyclePhase::StopReturned as usize] = None;
                let mut observations = vec![partial, final_snapshot].into_iter();
                let mut polls = 0;
                evidence
                    .observe_shutdown_with(|| observations.next().unwrap(), || false, || polls += 1)
                    .unwrap();
                assert_eq!(polls, 1);
                assert_eq!(evidence.outputs_shutdown_proven, [true, true]);
            });
        }
        #[test]
        fn failed_cleanup_observation_remains_latched_even_if_later_snapshot_is_clean() {
            with_evidence(|evidence, _| {
                let mut final_snapshot = snapshot();
                complete(&mut final_snapshot);
                let mut failed = final_snapshot.clone();
                timing(&mut failed, 1).lifecycle
                    [NativeOutputLifecyclePhase::StopReturned as usize]
                    .as_mut()
                    .unwrap()
                    .status = Some(-1);
                timing(&mut failed, 1).lifecycle
                    [NativeOutputLifecyclePhase::OwnerCompleted as usize] = None;
                let mut observations = vec![failed, final_snapshot].into_iter();
                assert!(evidence
                    .observe_shutdown_with(|| observations.next().unwrap(), || false, || {})
                    .is_err());
                assert!(evidence.shutdown_fault_observed);
                assert_eq!(evidence.outputs_shutdown_proven, [false, false]);
            });
        }
        #[test]
        fn missing_cleanup_proof_fails_at_deadline_without_claiming_success() {
            with_evidence(|evidence, _| {
                assert!(evidence
                    .observe_shutdown_with(snapshot, || true, || panic!("deadline must finish"))
                    .is_err());
                assert_eq!(evidence.outputs_shutdown_proven, [false, false]);
            });
        }
        #[test]
        fn protocol_topology_has_two_distinct_sources_and_no_cross_bus_send_or_dsp() {
            let (session, buses) = build_session().unwrap();
            assert_eq!(session.sample_rate, RATE);
            assert_eq!(session.inputs.len(), 2);
            assert_eq!(session.sends.len(), 2);
            for index in 0..2 {
                assert_eq!(
                    session.inputs[index].kind,
                    InputKind::TestTone {
                        frequency_hz: FREQUENCIES[index],
                        amplitude_db: -18.0
                    }
                );
                assert_eq!(session.sends[index].input_id, session.inputs[index].id);
                assert_eq!(session.sends[index].bus_id, buses[index]);
                assert_eq!(session.buses[index].dsp, DspConfig::default());
                assert!(session.sends[index].controls.linked);
            }
        }
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    if let Err(error) = native::main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("This installed acceptance probe requires macOS");
        std::process::exit(1);
    }
}
