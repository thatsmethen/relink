//! Explicit-consent synthetic-application observer using the public AudioRuntime API.
//! No actual capture is started by its unit tests or the fixture prepare command.
//! No callback or product implementation is replaced by this instrument.

#[cfg(target_os = "macos")]
mod native {
    use relink_audio::{
        AudioFault, AudioRuntime, AudioSnapshot, FaultAttribution, NativeCallbackOutcome,
        NativeCapturePacket, NativeLifecyclePhase, NativeOutputLifecyclePhase, NativeOutputOutcome,
        NativeOutputPacket, NativeOutputStamp, NativeTimingSnapshot, OutputId,
        OutputTimingSnapshot,
    };
    use relink_core::{ApplicationSelector, Bus, DspConfig, Input, InputKind, Send, Session};
    use serde_json::{json, Value};
    use std::{
        env,
        fs::{self, File, OpenOptions},
        io::{BufWriter, Write},
        path::{Path, PathBuf},
        thread,
        time::{Duration, Instant},
    };

    const PROTOCOL: &str = "relink-native-application-tones-v1";
    const FIXTURE_BUNDLE_ID: &str = "au.textual.relink.fixture.selected";
    const OUTPUT_ID: &str = "coreaudio:au.textual.ravelink.cable.2";
    const TWO_PROCESS_FORMAT: &str = "private stereo process tap; 2 current bundle audio processes; restart on membership change";

    fn application_id(bundle: &Path, bundle_id: &str) -> Result<String, String> {
        if bundle_id != FIXTURE_BUNDLE_ID
            || !bundle.is_absolute()
            || bundle.file_name().and_then(|v| v.to_str()) != Some("ReLinkToneFixture.app")
            || bundle.canonicalize().map_err(|e| e.to_string())? != bundle
        {
            return Err(
                "The protocol requires its canonical selected fixture bundle and fixed bundle ID"
                    .into(),
            );
        }
        ApplicationSelector::MacBundle {
            bundle_id: bundle_id.into(),
            path: bundle.to_str().ok_or("Fixture path is not UTF-8")?.into(),
        }
        .encode()
        .map_err(|e| e.to_string())
    }

    extern "C" {
        fn mach_absolute_time() -> u64;
    }
    fn ticks() -> u64 {
        unsafe { mach_absolute_time() }
    }
    fn packet_json(packet: NativeCapturePacket) -> Value {
        json!({
            "packet_index": packet.packet_index, "queue_frame": packet.queue_frame,
            "sample_time": packet.sample_time, "sample_time_bits": packet.sample_time.to_bits(),
            "host_time": packet.host_time, "observed_host_time": packet.observed_host_time,
            "rate_scalar": packet.rate_scalar, "rate_scalar_bits": packet.rate_scalar.to_bits(),
            "flags": packet.flags, "frames": packet.frames, "epoch": packet.epoch,
        })
    }
    fn timing_json(timing: NativeTimingSnapshot) -> Value {
        let completed = timing.last_completed_callback.map(|callback| {
            let outcome = match callback.outcome {
                NativeCallbackOutcome::Published => json!({"kind":"published"}),
                NativeCallbackOutcome::Cancelled => json!({"kind":"cancelled"}),
                NativeCallbackOutcome::Rejected(fault) => json!({"kind":"rejected", "fault": fault as u32}),
            };
            json!({
                "packet_index": callback.packet_index, "entry_host_time": callback.entry_host_time,
                "after_pcm_host_time": callback.after_pcm_host_time, "completion_host_time": callback.completion_host_time,
                "os_now_sample_time": callback.os_now_sample_time, "os_now_sample_time_bits": callback.os_now_sample_time.to_bits(),
                "os_now_host_time": callback.os_now_host_time, "os_now_flags": callback.os_now_flags, "outcome": outcome,
            })
        });
        let first_fault = timing.first_fault.map(|fault| json!({
            "fault": fault.fault as u32, "previous": fault.previous.map(packet_json), "current": packet_json(fault.current),
        }));
        let lifecycle: Vec<Value> = timing
            .lifecycle
            .into_iter()
            .flatten()
            .map(|event| {
                json!({
                    "phase": format!("{:?}", event.phase), "host_time": event.host_time,
                    "status": event.status, "callbacks_in_flight": event.callbacks_in_flight,
                })
            })
            .collect();
        json!({
            "packets_seen": timing.packets_seen, "last_packet": timing.last_packet.map(packet_json),
            "last_packet_contended": timing.last_packet_contended, "first_fault": first_fault,
            "last_completed_callback": completed, "callback_completion_contended": timing.callback_completion_contended,
            "lifecycle": lifecycle,
        })
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
    fn output_shutdown_observed(snapshot: &AudioSnapshot) -> bool {
        snapshot.output_timing.as_ref().is_some_and(|timing| {
            let OutputTimingSnapshot::CoreAudio(timing) = timing;
            timing.lifecycle[NativeOutputLifecyclePhase::OwnerCompleted as usize].is_some()
                || timing.lifecycle[NativeOutputLifecyclePhase::ContextRetained as usize].is_some()
        })
    }
    fn output_shutdown_proven(snapshot: &AudioSnapshot) -> bool {
        if snapshot.fault != AudioFault::None || snapshot.error.is_some() {
            return false;
        }
        snapshot.output_timing.as_ref().is_some_and(|timing| {
            let OutputTimingSnapshot::CoreAudio(timing) = timing;
            let event = |phase: NativeOutputLifecyclePhase| {
                timing.lifecycle[phase as usize]
                    .filter(|event| event.phase == phase && event.host_time > 0)
            };
            timing.first_fault.is_none()
                && !timing.first_fault_pending
                && timing.lifecycle[NativeOutputLifecyclePhase::ContextRetained as usize].is_none()
                && [
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
        })
    }
    fn capture_shutdown_proven(snapshot: &AudioSnapshot) -> bool {
        snapshot.fault == AudioFault::None
            && snapshot.error.is_none()
            && snapshot.captures.len() == 1
            && snapshot.captures[0].native_timing.is_some_and(|timing| {
                let event = |phase: NativeLifecyclePhase| {
                    timing.lifecycle[phase as usize]
                        .filter(|event| event.phase == phase && event.host_time > 0)
                };
                timing.first_fault.is_none()
                    && event(NativeLifecyclePhase::ContextRetained).is_none()
                    && [
                        NativeLifecyclePhase::CreateReturned,
                        NativeLifecyclePhase::StartReturned,
                        NativeLifecyclePhase::StopReturned,
                        NativeLifecyclePhase::DestroyReturned,
                        NativeLifecyclePhase::ListenersRemovalReturned,
                    ]
                    .into_iter()
                    .all(|phase| event(phase).is_some_and(|event| event.status == Some(0)))
                    && [
                        NativeLifecyclePhase::QuiescenceChecked,
                        NativeLifecyclePhase::OwnerCompleted,
                    ]
                    .into_iter()
                    .all(|phase| {
                        event(phase).is_some_and(|event| event.callbacks_in_flight == Some(0))
                    })
            })
    }
    fn shutdown_fault(snapshot: &AudioSnapshot) -> Option<String> {
        if snapshot.fault != AudioFault::None || snapshot.error.is_some() {
            return Some(format!(
                "runtime fault during shutdown: fault={:?}, error={:?}",
                snapshot.fault, snapshot.error
            ));
        }
        if let Some(OutputTimingSnapshot::CoreAudio(timing)) = &snapshot.output_timing {
            if timing.first_fault.is_some() || timing.first_fault_pending {
                return Some(format!(
                    "native output fault during shutdown: first_fault={:?}, pending={}",
                    timing.first_fault, timing.first_fault_pending
                ));
            }
        }
        None
    }
    fn output_shutdown_failed(snapshot: &AudioSnapshot) -> bool {
        if snapshot.fault != AudioFault::None || snapshot.error.is_some() {
            return true;
        }
        snapshot.output_timing.as_ref().is_some_and(|timing| {
            let OutputTimingSnapshot::CoreAudio(timing) = timing;
            timing.first_fault.is_some()
                || timing.first_fault_pending
                || timing.lifecycle[NativeOutputLifecyclePhase::ContextRetained as usize].is_some()
                || [
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
    fn native_shutdown_observed(snapshot: &AudioSnapshot) -> bool {
        output_shutdown_observed(snapshot)
            && !snapshot.captures.is_empty()
            && snapshot.captures.iter().all(|capture| {
                capture.native_timing.is_some_and(|timing| {
                    timing.lifecycle[NativeLifecyclePhase::OwnerCompleted as usize].is_some()
                        || timing.lifecycle[NativeLifecyclePhase::ContextRetained as usize]
                            .is_some()
                })
            })
    }
    fn shutdown_observation_finished(snapshot: &AudioSnapshot, deadline_elapsed: bool) -> bool {
        // Lifecycle slots are read individually. A concurrently published final
        // slot can be visible while an earlier slot was read as absent. Wait for
        // complete proof or explicit failure; missing evidence gets the same
        // bounded observation window rather than an immediate false rejection.
        (native_shutdown_observed(snapshot)
            && ((output_shutdown_proven(snapshot) && capture_shutdown_proven(snapshot))
                || output_shutdown_failed(snapshot)))
            || (snapshot.captures.is_empty() && snapshot.output_timing.is_none())
            || deadline_elapsed
    }
    fn write_new(path: &Path, text: &str) -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())
    }
    fn publish_ready(directory: &Path, snapshot: &AudioSnapshot) -> Result<(), String> {
        let temporary = directory.join("runtime.ready.pending");
        write_new(&temporary, &format!("{{\"ready\":true,\"host_time\":{},\"rendered_frames\":{},\"captured_frames\":{}}}\n",
            ticks(), snapshot.rendered_frames, snapshot.captures[0].captured_frames))?;
        fs::hard_link(&temporary, directory.join("runtime.ready.json"))
            .map_err(|e| e.to_string())?;
        fs::remove_file(temporary).map_err(|e| e.to_string())
    }
    struct Evidence {
        events: BufWriter<File>,
        telemetry: BufWriter<File>,
        native_timing: BufWriter<File>,
        shutdown_fault_observed: bool,
        native_output_shutdown_proven: bool,
        native_capture_shutdown_proven: bool,
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
            let mut telemetry = BufWriter::new(create("runtime-telemetry.csv")?);
            writeln!(telemetry, "host_time,rendered_frames,callback_count,applied_control_version,captured_frames,buffered_frames,clock_correction_ppm,fault,running").map_err(|e| e.to_string())?;
            Ok(Self {
                events: BufWriter::new(create("runtime-events.jsonl")?),
                telemetry,
                native_timing: BufWriter::new(create("runtime-native-timing.jsonl")?),
                shutdown_fault_observed: false,
                native_output_shutdown_proven: false,
                native_capture_shutdown_proven: false,
            })
        }
        fn event(&mut self, text: &str) -> Result<(), String> {
            writeln!(self.events, "{text}")
                .and_then(|_| self.events.flush())
                .map_err(|e| e.to_string())
        }
        fn snapshot(&mut self, runtime: &AudioRuntime) -> Result<AudioSnapshot, String> {
            let snapshot = runtime.snapshot();
            self.native_snapshot(&snapshot, "running", None)?;
            let capture = snapshot.captures.first();
            writeln!(
                self.telemetry,
                "{},{},{},{},{},{},{},{},{}",
                ticks(),
                snapshot.rendered_frames,
                snapshot.callback_count,
                snapshot.applied_control_version,
                capture.map_or(0, |c| c.captured_frames),
                capture.map_or(0, |c| c.buffered_frames),
                capture.map_or(0.0, |c| c.clock_correction_ppm),
                snapshot.fault as u32,
                u8::from(snapshot.running)
            )
            .and_then(|_| self.telemetry.flush())
            .map_err(|e| e.to_string())?;
            if snapshot.fault != AudioFault::None || snapshot.error.is_some() || !snapshot.running {
                return Err(format!(
                    "runtime stopped: fault={:?}, error={:?}",
                    snapshot.fault, snapshot.error
                ));
            }
            Ok(snapshot)
        }
        fn native_snapshot(
            &mut self,
            snapshot: &AudioSnapshot,
            observation: &str,
            shutdown_complete: Option<bool>,
        ) -> Result<(), String> {
            let captures: Vec<Value> = snapshot
                .captures
                .iter()
                .map(|capture| {
                    json!({
                        "input_id": capture.device.0, "captured_frames": capture.captured_frames,
                        "owner_status": format!("{:?}", capture.owner_status),
                        "native_timing": capture.native_timing.map(timing_json),
                    })
                })
                .collect();
            let record = json!({"schema":1, "protocol":PROTOCOL, "observation":observation, "observed_host_time":ticks(),
                "running":snapshot.running, "fault": snapshot.fault as u32,
                "first_fault":snapshot.first_fault.as_ref().map(fault_json),
                "error":snapshot.error.as_ref().map(ToString::to_string), "captures": captures,
                "output_timing":snapshot.output_timing.as_ref().map(output_timing_json),
                "native_output_shutdown_observed":output_shutdown_observed(snapshot),
                "native_output_shutdown_proven":!self.shutdown_fault_observed && output_shutdown_proven(snapshot),
                "native_capture_shutdown_proven":!self.shutdown_fault_observed && capture_shutdown_proven(snapshot),
                "native_shutdown_observed":shutdown_complete});
            writeln!(self.native_timing, "{record}")
                .and_then(|_| self.native_timing.flush())
                .map_err(|e| e.to_string())
        }
        fn observe_shutdown(&mut self, runtime: &AudioRuntime) -> Result<(), String> {
            // The separate output teardown gate leaves PCM/control/timestamp
            // thresholds unchanged. Observation is bounded even if a native
            // owner is stuck. Capture proof requires all successful return slots
            // plus OwnerCompleted, emitted after dependent tap resources close.
            let deadline = Instant::now() + Duration::from_secs(2);
            self.observe_shutdown_with(
                || runtime.snapshot(),
                || Instant::now() >= deadline,
                || thread::sleep(Duration::from_millis(10)),
            )
        }
        fn observe_shutdown_with(
            &mut self,
            mut snapshot: impl FnMut() -> AudioSnapshot,
            mut deadline_elapsed: impl FnMut() -> bool,
            mut poll: impl FnMut(),
        ) -> Result<(), String> {
            let mut first_fault = None;
            loop {
                let snapshot = snapshot();
                if first_fault.is_none() {
                    first_fault = shutdown_fault(&snapshot);
                }
                self.shutdown_fault_observed |= first_fault.is_some();
                let completed = native_shutdown_observed(&snapshot);
                let final_observation =
                    shutdown_observation_finished(&snapshot, deadline_elapsed());
                self.native_snapshot(
                    &snapshot,
                    if final_observation {
                        "post-stop-final"
                    } else {
                        "post-stop"
                    },
                    final_observation.then_some(completed),
                )?;
                if final_observation {
                    self.native_output_shutdown_proven =
                        !self.shutdown_fault_observed && output_shutdown_proven(&snapshot);
                    self.native_capture_shutdown_proven =
                        !self.shutdown_fault_observed && capture_shutdown_proven(&snapshot);
                    return if let Some(error) = first_fault {
                        Err(error)
                    } else if self.native_output_shutdown_proven
                        && self.native_capture_shutdown_proven
                    {
                        Ok(())
                    } else {
                        Err("Native input/output shutdown was not proven by successful cleanup and zero callbacks in flight".into())
                    };
                }
                poll();
            }
        }
    }
    struct StopOnDrop<'a>(&'a AudioRuntime);
    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.cancel_output();
            let _ = self.0.stop();
        }
    }
    fn execute(
        directory: &Path,
        input: &str,
        output: &str,
        evidence: &mut Evidence,
    ) -> Result<(), String> {
        let runtime = AudioRuntime::new().map_err(|e| e.to_string())?;
        let _stop = StopOnDrop(&runtime);
        let result = execute_active(&runtime, directory, input, output, evidence);
        if result.is_err() {
            runtime.cancel_output();
            // Cleanup is always attempted; the original failure remains the
            // result, while JSONL retains the last available native diagnostics.
            let _ = runtime.stop();
        }
        let diagnostics = evidence.observe_shutdown(&runtime);
        result.and(diagnostics)
    }
    fn execute_active(
        runtime: &AudioRuntime,
        directory: &Path,
        input: &str,
        output: &str,
        evidence: &mut Evidence,
    ) -> Result<(), String> {
        if output != OUTPUT_ID {
            return Err("This fixed protocol requires the explicit Cable2 output".into());
        }
        let outputs = runtime
            .enumerate_output_devices()
            .map_err(|e| e.to_string())?;
        if outputs.iter().filter(|d| d.id.0 == output).count() != 1 {
            return Err(
                "Explicit output identity was absent or ambiguous; no default substitution".into(),
            );
        }
        let mut session = Session::new("Synthetic application capture acceptance", 48_000);
        let source = session
            .add_input(Input::new(
                "Selected two-process fixture",
                InputKind::Application {
                    application_id: input.into(),
                },
            ))
            .map_err(|e| e.to_string())?;
        let mut bus = Bus::new("Measured Cable2 output");
        bus.dsp = DspConfig::default();
        let bus = session.add_bus(bus).map_err(|e| e.to_string())?;
        let mut send = Send::new(source, bus);
        send.controls.set_gain_db(-6.0);
        session.add_send(send).map_err(|e| e.to_string())?;
        let configuration = runtime
            .start(session.clone(), bus, OutputId(output.into()))
            .map_err(|e| e.to_string())?;
        if configuration.sample_rate != 48_000
            || configuration.device_channels != 2
            || configuration.inputs.len() != 1
            || configuration.inputs[0].device.id.0 != input
            || configuration.inputs[0].device_channels != 2
            || configuration.inputs[0].sample_format != TWO_PROCESS_FORMAT
        {
            return Err(format!(
                "Unexpected native configuration: {configuration:?}"
            ));
        }
        evidence.event(
            &json!({"event":"started", "protocol":PROTOCOL,
            "host_time":ticks(), "input_id":input, "output_id":output,
            "sample_rate":48000, "input_format":configuration.inputs[0].sample_format,
            "output_format":configuration.sample_format,
            "expected_current_bundle_audio_processes":2})
            .to_string(),
        )?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = evidence.snapshot(runtime)?;
            if snapshot.rendered_frames >= 4096
                && snapshot.callback_count >= 2
                && snapshot.captures.len() == 1
                && snapshot.captures[0].captured_frames >= 4096
                && snapshot
                    .bus_meters
                    .iter()
                    .any(|m| m.rms.iter().all(|&r| r > 0.01))
            {
                publish_ready(directory, &snapshot)?;
                break;
            }
            if Instant::now() >= deadline {
                return Err(
                    "Real input/mix callbacks did not become ready within five seconds".into(),
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !directory.join("runtime.go").exists() {
            evidence.snapshot(runtime)?;
            if Instant::now() >= deadline {
                return Err("Independent recorder did not arm the phase sequence".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        // Protocol constants. The independent analyzer repeats these values;
        // events provide timing and acknowledgement, never acceptance limits.
        for (phase, gain, muted, seconds) in [
            (0, -6.0, false, 6),
            (1, -12.0, false, 6),
            (2, -12.0, true, 4),
            (3, -6.0, false, 6),
        ] {
            session.sends[0].controls.set_gain_db(gain);
            session.sends[0].controls.muted = muted;
            let requested = ticks();
            let version = runtime
                .update_controls(session.clone())
                .map_err(|e| e.to_string())?;
            let deadline = Instant::now() + Duration::from_secs(2);
            let snapshot = loop {
                let snapshot = evidence.snapshot(runtime)?;
                if snapshot.applied_control_version == version {
                    break snapshot;
                }
                if snapshot.applied_control_version > version {
                    return Err("Unexpected control version overtook the phase".into());
                }
                if Instant::now() >= deadline {
                    return Err("Control version was not acknowledged by an output callback".into());
                }
                thread::sleep(Duration::from_millis(5));
            };
            let acknowledged = ticks();
            evidence.event(&format!("{{\"event\":\"phase\",\"phase\":{phase},\"gain_db\":{gain},\"muted\":{muted},\"requested_host_time\":{requested},\"acknowledged_host_time\":{acknowledged},\"version\":{version},\"rendered_frames\":{},\"callback_count\":{}}}",
                snapshot.rendered_frames, snapshot.callback_count))?;
            let end = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < end {
                evidence.snapshot(runtime)?;
                thread::sleep(Duration::from_millis(20));
            }
        }
        let snapshot = evidence.snapshot(runtime)?;
        let requested = ticks();
        runtime.stop().map_err(|e| e.to_string())?;
        evidence.event(&format!("{{\"event\":\"stopped\",\"requested_host_time\":{requested},\"returned_host_time\":{},\"rendered_frames\":{},\"callback_count\":{}}}", ticks(), snapshot.rendered_frames, snapshot.callback_count))?;
        Ok(())
    }
    pub fn main() -> Result<(), String> {
        if env::var("RELINK_APPLICATION_CAPTURE_CONSENT").as_deref() != Ok("1") {
            return Err("Synthetic application capture requires explicit system-audio recording consent; prepare alone never starts audio".into());
        }
        let mut bundle = None;
        let mut bundle_id = None;
        let mut output = None;
        let mut directory = None;
        let mut args = env::args().skip(1);
        while let Some(key) = args.next() {
            let value = args.next().ok_or("Missing option value")?;
            match key.as_str() {
                "--application-bundle" if bundle.is_none() => bundle = Some(PathBuf::from(value)),
                "--application-bundle-id" if bundle_id.is_none() => bundle_id = Some(value),
                "--output-id" if output.is_none() => output = Some(value),
                "--directory" if directory.is_none() => directory = Some(PathBuf::from(value)),
                _ => return Err(format!("Unknown or duplicate option {key}")),
            }
        }
        let directory = directory.ok_or("Missing --directory")?;
        let bundle = bundle.ok_or("Missing --application-bundle")?;
        let bundle_id = bundle_id.ok_or("Missing --application-bundle-id")?;
        let output = output.ok_or("Missing --output-id")?;
        if directory.join("runtime.go").exists() || directory.join("runtime.ready.json").exists() {
            return Err("Refusing stale fixture protocol markers".into());
        }
        let mut evidence = Evidence::new(&directory)?;
        let result = application_id(&bundle, &bundle_id)
            .and_then(|input| execute(&directory, &input, &output, &mut evidence));
        write_new(
            &directory.join("runtime-result.json"),
            &format!(
                "{}\n",
                json!({
                    "schema":1, "protocol":PROTOCOL, "pass":result.is_ok(),
                    "native_output_shutdown_proven":evidence.native_output_shutdown_proven,
                    "native_capture_shutdown_proven":evidence.native_capture_shutdown_proven,
                    "error":result.as_ref().err(),
                })
            ),
        )?;
        result
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn unrelated_bundles_and_ids_are_rejected_without_native_capture() {
            assert!(application_id(
                Path::new("/Applications/ActualUserApp.app"),
                FIXTURE_BUNDLE_ID
            )
            .is_err());
            assert!(application_id(Path::new("ReLinkToneFixture.app"), FIXTURE_BUNDLE_ID).is_err());
            assert!(
                application_id(Path::new("/tmp/ReLinkToneFixture.app"), "unrelated.app").is_err()
            );
        }
        fn complete_capture_snapshot() -> AudioSnapshot {
            use relink_audio::{
                CaptureSnapshot, InputId, NativeLifecycleEvent, NATIVE_LIFECYCLE_PHASES,
            };
            let mut timing = NativeTimingSnapshot {
                packets_seen: 1,
                last_packet: None,
                first_fault: None,
                last_packet_contended: false,
                last_completed_callback: None,
                callback_completion_contended: false,
                lifecycle: [None; NATIVE_LIFECYCLE_PHASES],
            };
            for phase in [
                NativeLifecyclePhase::CreateReturned,
                NativeLifecyclePhase::StartReturned,
                NativeLifecyclePhase::StopReturned,
                NativeLifecyclePhase::DestroyReturned,
                NativeLifecyclePhase::ListenersRemovalReturned,
                NativeLifecyclePhase::QuiescenceChecked,
                NativeLifecyclePhase::OwnerCompleted,
            ] {
                timing.lifecycle[phase as usize] = Some(NativeLifecycleEvent {
                    phase,
                    host_time: 100 + phase as u64,
                    status: Some(0),
                    callbacks_in_flight: Some(0),
                });
            }
            AudioSnapshot {
                outputs: Vec::new(),
                running: false,
                configuration: None,
                rendered_frames: 512,
                callback_count: 1,
                applied_control_version: 1,
                captures: vec![CaptureSnapshot {
                    device: InputId("synthetic".into()),
                    captured_frames: 512,
                    buffered_frames: 0,
                    clock_correction_ppm: 0.0,
                    native_timing: Some(timing),
                    event_close: None,
                    owner_status: relink_audio::CaptureOwnerStatus::Completed,
                }],
                output_timing: None,
                input_meters: Vec::new(),
                bus_meters: Vec::new(),
                fault: AudioFault::None,
                first_fault: None,
                error: None,
            }
        }
        #[test]
        fn incomplete_or_failed_capture_cleanup_never_qualifies() {
            let good = complete_capture_snapshot();
            assert!(capture_shutdown_proven(&good));
            assert!(!output_shutdown_proven(&good));
            for phase in [
                NativeLifecyclePhase::CreateReturned,
                NativeLifecyclePhase::StartReturned,
                NativeLifecyclePhase::StopReturned,
                NativeLifecyclePhase::DestroyReturned,
                NativeLifecyclePhase::ListenersRemovalReturned,
                NativeLifecyclePhase::QuiescenceChecked,
                NativeLifecyclePhase::OwnerCompleted,
            ] {
                let mut missing = good.clone();
                missing.captures[0]
                    .native_timing
                    .as_mut()
                    .unwrap()
                    .lifecycle[phase as usize] = None;
                assert!(!capture_shutdown_proven(&missing), "{phase:?}");
                let mut absent_time = good.clone();
                absent_time.captures[0]
                    .native_timing
                    .as_mut()
                    .unwrap()
                    .lifecycle[phase as usize]
                    .as_mut()
                    .unwrap()
                    .host_time = 0;
                assert!(!capture_shutdown_proven(&absent_time), "{phase:?}");
            }
            for phase in [
                NativeLifecyclePhase::CreateReturned,
                NativeLifecyclePhase::StartReturned,
                NativeLifecyclePhase::StopReturned,
                NativeLifecyclePhase::DestroyReturned,
                NativeLifecyclePhase::ListenersRemovalReturned,
            ] {
                let mut failed = good.clone();
                failed.captures[0].native_timing.as_mut().unwrap().lifecycle[phase as usize]
                    .as_mut()
                    .unwrap()
                    .status = Some(-7);
                assert!(!capture_shutdown_proven(&failed), "{phase:?}");
            }
            for phase in [
                NativeLifecyclePhase::QuiescenceChecked,
                NativeLifecyclePhase::OwnerCompleted,
            ] {
                let mut active = good.clone();
                active.captures[0].native_timing.as_mut().unwrap().lifecycle[phase as usize]
                    .as_mut()
                    .unwrap()
                    .callbacks_in_flight = Some(1);
                assert!(!capture_shutdown_proven(&active));
            }
            let mut faulted = good.clone();
            faulted.fault = AudioFault::Xrun;
            assert!(!capture_shutdown_proven(&faulted));
            let mut empty = good;
            empty.captures.clear();
            assert!(!capture_shutdown_proven(&empty));
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
        eprintln!("This fixed application-capture observer requires macOS CoreAudio");
        std::process::exit(1);
    }
}
