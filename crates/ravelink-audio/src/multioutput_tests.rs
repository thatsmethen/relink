//! Offline controls exercise the same renderer construction/dispatch as native
//! owners. These tests never discover, open, or activate an audio device.
use super::*;
use crate::{
    capture_transport::Producer,
    output_sink::{InterleavedSink, InvalidOutputBuffer},
    tests::{COUNTING, OPERATIONS},
    *,
};
use ravelink_core::{Bus, Input, InputKind, Send, Session};
use std::sync::{mpsc, Mutex};

fn session(capture: bool) -> Session {
    let mut session = Session::new("Multi-output dispatcher controls", 48000);
    for index in 0..if capture { 2 } else { 1 } {
        session
            .add_input(Input::new(
                format!("Source {index}"),
                if capture {
                    InputKind::Hardware {
                        device_id: "CoreAudio:offline-capture".into(),
                    }
                } else {
                    InputKind::TestTone {
                        frequency_hz: 997.0,
                        amplitude_db: -18.0,
                    }
                },
            ))
            .unwrap();
    }
    session.add_bus(Bus::new("First")).unwrap();
    let mut second = Bus::new("Second");
    second.controls.set_gain_db(-6.0);
    session.add_bus(second).unwrap();
    for input in session.inputs.clone() {
        for bus in session.buses.clone() {
            session.add_send(Send::new(input.id, bus.id)).unwrap();
        }
    }
    session
}
fn pipeline(session: &Session) -> (OutputRenderer, Vec<OutputRenderer>, Arc<Telemetry>) {
    // Clock sits in the middle to exercise route index versus follower index.
    let mut render = crate::tests::render(session, session.buses[0].id, 2);
    let plan = OutputPlan {
        clock_output: OutputId("clock".into()),
        routes: vec![
            OutputRoute {
                output: OutputId("attenuated".into()),
                bus_id: session.buses[1].id,
            },
            OutputRoute {
                output: OutputId("clock".into()),
                bus_id: session.buses[0].id,
            },
            OutputRoute {
                output: OutputId("duplicate".into()),
                bus_id: session.buses[0].id,
            },
        ],
    };
    let gate = render.telemetry.gate.clone();
    let followers = configure(&mut render, &plan, 1, gate.clone(), GRAPH_BYTES).unwrap();
    gate.epoch.store(render.epoch, Ordering::Release);
    render.shared.active_gate.store(Some(gate.clone()));
    assert!(gate.activate());
    let telemetry = render.telemetry.clone();
    (
        OutputRenderer::from(render),
        followers
            .into_iter()
            .map(OutputRenderer::Follower)
            .collect(),
        telemetry,
    )
}
fn write(renderer: &mut OutputRenderer, pcm: &mut [f32]) -> RenderOutcome {
    renderer.write_to(&mut InterleavedSink {
        samples: pcm,
        channels: 2,
    })
}
fn primary(renderer: &mut OutputRenderer) -> &mut RenderState {
    match renderer {
        OutputRenderer::Primary(state) => state,
        _ => panic!("primary expected"),
    }
}

#[test]
fn real_dispatcher_renders_one_graph_and_routes_distinct_and_duplicate_buses() {
    let session = session(false);
    let (mut clock, mut followers, telemetry) = pipeline(&session);
    let mut pcm = [0.0f32; 512];
    let mut outputs = [[0.0f32; 512]; 2];
    for block in 0..30 {
        assert!(matches!(
            write(&mut clock, &mut pcm),
            RenderOutcome::Rendered { .. }
        ));
        for frame in 0..256 {
            let expected = ((block * 256 + frame) as f64 * std::f64::consts::TAU * 997.0 / 48000.0)
                .sin() as f32
                * 10.0f32.powf(-18.0 / 20.0);
            assert!((pcm[frame * 2] - expected).abs() < 2.0e-7);
        }
        if block < 5 {
            continue;
        }
        for (index, follower) in followers.iter_mut().enumerate() {
            let outcome = write(follower, &mut outputs[index]);
            assert!(
                matches!(
                    outcome,
                    RenderOutcome::Rendered {
                        accepted_version: None,
                        ..
                    }
                ) == (block == 5)
            );
            assert!(matches!(outcome, RenderOutcome::Rendered { .. }));
        }
        for frame in 0..256 {
            assert!(
                (outputs[0][frame * 2] - outputs[1][frame * 2] * 10.0f32.powf(-6.0 / 20.0)).abs()
                    < 2.0e-7
            );
            if block > 5 || frame >= 64 {
                let expected = (((block - 5) * 256 + frame) as f64 * std::f64::consts::TAU * 997.0
                    / 48000.0)
                    .sin() as f32
                    * 10.0f32.powf(-18.0 / 20.0);
                assert!(
                    (outputs[1][frame * 2] - expected).abs() < 2.0e-6,
                    "follower discarded/advanced source prefix"
                );
            }
        }
    }
    assert_eq!(telemetry.frames.load(Ordering::Acquire), 30 * 256);
    assert_eq!(telemetry.callbacks.load(Ordering::Acquire), 30);
    assert_eq!(
        telemetry.outputs[0].rendered.load(Ordering::Acquire),
        25 * 256
    );
    assert_eq!(
        telemetry.outputs[1].rendered.load(Ordering::Acquire),
        30 * 256
    );
    assert_eq!(
        telemetry.outputs[2].rendered.load(Ordering::Acquire),
        25 * 256
    );
    assert!(
        telemetry
            .outputs
            .iter()
            .all(|t| !t.ready.load(Ordering::Acquire)),
        "preparation is not native submission"
    );
}

#[test]
fn followers_never_consume_capture_or_advance_controls_and_graph() {
    let session = session(true);
    let (mut clock, mut followers, telemetry) = pipeline(&session);
    let render = primary(&mut clock);
    assert_eq!(
        render.captures.len(),
        1,
        "logical aliases must share one capture"
    );
    let ring = render.captures[0].consumer.ring.clone();
    let mut producer = Producer::new(ring.clone());
    for _ in 0..5 {
        producer.push_frames(256, |_| [0.125, -0.125]).unwrap();
    }
    let mut pcm = [0.0f32; 512];
    for _ in 0..8 {
        producer.push_frames(256, |_| [0.125, -0.125]).unwrap();
        assert!(matches!(
            write(&mut clock, &mut pcm),
            RenderOutcome::Rendered { .. }
        ));
        let buffered = ring.buffered.load(Ordering::Acquire);
        let frames = telemetry.frames.load(Ordering::Acquire);
        for follower in &mut followers {
            let _ = write(follower, &mut pcm);
        }
        assert_eq!(ring.buffered.load(Ordering::Acquire), buffered);
        assert_eq!(telemetry.frames.load(Ordering::Acquire), frames);
    }
    assert_eq!(ring.captured.load(Ordering::Acquire), 13 * 256);
    assert_eq!(telemetry.frames.load(Ordering::Acquire), 8 * 256);
}

fn captured_phase_leading_startup(rate: u32) {
    use std::time::Duration;

    let mut session = session(true);
    session.sample_rate = rate;
    let (mut clock, mut followers, telemetry) = pipeline(&session);
    let ring = primary(&mut clock).captures[0].consumer.ring.clone();
    let mut producer = Producer::new(ring.clone());
    let mut pcm = vec![0.0f32; MAX_CALLBACK_FRAMES * 2];
    let period = Duration::from_secs_f64(MAX_CALLBACK_FRAMES as f64 / f64::from(rate));
    let mut first_clock = None;
    let mut first_followers = vec![None; followers.len()];

    // The capture packet arrives immediately after the graph owner's callback;
    // followers run immediately before it. These independent native phases are
    // valid even when every device progresses at exactly its nominal rate.
    // Drive the actual capture consumer, mixer, fanout and follower FIR with a
    // logical clock, without sleeping or opening devices.
    for callback in 1..=7 {
        let follower_at = period * callback - Duration::from_millis(2);
        for (index, follower) in followers.iter_mut().enumerate() {
            match write(follower, &mut pcm) {
                RenderOutcome::Silence => {
                    assert!(
                        callback < 7,
                        "healthy follower still unready at rate {rate}"
                    );
                    assert!(pcm.iter().all(|sample| *sample == 0.0));
                }
                RenderOutcome::Rendered {
                    frames,
                    accepted_version,
                } => {
                    assert_eq!(callback, 7);
                    assert_eq!(frames, MAX_CALLBACK_FRAMES);
                    assert_eq!(accepted_version, None, "initial FIR support is versionless");
                    assert!(pcm.iter().any(|sample| *sample != 0.0));
                    first_followers[index] = Some(follower_at);
                    follower.permit().submitted(frames, accepted_version);
                }
            }
            assert!(telemetry.fault.snapshot().is_none(), "rate {rate}");
        }

        let clock_at = period * callback - Duration::from_millis(1);
        match write(&mut clock, &mut pcm) {
            RenderOutcome::Silence => {
                assert!(callback < 4, "healthy clock still unready at rate {rate}");
                assert!(pcm.iter().all(|sample| *sample == 0.0));
            }
            RenderOutcome::Rendered {
                frames,
                accepted_version,
            } => {
                assert!(callback >= 4);
                assert_eq!(frames, MAX_CALLBACK_FRAMES);
                assert_eq!(accepted_version, Some(0));
                first_clock.get_or_insert(clock_at);
                clock.permit().submitted(frames, accepted_version);
            }
        }
        assert!(
            telemetry.fault.snapshot().is_none(),
            "valid capture priming was rejected at rate {rate}, callback {callback}"
        );
        producer
            .push_frames(MAX_CALLBACK_FRAMES, |_| [0.125, -0.125])
            .unwrap();
    }

    let first_clock = first_clock.expect("capture feeds clock by fourth callback");
    assert_eq!(first_clock, period * 4 - Duration::from_millis(1));
    assert!(first_clock < crate::output_progress::program_deadline(rate, true, false));
    for first in first_followers {
        let first = first.expect("follower receives its delayed program prefix");
        assert_eq!(first, period * 7 - Duration::from_millis(2));
        assert!(first > crate::output_progress::deadline(rate));
        assert!(first < crate::output_progress::program_deadline(rate, true, true));
    }
    assert!(telemetry
        .outputs
        .iter()
        .all(|output| output.ready.load(Ordering::Acquire)));
    assert_eq!(telemetry.callbacks.load(Ordering::Acquire), 4);
    assert_eq!(
        telemetry.frames.load(Ordering::Acquire),
        4 * MAX_CALLBACK_FRAMES as u64
    );
    assert_eq!(
        ring.captured.load(Ordering::Acquire),
        7 * MAX_CALLBACK_FRAMES as u64
    );
}

#[test]
fn captured_maximum_quantum_startup_8khz_preserves_healthy_independent_phases() {
    captured_phase_leading_startup(8_000);
}

#[test]
fn captured_maximum_quantum_startup_44100hz_preserves_healthy_independent_phases() {
    captured_phase_leading_startup(44_100);
}

#[test]
fn captured_maximum_quantum_startup_48khz_preserves_healthy_independent_phases() {
    captured_phase_leading_startup(48_000);
}

struct FailingSink {
    pcm: Vec<f32>,
    fail_at: usize,
}
impl StereoSink for FailingSink {
    fn frames(&self) -> Result<usize, InvalidOutputBuffer> {
        Ok(self.pcm.len() / 2)
    }
    fn clear(&mut self) {
        self.pcm.fill(0.0);
    }
    fn write_stereo(
        &mut self,
        start: usize,
        left: &[f32],
        right: &[f32],
    ) -> Result<(), InvalidOutputBuffer> {
        if start == self.fail_at {
            return Err(InvalidOutputBuffer);
        }
        for (index, (&left, &right)) in left.iter().zip(right).enumerate() {
            self.pcm[(start + index) * 2] = left;
            self.pcm[(start + index) * 2 + 1] = right;
        }
        Ok(())
    }
}
#[test]
fn failed_final_chunk_publishes_no_route_or_ack_and_silences_every_owner() {
    let mut session = session(false);
    let (mut clock, mut followers, telemetry) = pipeline(&session);
    session.buses[0].controls.set_gain_db(-3.0);
    primary(&mut clock)
        .bank
        .publish(&ravelink_core::PreparedControls::compile(&session, GRAPH_BYTES).unwrap())
        .unwrap();
    let mut sink = FailingSink {
        pcm: vec![9.0; RENDER_CHUNK * 4],
        fail_at: RENDER_CHUNK,
    };
    assert_eq!(clock.write_to(&mut sink), RenderOutcome::Silence);
    assert!(sink.pcm.iter().all(|value| *value == 0.0));
    assert_eq!(
        telemetry.gate.fanout_control().unwrap().committed_frames(),
        0
    );
    assert_eq!(telemetry.applied_control_version.load(Ordering::Acquire), 0);
    assert_eq!(telemetry.frames.load(Ordering::Acquire), 0);
    for follower in &mut followers {
        let mut pcm = [9.0; 512];
        assert_eq!(write(follower, &mut pcm), RenderOutcome::Silence);
        assert_eq!(pcm, [0.0; 512]);
    }
}

#[test]
fn full_follower_preflight_happens_before_advancing_the_actual_graph() {
    let session = session(false);
    let (mut clock, _followers, telemetry) = pipeline(&session);
    let mut pcm = vec![0.0f32; MAX_CALLBACK_FRAMES * 2];
    for _ in 0..4 {
        assert!(matches!(
            write(&mut clock, &mut pcm),
            RenderOutcome::Rendered { .. }
        ));
    }
    assert_eq!(write(&mut clock, &mut pcm), RenderOutcome::Silence);
    assert_eq!(telemetry.frames.load(Ordering::Acquire), 65536);
    let fault = telemetry.fault.snapshot().unwrap();
    assert_eq!(fault.origin, FaultOrigin::OutputFanoutProducer);
    assert_eq!(fault.endpoint_index, Some(0));
    let render = primary(&mut clock);
    let bus = render.mixer.resolve_bus(session.buses[0].id).unwrap();
    render
        .mixer
        .render_sources(std::iter::empty(), &mut [], 1)
        .unwrap();
    let next = render.mixer.rendered_bus(bus).unwrap()[0][0];
    let expected = (65536.0f64 * std::f64::consts::TAU * 997.0 / 48000.0).sin() as f32
        * 10.0f32.powf(-18.0 / 20.0);
    assert!((next - expected).abs() < 2.0e-7);
}

#[test]
fn dispatcher_initial_priming_program_and_fault_callbacks_allocate_nothing() {
    let session = session(false);
    let (mut clock, mut followers, telemetry) = pipeline(&session);
    let mut pcm = [0.0f32; 512];
    OPERATIONS.with(|n| n.set(0));
    COUNTING.with(|on| on.set(true));
    for _ in 0..12 {
        let _ = write(&mut clock, &mut pcm);
        for follower in &mut followers {
            let _ = write(follower, &mut pcm);
        }
    }
    telemetry.gate.cancel();
    let _ = write(&mut clock, &mut pcm);
    for follower in &mut followers {
        let _ = write(follower, &mut pcm);
    }
    COUNTING.with(|on| on.set(false));
    assert_eq!(OPERATIONS.with(|n| n.get()), 0);
}

#[test]
fn versionless_pcm_is_submitted_without_inventing_a_device_ack() {
    let telemetry = OutputTelemetry::default();
    telemetry.submitted(256, None);
    assert!(telemetry.ready.load(Ordering::Acquire));
    assert_eq!(telemetry.submitted.load(Ordering::Acquire), 256);
    assert!(!telemetry.submitted_version_present.load(Ordering::Acquire));
    telemetry.submitted(256, Some(0));
    assert!(telemetry.submitted_version_present.load(Ordering::Acquire));
    assert_eq!(telemetry.submitted_version.load(Ordering::Acquire), 0);
    let wasapi = crate::wasapi_output_state::WasapiOutputTiming::new();
    wasapi.submitted_pcm(256, true, None, false, 0);
    let snapshot = wasapi.snapshot();
    assert_eq!(snapshot.submitted_pcm_frames, 256);
    assert_eq!(snapshot.submitted_silent_frames, 0);
    assert_eq!(snapshot.submitted_version, None);
}

#[test]
fn emergency_cancel_during_partial_render_blocks_publication_without_control_lock() {
    struct PausedSink {
        pcm: Vec<f32>,
        entered: mpsc::SyncSender<()>,
        resume: mpsc::Receiver<()>,
    }
    impl StereoSink for PausedSink {
        fn frames(&self) -> Result<usize, InvalidOutputBuffer> {
            Ok(self.pcm.len() / 2)
        }
        fn clear(&mut self) {
            self.pcm.fill(0.0);
        }
        fn write_stereo(
            &mut self,
            start: usize,
            left: &[f32],
            right: &[f32],
        ) -> Result<(), InvalidOutputBuffer> {
            for (i, (&a, &b)) in left.iter().zip(right).enumerate() {
                self.pcm[2 * (start + i)] = a;
                self.pcm[2 * (start + i) + 1] = b;
            }
            if start == RENDER_CHUNK {
                self.entered.send(()).unwrap();
                self.resume.recv().unwrap();
            }
            Ok(())
        }
    }
    let (mut clock, _followers, telemetry) = pipeline(&session(false));
    let shared = primary(&mut clock).shared.clone();
    let (commands, _receiver) = mpsc::sync_channel(1);
    commands.send(crate::Command::Shutdown).unwrap();
    let runtime = AudioRuntime {
        commands,
        shared: shared.clone(),
    };
    let (entered, reached) = mpsc::sync_channel(1);
    let (resume, resumed) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let mut sink = PausedSink {
            pcm: vec![0.0; 4 * RENDER_CHUNK],
            entered,
            resume: resumed,
        };
        let result = clock.write_to(&mut sink);
        (result, sink.pcm)
    });
    reached
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let held = shared.state();
    runtime.cancel_output();
    assert!(telemetry.gate.fanout_control().unwrap().cancelled());
    resume.send(()).unwrap();
    let (outcome, pcm) = worker.join().unwrap();
    drop(held);
    assert_eq!(outcome, RenderOutcome::Silence);
    assert!(pcm.iter().all(|x| *x == 0.0));
    assert_eq!(
        telemetry.gate.fanout_control().unwrap().committed_frames(),
        0
    );
    assert_eq!(telemetry.applied_control_version.load(Ordering::Acquire), 0);
}

#[test]
fn paused_old_slot_reader_never_cancels_or_reclaims_the_new_epoch() {
    let shared = Arc::new(Shared {
        active_gate: arc_swap::ArcSwapOption::empty(),
        state: Mutex::new(ControlState::default()),
        epoch: AtomicU64::new(0),
    });
    let old = Arc::new(EpochGate::new(Arc::new(AtomicBool::new(true))));
    old.epoch.store(1, Ordering::Release);
    let weak = Arc::downgrade(&old);
    shared.active_gate.store(Some(old));
    let (entered, reached) = mpsc::sync_channel(1);
    let (resume, resumed) = mpsc::sync_channel(1);
    let read = shared.clone();
    let worker = std::thread::spawn(move || {
        let guard = read.active_gate.load();
        entered.send(()).unwrap();
        resumed.recv().unwrap();
        guard.as_ref().unwrap().cancel();
    });
    reached.recv().unwrap();
    let next = Arc::new(EpochGate::new(Arc::new(AtomicBool::new(true))));
    next.epoch.store(2, Ordering::Release);
    shared.active_gate.store(Some(next.clone()));
    assert!(
        weak.upgrade().is_some(),
        "guard owns the retired epoch while paused"
    );
    resume.send(()).unwrap();
    worker.join().unwrap();
    assert!(next.wanted.load(Ordering::Acquire));
    assert!(
        weak.upgrade().is_none(),
        "old epoch becomes reclaimable after guard exits"
    );
}

#[test]
fn excessive_physical_routes_are_rejected_before_discovery_and_not_a_logical_channel_cap() {
    let mut plan = OutputPlan::single(OutputId("invalid-would-fail-syntax".into()), Uuid::new_v4());
    plan.routes = vec![plan.routes[0].clone(); 64];
    let runtime = AudioRuntime::new().unwrap();
    let error = runtime
        .start_outputs(session(false), plan.clone())
        .unwrap_err();
    assert!(
        matches!(error, AudioError::InvalidSession(ref text) if text.contains("memory budget"))
    );
    assert!(runtime.snapshot().configuration.is_none());
    assert!(validate_plan_budget(&plan).is_err());
    plan.routes.truncate(1);
    assert!(validate_plan_budget(&plan).is_ok());
}

#[test]
fn a_fresh_epoch_has_no_cancelled_pcm_or_fir_ack_history() {
    let session = session(false);
    let (mut clock, _followers, old) = pipeline(&session);
    let mut pcm = [0.0f32; 512];
    for _ in 0..6 {
        let _ = write(&mut clock, &mut pcm);
    }
    old.gate.cancel();
    let (mut next, mut followers, fresh) = pipeline(&session);
    assert_eq!(fresh.gate.fanout_control().unwrap().committed_frames(), 0);
    assert!(!fresh.gate.fanout_control().unwrap().cancelled());
    assert_eq!(write(&mut followers[0], &mut pcm), RenderOutcome::Silence);
    for _ in 0..6 {
        let _ = write(&mut next, &mut pcm);
    }
    assert!(matches!(
        write(&mut followers[0], &mut pcm),
        RenderOutcome::Rendered {
            accepted_version: None,
            ..
        }
    ));
    assert!(!fresh.outputs[0]
        .submitted_version_present
        .load(Ordering::Acquire));
}

#[test]
fn follower_underflow_cancels_every_route_and_snapshot_keeps_exact_endpoint() {
    let (mut clock, mut followers, telemetry) = pipeline(&session(false));
    let mut pcm = [0.0f32; 512];
    for _ in 0..6 {
        let _ = write(&mut clock, &mut pcm);
    }
    for _ in 0..8 {
        let _ = write(&mut followers[0], &mut pcm);
    }
    let first = telemetry.fault.snapshot().unwrap();
    assert_eq!(first.origin, FaultOrigin::OutputFanoutConsumer);
    assert_eq!(first.endpoint_index, Some(0));
    assert!(telemetry.gate.fanout_control().unwrap().cancelled());
    assert_eq!(write(&mut clock, &mut pcm), RenderOutcome::Silence);
    assert_eq!(write(&mut followers[1], &mut pcm), RenderOutcome::Silence);
    let shared = primary(&mut clock).shared.clone();
    let routes: Vec<_> = ["attenuated", "clock", "duplicate"]
        .into_iter()
        .map(|id| RunningOutput {
            output: OutputDevice {
                id: OutputId(id.into()),
                name: id.into(),
            },
            bus_id: Uuid::new_v4(),
            sample_rate: 48000,
            device_channels: 2,
            sample_format: "F32".into(),
        })
        .collect();
    let clock_route = &routes[1];
    let configuration = RunningConfiguration {
        output: clock_route.output.clone(),
        inputs: Vec::new(),
        bus_id: clock_route.bus_id,
        sample_rate: 48000,
        device_channels: 2,
        sample_format: "F32".into(),
        outputs: routes,
        clock_output_index: 1,
    };
    {
        let mut state = shared.state();
        state.configuration = Some(configuration);
        state.telemetry = Some(telemetry);
        state.output_diagnostics = (0..3)
            .map(|_| {
                #[cfg(target_os = "macos")]
                {
                    crate::output::Diagnostics::CoreAudio(Arc::new(
                        crate::native_output_timing::NativeOutputTiming::new(),
                    ))
                }
                #[cfg(target_os = "windows")]
                {
                    crate::output::Diagnostics::Wasapi(Arc::new(
                        crate::wasapi_output_state::WasapiOutputTiming::new(),
                    ))
                }
            })
            .collect();
    }
    let (commands, _receiver) = mpsc::sync_channel(1);
    let runtime = AudioRuntime { commands, shared };
    let snapshot = runtime.snapshot();
    assert_eq!(
        snapshot.first_fault.unwrap().endpoint_id.as_deref(),
        Some("attenuated")
    );
    assert_eq!(snapshot.outputs.len(), 3);
    assert_eq!(snapshot.outputs[1].output.0, "clock");
    assert!(snapshot.outputs[1].is_clock);
    assert!(!snapshot.outputs[0].is_clock);
    assert!(snapshot.output_timing.is_some());
}
