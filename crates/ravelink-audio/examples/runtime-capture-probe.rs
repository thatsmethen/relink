//! Disposable-runner acceptance client of the real public AudioRuntime API.
//! No callback or product implementation is replaced by this instrument.

#[cfg(target_os = "macos")]
mod native {
    use ravelink_audio::{
        AudioFault, AudioRuntime, AudioSnapshot, FaultAttribution, NativeCallbackOutcome,
        NativeCapturePacket, NativeLifecyclePhase, NativeOutputLifecyclePhase, NativeOutputOutcome,
        NativeOutputPacket, NativeOutputStamp, NativeTimingSnapshot, OutputId,
        OutputTimingSnapshot,
    };
    use ravelink_core::{Bus, DspConfig, Input, InputKind, Send, Session};
    use serde_json::{json, Value};
    use std::{
        env,
        fs::{self, File, OpenOptions},
        io::{BufWriter, Write},
        path::{Path, PathBuf},
        thread,
        time::{Duration, Instant},
    };

    extern "C" {
        fn mach_absolute_time() -> u64;
    }
    fn ticks() -> u64 {
        unsafe { mach_absolute_time() }
    }
    fn quoted(value: &str) -> String {
        let mut out = String::from("\"");
        for character in value.chars() {
            match character {
                '\"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if c <= '\u{1f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('\"');
        out
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
                timing.lifecycle[phase as usize].filter(|event| event.phase == phase)
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
            && (output_shutdown_proven(snapshot) || output_shutdown_failed(snapshot)))
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
                        "native_timing": capture.native_timing.map(timing_json),
                    })
                })
                .collect();
            let record = json!({"schema":1, "observation":observation, "observed_host_time":ticks(),
                "running":snapshot.running, "fault": snapshot.fault as u32,
                "first_fault":snapshot.first_fault.as_ref().map(fault_json),
                "error":snapshot.error.as_ref().map(ToString::to_string), "captures": captures,
                "output_timing":snapshot.output_timing.as_ref().map(output_timing_json),
                "native_output_shutdown_observed":output_shutdown_observed(snapshot),
                "native_output_shutdown_proven":!self.shutdown_fault_observed && output_shutdown_proven(snapshot),
                "native_shutdown_observed":shutdown_complete});
            writeln!(self.native_timing, "{record}")
                .and_then(|_| self.native_timing.flush())
                .map_err(|e| e.to_string())
        }
        fn observe_shutdown(&mut self, runtime: &AudioRuntime) -> Result<(), String> {
            // The separate output teardown gate leaves PCM/control/timestamp
            // thresholds and legacy CSV rows unchanged. Observation is bounded
            // even if an owner is stuck in a native call. Capture terminal events
            // remain observations, not proof that all capture fields were freed.
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
                    return if let Some(error) = first_fault {
                        Err(error)
                    } else if self.native_output_shutdown_proven {
                        Ok(())
                    } else {
                        Err("Native output shutdown was not proven by successful cleanup and zero callbacks in flight".into())
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
        if input != "coreaudio:au.textual.ravelink.cable.1"
            || output != "coreaudio:au.textual.ravelink.cable.2"
        {
            return Err(
                "This fixed protocol requires distinct explicit Cable1 input and Cable2 output IDs"
                    .into(),
            );
        }
        let inputs = runtime
            .enumerate_input_devices()
            .map_err(|e| e.to_string())?;
        let outputs = runtime
            .enumerate_output_devices()
            .map_err(|e| e.to_string())?;
        if inputs.iter().filter(|d| d.id.0 == input).count() != 1
            || outputs.iter().filter(|d| d.id.0 == output).count() != 1
        {
            return Err("Explicit runtime device identities were absent or ambiguous; no default substitution".into());
        }
        let mut session = Session::new("Installed capture acceptance", 48_000);
        let source = session
            .add_input(Input::new(
                "Independent Cable1 source",
                InputKind::Hardware {
                    device_id: input.into(),
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
        {
            return Err(format!(
                "Unexpected native configuration: {configuration:?}"
            ));
        }
        evidence.event(&format!("{{\"event\":\"started\",\"host_time\":{},\"input_id\":{},\"output_id\":{},\"sample_rate\":48000,\"input_format\":{},\"output_format\":{}}}",
            ticks(), quoted(input), quoted(output), quoted(&configuration.inputs[0].sample_format), quoted(&configuration.sample_format)))?;
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
        if env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
            || env::var("RUNNER_OS").as_deref() != Ok("macOS")
            || env::var("RAVELINK_RUNTIME_ACCEPTANCE").as_deref() != Ok("1")
        {
            return Err(
                "This audio probe runs only in the explicitly enabled disposable macOS CI protocol"
                    .into(),
            );
        }
        let mut input = None;
        let mut output = None;
        let mut directory = None;
        let mut args = env::args().skip(1);
        while let Some(key) = args.next() {
            let value = args.next().ok_or("Missing option value")?;
            match key.as_str() {
                "--input-id" => input = Some(value),
                "--output-id" => output = Some(value),
                "--directory" => directory = Some(PathBuf::from(value)),
                _ => return Err(format!("Unknown option {key}")),
            }
        }
        let directory = directory.ok_or("Missing --directory")?;
        let mut evidence = Evidence::new(&directory)?;
        let result = execute(
            &directory,
            &input.ok_or("Missing --input-id")?,
            &output.ok_or("Missing --output-id")?,
            &mut evidence,
        );
        let error = result.as_ref().err().map_or("null".into(), |e| quoted(e));
        write_new(
            &directory.join("runtime-result.json"),
            &format!(
                "{{\"schema\":1,\"pass\":{},\"native_output_shutdown_proven\":{},\"error\":{error}}}\n",
                result.is_ok(), evidence.native_output_shutdown_proven
            ),
        )?;
        result
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use ravelink_audio::{
            AudioError, CaptureSnapshot, InputId, NativeCallbackCompletion, NativeCaptureFault,
            NativeLifecycleEvent, NativeOutputFault, NativeOutputFaultReason,
            NativeOutputLifecycleEvent, NativeOutputTimingSnapshot, NATIVE_LIFECYCLE_PHASES,
            NATIVE_OUTPUT_LIFECYCLE_PHASES,
        };

        fn output_timing() -> OutputTimingSnapshot {
            OutputTimingSnapshot::CoreAudio(NativeOutputTimingSnapshot {
                callbacks_seen: 0,
                progress_count: 0,
                last_packet: None,
                last_packet_contended: false,
                first_fault: None,
                first_fault_pending: false,
                last_completion: None,
                completion_contended: false,
                lifecycle: [None; NATIVE_OUTPUT_LIFECYCLE_PHASES],
            })
        }

        fn output_shutdown_snapshot() -> AudioSnapshot {
            let mut snapshot = AudioSnapshot {
                outputs: Vec::new(),
                running: false,
                configuration: None,
                rendered_frames: 512,
                callback_count: 1,
                applied_control_version: 1,
                captures: Vec::new(),
                output_timing: Some(output_timing()),
                input_meters: Vec::new(),
                bus_meters: Vec::new(),
                fault: AudioFault::None,
                first_fault: None,
                error: None,
            };
            let OutputTimingSnapshot::CoreAudio(output) = snapshot.output_timing.as_mut().unwrap();
            for phase in [
                NativeOutputLifecyclePhase::StopReturned,
                NativeOutputLifecyclePhase::DestroyReturned,
                NativeOutputLifecyclePhase::ListenerRemovalReturned,
                NativeOutputLifecyclePhase::QuiescenceChecked,
                NativeOutputLifecyclePhase::OwnerCompleted,
            ] {
                output.lifecycle[phase as usize] = Some(NativeOutputLifecycleEvent {
                    phase,
                    host_time: 123 + phase as u64,
                    status: match phase {
                        NativeOutputLifecyclePhase::StopReturned
                        | NativeOutputLifecyclePhase::DestroyReturned
                        | NativeOutputLifecyclePhase::ListenerRemovalReturned => Some(0),
                        _ => None,
                    },
                    callbacks_in_flight: Some(0),
                });
            }
            snapshot
        }

        fn all_shutdown_snapshot() -> AudioSnapshot {
            let mut snapshot = output_shutdown_snapshot();
            let mut capture_timing = timing();
            capture_timing.first_fault = None;
            capture_timing.lifecycle[NativeLifecyclePhase::OwnerCompleted as usize] =
                Some(NativeLifecycleEvent {
                    phase: NativeLifecyclePhase::OwnerCompleted,
                    host_time: 999,
                    status: None,
                    callbacks_in_flight: Some(0),
                });
            snapshot.captures.push(CaptureSnapshot {
                device: InputId("synthetic".into()),
                captured_frames: 512,
                buffered_frames: 0,
                clock_correction_ppm: 0.0,
                native_timing: Some(capture_timing),
            });
            snapshot
        }

        fn timing() -> NativeTimingSnapshot {
            let packet = NativeCapturePacket {
                packet_index: 2,
                queue_frame: 512,
                sample_time: 52024.0,
                host_time: u64::MAX - 8,
                observed_host_time: u64::MAX - 7,
                rate_scalar: 1.0,
                flags: 7,
                frames: 512,
                epoch: 9,
            };
            let previous = NativeCapturePacket {
                packet_index: 1,
                queue_frame: 0,
                sample_time: 50944.0,
                host_time: u64::MAX - 100,
                observed_host_time: u64::MAX - 90,
                ..packet
            };
            NativeTimingSnapshot {
                packets_seen: 2,
                last_packet: Some(packet),
                first_fault: Some(NativeCaptureFault {
                    fault: AudioFault::Xrun,
                    previous: Some(previous),
                    current: packet,
                }),
                last_packet_contended: false,
                last_completed_callback: Some(NativeCallbackCompletion {
                    packet_index: 2,
                    entry_host_time: u64::MAX - 7,
                    after_pcm_host_time: u64::MAX - 6,
                    completion_host_time: u64::MAX - 5,
                    os_now_sample_time: 52088.0,
                    os_now_host_time: u64::MAX - 4,
                    os_now_flags: 3,
                    outcome: NativeCallbackOutcome::Rejected(AudioFault::Xrun),
                }),
                callback_completion_contended: false,
                lifecycle: [None; NATIVE_LIFECYCLE_PHASES],
            }
        }
        #[test]
        fn report_strings_escape_control_characters() {
            assert_eq!(super::quoted("a\"\\\n\0"), "\"a\\\"\\\\\\u000a\\u0000\"");
        }

        #[test]
        fn native_json_keeps_fault_pair_large_ticks_and_nonfinite_float_bits() {
            let mut timing = timing();
            let nan_bits = 0x7ff8_0000_0000_1234;
            timing.last_packet.as_mut().unwrap().sample_time = f64::from_bits(nan_bits);
            timing.last_packet.as_mut().unwrap().rate_scalar = f64::NEG_INFINITY;
            timing
                .last_completed_callback
                .as_mut()
                .unwrap()
                .os_now_sample_time = -0.0;
            timing.lifecycle[NativeLifecyclePhase::DestroyReturned as usize] =
                Some(NativeLifecycleEvent {
                    phase: NativeLifecyclePhase::DestroyReturned,
                    host_time: u64::MAX,
                    status: Some(-2000),
                    callbacks_in_flight: Some(1),
                });
            timing.lifecycle[NativeLifecyclePhase::HandleCancelled as usize] =
                Some(NativeLifecycleEvent {
                    phase: NativeLifecyclePhase::HandleCancelled,
                    host_time: u64::MAX - 1,
                    status: None,
                    callbacks_in_flight: None,
                });
            let serialized = timing_json(timing).to_string();
            let parsed: Value = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed["last_packet"]["sample_time"], Value::Null);
            assert_eq!(
                parsed["last_packet"]["sample_time_bits"].as_u64(),
                Some(nan_bits)
            );
            assert_eq!(parsed["last_packet"]["rate_scalar"], Value::Null);
            assert_eq!(
                parsed["last_packet"]["rate_scalar_bits"].as_u64(),
                Some(f64::NEG_INFINITY.to_bits())
            );
            assert_eq!(
                parsed["last_packet"]["host_time"].as_u64(),
                Some(u64::MAX - 8)
            );
            assert_eq!(parsed["first_fault"]["previous"]["sample_time"], 50944.0);
            assert_eq!(parsed["first_fault"]["current"]["sample_time"], 52024.0);
            assert_eq!(
                parsed["last_completed_callback"]["os_now_sample_time_bits"].as_u64(),
                Some((-0.0_f64).to_bits())
            );
            assert_eq!(
                parsed["last_completed_callback"]["outcome"]["fault"],
                AudioFault::Xrun as u32
            );
            assert_eq!(parsed["lifecycle"][0]["phase"], "HandleCancelled");
            assert_eq!(parsed["lifecycle"][0]["callbacks_in_flight"], Value::Null);
            assert_eq!(parsed["lifecycle"][1]["host_time"].as_u64(), Some(u64::MAX));
            assert_eq!(parsed["lifecycle"][1]["status"], -2000);
        }

        #[test]
        fn fault_json_preserves_source_raw_metadata_and_explicit_pending_state() {
            use ravelink_audio::{FaultDirection, FaultMetadata, FaultOrigin, FaultProperty};
            let mut record = FaultAttribution {
                fault: AudioFault::Xrun,
                origin: FaultOrigin::NativeInputOverload,
                direction: FaultDirection::Input,
                endpoint_index: Some(3),
                endpoint_id: Some("CoreAudio:device\n\"".into()),
                metadata: None,
            };
            let pending = fault_json(&record);
            assert_eq!(pending["metadata_pending"], true);
            assert_eq!(pending["metadata"], Value::Null);
            record.metadata = Some(FaultMetadata {
                epoch: 7,
                observed_mach_host_time: Some(u64::MAX),
                native_object: Some(91),
                property: Some(FaultProperty {
                    selector: u32::MAX,
                    scope: 5,
                    element: 0,
                }),
                status: Some(-2000),
                packet_index: None,
            });
            let decoded: Value = serde_json::from_str(&fault_json(&record).to_string()).unwrap();
            assert_eq!(decoded["fault"], 5);
            assert_eq!(decoded["origin"], "NativeInputOverload");
            assert_eq!(decoded["direction"], "Input");
            assert_eq!(decoded["endpoint_index"], 3);
            assert_eq!(decoded["endpoint_id"], record.endpoint_id.unwrap());
            assert_eq!(decoded["metadata_pending"], false);
            assert_eq!(
                decoded["metadata"]["observed_mach_host_time"].as_u64(),
                Some(u64::MAX)
            );
            assert_eq!(
                decoded["metadata"]["property"]["selector"].as_u64(),
                Some(u64::from(u32::MAX))
            );
            assert_eq!(decoded["metadata"]["status"], -2000);
            assert_eq!(decoded["metadata"]["packet_index"], Value::Null);
        }

        #[test]
        fn native_shutdown_observation_requires_an_explicit_terminal_phase() {
            let runtime = AudioRuntime::new().unwrap(); // No device discovery or open.
            let mut snapshot = runtime.snapshot();
            assert!(!native_shutdown_observed(&snapshot));
            snapshot.captures.push(CaptureSnapshot {
                device: InputId("synthetic".into()),
                captured_frames: 512,
                buffered_frames: 0,
                clock_correction_ppm: 0.0,
                native_timing: Some(timing()),
            });
            assert!(!native_shutdown_observed(&snapshot));
            let timing = snapshot.captures[0].native_timing.as_mut().unwrap();
            timing.lifecycle[NativeLifecyclePhase::StopReturned as usize] =
                Some(NativeLifecycleEvent {
                    phase: NativeLifecyclePhase::StopReturned,
                    host_time: 123,
                    status: Some(0),
                    callbacks_in_flight: Some(0),
                });
            assert!(!native_shutdown_observed(&snapshot));
            let timing = snapshot.captures[0].native_timing.as_mut().unwrap();
            timing.lifecycle[NativeLifecyclePhase::ContextRetained as usize] =
                Some(NativeLifecycleEvent {
                    phase: NativeLifecyclePhase::ContextRetained,
                    host_time: 456,
                    status: None,
                    callbacks_in_flight: Some(1),
                });
            assert!(!native_shutdown_observed(&snapshot));
            snapshot.output_timing = Some(output_timing());
            assert!(!native_shutdown_observed(&snapshot));
            let OutputTimingSnapshot::CoreAudio(output) = snapshot.output_timing.as_mut().unwrap();
            output.lifecycle[NativeOutputLifecyclePhase::OwnerCompleted as usize] =
                Some(NativeOutputLifecycleEvent {
                    phase: NativeOutputLifecyclePhase::OwnerCompleted,
                    host_time: 567,
                    status: None,
                    callbacks_in_flight: Some(0),
                });
            assert!(native_shutdown_observed(&snapshot));
            snapshot.captures.push(CaptureSnapshot {
                device: InputId("second-synthetic".into()),
                captured_frames: 0,
                buffered_frames: 0,
                clock_correction_ppm: 0.0,
                native_timing: None,
            });
            assert!(!native_shutdown_observed(&snapshot));
        }

        #[test]
        fn output_shutdown_acceptance_requires_every_successful_cleanup_event() {
            let complete = output_shutdown_snapshot();
            assert!(output_shutdown_observed(&complete));
            assert!(output_shutdown_proven(&complete));
            assert_eq!(shutdown_fault(&complete), None);
            let mut absent = complete.clone();
            absent.output_timing = None;
            assert!(!output_shutdown_proven(&absent));
            for phase in [
                NativeOutputLifecyclePhase::StopReturned,
                NativeOutputLifecyclePhase::DestroyReturned,
                NativeOutputLifecyclePhase::ListenerRemovalReturned,
                NativeOutputLifecyclePhase::QuiescenceChecked,
                NativeOutputLifecyclePhase::OwnerCompleted,
            ] {
                let mut missing = complete.clone();
                let OutputTimingSnapshot::CoreAudio(output) =
                    missing.output_timing.as_mut().unwrap();
                output.lifecycle[phase as usize] = None;
                assert!(!output_shutdown_proven(&missing), "missing {phase:?}");
            }
            for phase in [
                NativeOutputLifecyclePhase::StopReturned,
                NativeOutputLifecyclePhase::DestroyReturned,
                NativeOutputLifecyclePhase::ListenerRemovalReturned,
            ] {
                for status in [None, Some(-2000), Some(1)] {
                    let mut failed = complete.clone();
                    let OutputTimingSnapshot::CoreAudio(output) =
                        failed.output_timing.as_mut().unwrap();
                    output.lifecycle[phase as usize].as_mut().unwrap().status = status;
                    assert!(!output_shutdown_proven(&failed), "{phase:?}: {status:?}");
                    assert_eq!(output_shutdown_failed(&failed), status.is_some());
                }
            }
        }

        #[test]
        fn output_terminal_observation_does_not_accept_retention_or_unknown_quiescence() {
            let complete = output_shutdown_snapshot();
            for phase in [
                NativeOutputLifecyclePhase::QuiescenceChecked,
                NativeOutputLifecyclePhase::OwnerCompleted,
            ] {
                for inflight in [None, Some(1), Some(u64::MAX)] {
                    let mut changed = complete.clone();
                    let OutputTimingSnapshot::CoreAudio(output) =
                        changed.output_timing.as_mut().unwrap();
                    output.lifecycle[phase as usize]
                        .as_mut()
                        .unwrap()
                        .callbacks_in_flight = inflight;
                    assert!(output_shutdown_observed(&changed));
                    assert!(!output_shutdown_proven(&changed));
                }
            }
            let mut retained = complete.clone();
            let OutputTimingSnapshot::CoreAudio(output) = retained.output_timing.as_mut().unwrap();
            output.lifecycle[NativeOutputLifecyclePhase::ContextRetained as usize] =
                Some(NativeOutputLifecycleEvent {
                    phase: NativeOutputLifecyclePhase::ContextRetained,
                    host_time: 789,
                    status: None,
                    callbacks_in_flight: Some(0),
                });
            assert!(output_shutdown_observed(&retained));
            assert!(!output_shutdown_proven(&retained));
            assert!(output_shutdown_failed(&retained));
            let OutputTimingSnapshot::CoreAudio(output) = retained.output_timing.as_mut().unwrap();
            output.lifecycle[NativeOutputLifecyclePhase::OwnerCompleted as usize] = None;
            assert!(output_shutdown_observed(&retained));
            assert!(!output_shutdown_proven(&retained));
        }

        #[test]
        fn output_shutdown_rejects_native_pending_faults_and_late_runtime_failure() {
            let complete = output_shutdown_snapshot();
            let mut pending = complete.clone();
            let OutputTimingSnapshot::CoreAudio(output) = pending.output_timing.as_mut().unwrap();
            output.first_fault_pending = true;
            assert!(!output_shutdown_proven(&pending));
            assert!(shutdown_fault(&pending).unwrap().contains("pending=true"));
            let mut faulted = complete.clone();
            let OutputTimingSnapshot::CoreAudio(output) = faulted.output_timing.as_mut().unwrap();
            let stamp = NativeOutputStamp {
                sample_time: 512.0,
                host_time: 456,
                rate_scalar: 1.0,
                flags: 7,
            };
            output.first_fault = Some(NativeOutputFault {
                fault: AudioFault::Xrun,
                reason: NativeOutputFaultReason::SampleDiscontinuity,
                previous: None,
                current: NativeOutputPacket {
                    callback_index: 2,
                    epoch: 1,
                    frames: 512,
                    entry_host_time: 456,
                    now: stamp,
                    presentation: stamp,
                },
            });
            assert!(!output_shutdown_proven(&faulted));
            assert!(shutdown_fault(&faulted)
                .unwrap()
                .contains("SampleDiscontinuity"));
            let mut late_fault = complete.clone();
            late_fault.fault = AudioFault::Backend;
            assert!(!output_shutdown_proven(&late_fault));
            assert!(shutdown_fault(&late_fault)
                .unwrap()
                .contains("fault=Backend"));
            let mut late_error = complete;
            late_error.error = Some(AudioError::RuntimeUnavailable);
            assert!(!output_shutdown_proven(&late_error));
            assert!(shutdown_fault(&late_error)
                .unwrap()
                .contains("RuntimeUnavailable"));
        }

        #[test]
        fn shutdown_observation_does_not_skip_output_diagnostics_when_captures_are_empty() {
            let mut snapshot = output_shutdown_snapshot();
            snapshot.output_timing = Some(output_timing());
            assert!(!shutdown_observation_finished(&snapshot, false));
            assert!(shutdown_observation_finished(&snapshot, true));
            snapshot.output_timing = None;
            assert!(shutdown_observation_finished(&snapshot, false));
            assert!(!output_shutdown_proven(&snapshot));
            snapshot.captures.push(CaptureSnapshot {
                device: InputId("synthetic".into()),
                captured_frames: 512,
                buffered_frames: 0,
                clock_correction_ppm: 0.0,
                native_timing: Some(timing()),
            });
            assert!(!shutdown_observation_finished(&snapshot, false));
            assert!(shutdown_observation_finished(&snapshot, true));
        }

        #[test]
        fn output_shutdown_json_separates_observation_acceptance_and_latched_late_fault() {
            let directory = env::temp_dir().join(format!(
                "relink-output-shutdown-test-{}-{}",
                std::process::id(),
                ticks()
            ));
            fs::create_dir(&directory).unwrap();
            let mut evidence = Evidence::new(&directory).unwrap();
            let complete = output_shutdown_snapshot();
            evidence
                .native_snapshot(&complete, "synthetic-cleanup", Some(true))
                .unwrap();
            evidence.shutdown_fault_observed = true;
            evidence
                .native_snapshot(&complete, "synthetic-latched-fault", Some(true))
                .unwrap();
            drop(evidence);
            let records: Vec<Value> =
                fs::read_to_string(directory.join("runtime-native-timing.jsonl"))
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            assert_eq!(records[0]["native_output_shutdown_observed"], true);
            assert_eq!(records[0]["native_output_shutdown_proven"], true);
            assert_eq!(records[1]["native_output_shutdown_observed"], true);
            assert_eq!(records[1]["native_output_shutdown_proven"], false);
            fs::remove_dir_all(directory).unwrap();
        }

        #[test]
        fn shutdown_polling_rejects_late_fault_even_if_final_snapshot_looks_clean() {
            let directory = env::temp_dir().join(format!(
                "relink-output-shutdown-poll-test-{}-{}",
                std::process::id(),
                ticks()
            ));
            fs::create_dir(&directory).unwrap();
            let complete = all_shutdown_snapshot();
            let mut late_fault = complete.clone();
            late_fault.output_timing = Some(output_timing());
            late_fault.error = Some(AudioError::RuntimeUnavailable);
            let mut snapshots = [late_fault, complete.clone()].into_iter();
            let mut polls = 0;
            let mut evidence = Evidence::new(&directory).unwrap();
            let diagnostics = evidence.observe_shutdown_with(
                || snapshots.next().expect("bounded two-snapshot observation"),
                || false,
                || polls += 1,
            );
            assert_eq!(polls, 1);
            assert!(diagnostics
                .as_ref()
                .unwrap_err()
                .contains("RuntimeUnavailable"));
            assert!(!evidence.native_output_shutdown_proven);
            // execute() retains the active-run error before the diagnostics error.
            let original = Err::<(), _>("original active-run failure".to_string());
            assert_eq!(
                original.and(diagnostics).unwrap_err(),
                "original active-run failure"
            );
            drop(evidence);
            let records: Vec<Value> =
                fs::read_to_string(directory.join("runtime-native-timing.jsonl"))
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            assert_eq!(records.len(), 2);
            assert_eq!(records[1]["observation"], "post-stop-final");
            assert_eq!(records[1]["native_output_shutdown_observed"], true);
            assert_eq!(records[1]["native_output_shutdown_proven"], false);
            assert_eq!(records[1]["error"], Value::Null);

            let clean_directory = directory.join("clean");
            fs::create_dir(&clean_directory).unwrap();
            let mut clean = Evidence::new(&clean_directory).unwrap();
            clean
                .observe_shutdown_with(|| complete.clone(), || false, || panic!("already complete"))
                .unwrap();
            assert!(clean.native_output_shutdown_proven);
            drop(clean);

            let incomplete_directory = directory.join("incomplete");
            fs::create_dir(&incomplete_directory).unwrap();
            let mut incomplete = Evidence::new(&incomplete_directory).unwrap();
            let mut expired = complete;
            expired.output_timing = Some(output_timing());
            let failure = incomplete
                .observe_shutdown_with(|| expired.clone(), || true, || panic!("deadline expired"))
                .unwrap_err();
            assert!(failure.contains("shutdown was not proven"));
            assert!(!incomplete.native_output_shutdown_proven);
            drop(incomplete);
            fs::remove_dir_all(directory).unwrap();
        }

        #[test]
        fn terminal_snapshot_waits_for_earlier_cleanup_evidence_until_existing_deadline() {
            let directory = env::temp_dir().join(format!(
                "relink-output-shutdown-publication-test-{}-{}",
                std::process::id(),
                ticks()
            ));
            fs::create_dir(&directory).unwrap();
            let complete = all_shutdown_snapshot();
            let mut incomplete = complete.clone();
            let OutputTimingSnapshot::CoreAudio(output) =
                incomplete.output_timing.as_mut().unwrap();
            output.lifecycle[NativeOutputLifecyclePhase::StopReturned as usize] = None;
            assert!(native_shutdown_observed(&incomplete));
            assert!(!output_shutdown_proven(&incomplete));
            assert!(!output_shutdown_failed(&incomplete));
            assert!(!shutdown_observation_finished(&incomplete, false));

            let mut snapshots = [incomplete.clone(), complete].into_iter();
            let mut polls = 0;
            let mut evidence = Evidence::new(&directory).unwrap();
            evidence
                .observe_shutdown_with(
                    || snapshots.next().expect("two bounded observations"),
                    || false,
                    || polls += 1,
                )
                .unwrap();
            assert_eq!(polls, 1);
            assert!(evidence.native_output_shutdown_proven);
            drop(evidence);
            let records: Vec<Value> =
                fs::read_to_string(directory.join("runtime-native-timing.jsonl"))
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            assert_eq!(records.len(), 2);
            assert_eq!(records[0]["observation"], "post-stop");
            assert_eq!(records[0]["native_output_shutdown_observed"], true);
            assert_eq!(records[0]["native_output_shutdown_proven"], false);
            assert_eq!(records[1]["observation"], "post-stop-final");
            assert_eq!(records[1]["native_output_shutdown_proven"], true);

            let expired_directory = directory.join("expired");
            fs::create_dir(&expired_directory).unwrap();
            let mut expired = Evidence::new(&expired_directory).unwrap();
            let mut deadlines = [false, true].into_iter();
            let mut polls = 0;
            let failure = expired
                .observe_shutdown_with(
                    || incomplete.clone(),
                    || deadlines.next().expect("unchanged bounded deadline"),
                    || polls += 1,
                )
                .unwrap_err();
            assert_eq!(polls, 1);
            assert!(failure.contains("shutdown was not proven"));
            assert!(!expired.native_output_shutdown_proven);
            drop(expired);
            fs::remove_dir_all(directory).unwrap();
        }

        #[test]
        fn output_json_preserves_presentation_clock_bits_and_unavailable_lifecycle_fields() {
            let mut timing = output_timing();
            let OutputTimingSnapshot::CoreAudio(output) = &mut timing;
            output.last_packet = Some(NativeOutputPacket {
                callback_index: 8,
                epoch: 4,
                frames: 512,
                entry_host_time: u64::MAX - 3,
                now: NativeOutputStamp {
                    sample_time: f64::NAN,
                    host_time: u64::MAX - 4,
                    rate_scalar: 1.0,
                    flags: 7,
                },
                presentation: NativeOutputStamp {
                    sample_time: 1024.0,
                    host_time: u64::MAX - 1,
                    rate_scalar: 1.0,
                    flags: 7,
                },
            });
            output.lifecycle[NativeOutputLifecyclePhase::HandleCancelled as usize] =
                Some(NativeOutputLifecycleEvent {
                    phase: NativeOutputLifecyclePhase::HandleCancelled,
                    host_time: u64::MAX,
                    status: None,
                    callbacks_in_flight: None,
                });
            let value = output_timing_json(&timing);
            let decoded: Value = serde_json::from_str(&value.to_string()).unwrap();
            assert_eq!(decoded["last_packet"]["now"]["sample_time"], Value::Null);
            assert_eq!(
                decoded["last_packet"]["now"]["sample_time_bits"].as_u64(),
                Some(f64::NAN.to_bits())
            );
            assert_eq!(
                decoded["last_packet"]["presentation"]["host_time"].as_u64(),
                Some(u64::MAX - 1)
            );
            assert_eq!(decoded["lifecycle"][0]["callbacks_in_flight"], Value::Null);
            assert_eq!(decoded["lifecycle"][0]["status"], Value::Null);
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
        eprintln!("This fixed acceptance client requires macOS CoreAudio");
        std::process::exit(1);
    }
}
