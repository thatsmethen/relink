use super::*;
use capture_transport::{Consumer, Kernel, Producer, Ring, TransportError};
use relink_core::{Bus, DspConfig, Input, Send};
use std::cell::Cell;

fn input_session(device: &str) -> (Session, Uuid) {
    let mut session = Session::new("Captured PCM fixture", 48000);
    let input = Input::new(
        "Input",
        InputKind::Hardware {
            device_id: device.into(),
        },
    );
    let id = input.id;
    session.add_input(input).unwrap();
    let mut bus = Bus::new("Output");
    bus.dsp = DspConfig::default();
    let bus_id = bus.id;
    session.add_bus(bus).unwrap();
    session.add_send(Send::new(id, bus_id)).unwrap();
    (session, bus_id)
}
fn native_test_id() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "CoreAudio:fixture"
    }
    #[cfg(target_os = "windows")]
    {
        "WASAPI:fixture"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "ALSA:fixture"
    }
}

#[test]
fn controls_are_acknowledged_only_after_rendering_not_priming_or_underflow() {
    let (session, bus) = input_session(native_test_id());
    let mut render = tests::render(&session, bus, 2);
    let mut producer = Producer::new(render.captures[0].consumer.ring.clone());
    let mut changed = session.clone();
    changed.buses[0].controls.set_gain_db(-6.0);
    let first = render
        .bank
        .publish(&prepare_live_update(&session, &changed).unwrap())
        .unwrap();
    let mut output = [9.0_f32; 128];
    render.write(&mut output);
    assert_eq!(output, [0.0; 128]);
    assert_eq!(
        render.applied_version, first,
        "target is prepared while priming"
    );
    assert_eq!(
        render
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        0
    );

    // This request replaces the first before any program frame was rendered.
    // The eventual watermark must not imply that the intermediate target played.
    changed.buses[0].controls.set_gain_db(-12.0);
    let latest = render
        .bank
        .publish(&prepare_live_update(&session, &changed).unwrap())
        .unwrap();
    render.write(&mut output);
    assert_eq!(
        render
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        0
    );
    for _ in 0..5 {
        producer.push_frames(256, |_| [0.25, -0.25]).unwrap();
    }
    render.write(&mut output);
    assert_eq!(
        render
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        latest
    );
    assert!(
        output[0] > 0.2,
        "acknowledgment does not claim the 5 ms ramp has completed"
    );
    assert_eq!(render.telemetry.frames.load(Ordering::Relaxed), 64);

    changed.buses[0].controls.set_gain_db(-18.0);
    let rejected = render
        .bank
        .publish(&prepare_live_update(&session, &changed).unwrap())
        .unwrap();
    let mut too_much_output = [9.0_f32; 8192];
    render.write(&mut too_much_output);
    assert_eq!(render.applied_version, rejected);
    assert_eq!(
        render.telemetry.fault.load(Ordering::Acquire),
        AudioFault::CaptureUnderflow as u32
    );
    assert!(too_much_output.iter().all(|sample| *sample == 0.0));
    assert_eq!(
        render
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        latest
    );
    assert_eq!(render.telemetry.frames.load(Ordering::Relaxed), 64);
}

fn transport() -> (Producer, Consumer) {
    let ring = Ring::new();
    (
        Producer::new(ring.clone()),
        Consumer::new(ring, Kernel::new(), 48000),
    )
}
fn signal(frame: f64, hz: f64) -> f32 {
    ((frame * std::f64::consts::TAU * hz / 48000.0).sin() * 0.25) as f32
}
fn pcm(start: usize, frames: usize) -> Vec<f32> {
    (start..start + frames)
        .flat_map(|f| [signal(f as f64, 440.0), signal(f as f64, 997.0)])
        .collect()
}

#[test]
fn input_configuration_is_exact_rate_and_prefers_stereo_then_mono() {
    use tests::config;
    let selected = capture::select_config(
        vec![
            config(1, cpal::SampleFormat::F32, 48000, 48000),
            config(8, cpal::SampleFormat::F32, 48000, 48000),
            config(2, cpal::SampleFormat::I16, 48000, 48000),
        ]
        .into_iter(),
        48000,
    )
    .unwrap();
    assert_eq!(selected.channels(), 2);
    let mono = capture::select_config(
        vec![config(1, cpal::SampleFormat::F32, 48000, 48000)].into_iter(),
        48000,
    )
    .unwrap();
    assert_eq!(mono.channels(), 1);
    assert!(capture::select_config(
        vec![config(2, cpal::SampleFormat::F32, 44100, 44100)].into_iter(),
        48000
    )
    .is_none());
    assert!(capture::select_config(
        vec![config(0, cpal::SampleFormat::F32, 48000, 48000)].into_iter(),
        48000
    )
    .is_none());
}

#[test]
fn malformed_input_identity_is_rejected_without_any_device_discovery() {
    let (session, bus) = input_session("not-a-native-id");
    let runtime = AudioRuntime::new().unwrap();
    assert!(matches!(
        runtime.start(session, bus, OutputId("not-a-native-id".into())),
        Err(AudioError::InvalidInputId(_))
    ));
    assert!(!runtime.snapshot().running);
    assert!(runtime.snapshot().configuration.is_none());
}

#[test]
fn capture_mapping_sanitizes_float_duplicates_mono_and_ignores_extra_channels() {
    let ring = Ring::new();
    let mut producer = Producer::new(ring.clone());
    producer
        .push(&[f32::NAN, f32::INFINITY, -0.25, 0.75, 0.125, 2.0], 3)
        .unwrap();
    assert_eq!(ring.test_sample(0), [0.0, 0.0]);
    assert_eq!(ring.test_sample(1), [0.75, 0.125]);
    producer.push(&[i16::MIN, 0_i16, i16::MAX], 1).unwrap();
    for i in 2..5 {
        let sample = ring.test_sample(i);
        assert_eq!(sample[0], sample[1]);
    }
    assert_eq!(ring.test_sample(2), [-1.0, -1.0]);
    assert_eq!(ring.test_sample(3), [0.0, 0.0]);
    assert!(ring.test_sample(4)[0] > 0.9999);
}

#[test]
fn bandlimited_reader_matches_independent_stereo_pcm_through_wraparound() {
    let (mut producer, mut consumer) = transport();
    // Largest packet is 256, so target is 1024. Prefill exactly 1280 frames.
    for block in 0..5 {
        producer.push(&pcm(block * 256, 256), 2).unwrap();
    }
    let mut left = [0.0; 256];
    let mut right = [0.0; 256];
    for block in 0..600 {
        assert!(consumer.prepare(256).unwrap());
        consumer.read(&mut left, &mut right);
        for i in 0..256 {
            let frame = 256 + block * 256 + i;
            assert!(
                (left[i] - signal(frame as f64, 440.0)).abs() < 0.000003,
                "left frame {frame}"
            );
            assert!(
                (right[i] - signal(frame as f64, 997.0)).abs() < 0.000003,
                "right frame {frame}"
            );
        }
        producer.push(&pcm(1280 + block * 256, 256), 2).unwrap();
    }
    assert_eq!(
        consumer.ring.correction_ppm.load(Ordering::Relaxed),
        0_f64.to_bits()
    );
}

#[test]
fn capture_and_mixer_callbacks_allocate_nothing_and_share_one_input_stream() {
    let (mut session, bus) = input_session(native_test_id());
    let duplicate = Input::new(
        "Same capture",
        InputKind::Virtual {
            endpoint_id: native_test_id().into(),
        },
    );
    let id = duplicate.id;
    session.add_input(duplicate).unwrap();
    session.add_send(Send::new(id, bus)).unwrap();
    assert_eq!(capture::source_groups(&session).len(), 1);
    let mut render = tests::render(&session, bus, 2);
    assert_eq!(render.captures.len(), 1);
    let ring = render.captures[0].consumer.ring.clone();
    let mut capture = CaptureState::new(
        ring,
        0,
        2,
        0,
        render.wanted.clone(),
        render.shared.clone(),
        render.telemetry.clone(),
    );
    let input = [0.125_f32; 512];
    let mut output = [0.0_f32; 512];
    tests::OPERATIONS.with(|n| n.set(0));
    tests::COUNTING.with(|on| on.set(true));
    for _ in 0..5 {
        capture.push(&input);
    }
    for _ in 0..20 {
        render.write(&mut output);
        capture.push(&input);
    }
    tests::COUNTING.with(|on| on.set(false));
    assert_eq!(tests::OPERATIONS.with(Cell::get), 0);
    assert!(output.iter().all(|v| (*v - 0.25).abs() < 0.000001));
    assert_eq!(render.telemetry.fault.load(Ordering::Acquire), 0);
}

#[test]
fn underflow_overflow_and_invalid_capture_packets_are_explicit_faults() {
    let (mut producer, mut consumer) = transport();
    let block = [0.125_f32; 512];
    for _ in 0..5 {
        producer.push(&block, 2).unwrap();
    }
    let mut left = [0.0_f32; 256];
    let mut right = left;
    for _ in 0..3 {
        assert!(consumer.prepare(256).unwrap());
        consumer.read(&mut left, &mut right);
    }
    assert_eq!(consumer.prepare(256), Err(TransportError::Underflow));
    let ring = Ring::new();
    let mut producer = Producer::new(ring);
    let full = vec![0.0_f32; MAX_CALLBACK_FRAMES * 2];
    for _ in 0..4 {
        producer.push(&full, 2).unwrap();
    }
    assert_eq!(producer.push(&[0.0, 0.0], 2), Err(TransportError::Overflow));
    assert_eq!(
        producer.push(&[0.0, 0.0, 0.0], 2),
        Err(TransportError::InvalidBuffer)
    );
    assert_eq!(
        producer.push::<f32>(&[], 0),
        Err(TransportError::InvalidBuffer)
    );
}

#[test]
fn cancelled_capture_cannot_publish_and_new_start_has_no_stale_audio() {
    let (session, bus) = input_session(native_test_id());
    let render = tests::render(&session, bus, 2);
    let ring = render.captures[0].consumer.ring.clone();
    let mut state = CaptureState::new(
        ring.clone(),
        0,
        2,
        0,
        render.wanted.clone(),
        render.shared.clone(),
        render.telemetry.clone(),
    );
    render.shared.epoch.fetch_add(1, Ordering::AcqRel);
    state.push(&[0.25_f32; 512]);
    assert_eq!(ring.captured.load(Ordering::Relaxed), 0);
    let mut restarted = tests::render(&session, bus, 2);
    let mut out = [9.0_f32; 512];
    restarted.write(&mut out);
    assert!(out.iter().all(|s| *s == 0.0));
    assert_eq!(
        restarted.telemetry.captures[0]
            .1
            .captured
            .load(Ordering::Relaxed),
        0
    );
}

#[test]
fn missing_capture_faults_after_bounded_startup_silence() {
    let (session, bus) = input_session(native_test_id());
    let mut render = tests::render(&session, bus, 2);
    let mut output = [9.0_f32; 1024];
    for _ in 0..200 {
        render.write(&mut output);
    }
    assert!(output.iter().all(|s| *s == 0.0));
    assert_eq!(
        AudioFault::from_code(render.telemetry.fault.load(Ordering::Acquire)),
        AudioFault::CaptureStartup
    );
    let first = render.telemetry.fault.snapshot().unwrap();
    assert_eq!(first.origin, FaultOrigin::CaptureStartup);
    assert_eq!(first.direction, FaultDirection::Input);
    assert_eq!(first.endpoint_index, Some(0));
}

#[test]
fn independent_clock_drift_preserves_continuity_without_drops() {
    for ppm in [-1000.0_f64, 1000.0] {
        let (mut producer, mut consumer) = transport();
        let mut generated = 0;
        for _ in 0..5 {
            producer.push(&pcm(generated, 256), 2).unwrap();
            generated += 256;
        }
        let mut source_clock = generated as f64;
        let mut previous = [0.0_f32; 2];
        let mut worst_step = [0.0_f32; 2];
        let mut left = [0.0; 256];
        let mut right = left;
        // 60 seconds, independent input clock +/-1,000 ppm.
        for callback in 0..11250 {
            assert!(
                consumer.prepare(256).unwrap(),
                "ppm={ppm} callback={callback}"
            );
            consumer.read(&mut left, &mut right);
            for frame in 0..256 {
                let sample = [left[frame], right[frame]];
                if callback > 0 {
                    for side in 0..2 {
                        worst_step[side] =
                            worst_step[side].max((sample[side] - previous[side]).abs());
                    }
                }
                previous = sample;
            }
            source_clock += 256.0 * (1.0 + ppm / 1_000_000.0);
            let frames = source_clock.floor() as usize - generated;
            producer.push(&pcm(generated, frames), 2).unwrap();
            generated += frames;
        }
        let correction = f64::from_bits(consumer.ring.correction_ppm.load(Ordering::Relaxed));
        assert!(
            (correction - ppm).abs() < 80.0,
            "requested={ppm} measured={correction}"
        );
        assert!(worst_step[0] < 0.015, "left max step {}", worst_step[0]);
        assert!(worst_step[1] < 0.033, "right max step {}", worst_step[1]);
        let occupancy = consumer.ring.buffered.load(Ordering::Relaxed);
        assert!((900..1150).contains(&occupancy), "occupancy={occupancy}");
    }
}

#[test]
fn capture_owner_budget_rejects_overflow_and_is_added_to_render_cost_before_allocation() {
    assert_eq!(
        capture::checked_owner_bytes([Some(7), Some(9)]).unwrap(),
        16
    );
    assert!(capture::checked_owner_bytes([Some(usize::MAX), Some(1)]).is_err());
    assert!(capture::checked_owner_bytes([Some(7), usize::MAX.checked_mul(2)]).is_err());
    assert_eq!(capture::selected_owner_bytes(&[]).unwrap(), 0);
    let (session, bus) = input_session(native_test_id());
    let runtime = AudioRuntime::new().unwrap();
    let make = |output_bytes, capture_owner_bytes| {
        RenderState::new_with_owner_budget(
            &session,
            bus,
            2,
            0,
            Arc::new(AtomicBool::new(true)),
            runtime.shared.clone(),
            output_bytes,
            capture_owner_bytes,
        )
    };
    assert!(make(0, GRAPH_BYTES).is_err());
    assert!(make(0, usize::MAX).is_err());
    assert!(make(GRAPH_BYTES / 2, GRAPH_BYTES / 2).is_err());
    // Reserve just enough for everything allocated by the zero-owner graph;
    // adding even one owner stack must reject instead of omitting that charge.
    let mut low = 0;
    let mut high = GRAPH_BYTES;
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if make(0, middle).is_ok() {
            low = middle;
        } else {
            high = middle;
        }
    }
    assert!(make(0, low).is_ok());
    assert!(make(capture::OWNER_STACK_BYTES, low).is_err());
    assert!(make(4 * 1024 * 1024, capture::OWNER_STACK_BYTES).is_ok());
    assert!(!runtime.snapshot().running);
    assert!(runtime.snapshot().output_timing.is_none());
}

#[test]
fn application_processes_are_summed_before_one_logical_limiter_and_live_mute() {
    let (mut session, bus) = input_session(native_test_id());
    session.inputs[0].kind = InputKind::Application {
        application_id: ApplicationSelector::MacBundle {
            bundle_id: "org.example.Player".into(),
            path: "/Applications/Player.app".into(),
        }
        .encode()
        .unwrap(),
    };
    session.inputs[0].dsp.limiter = Some(relink_core::LimiterConfig {
        ceiling_db: -6.0,
        release_ms: 50.0,
    });
    let shared = Arc::new(Shared {
        active_gate: arc_swap::ArcSwapOption::empty(),
        state: Mutex::new(ControlState::default()),
        epoch: AtomicU64::new(0),
    });
    let mut render = RenderState::new_with_capture_plan(
        &session,
        bus,
        2,
        0,
        Arc::new(AtomicBool::new(true)),
        shared,
        0,
        0,
        vec![
            InputId("process-root-a".into()),
            InputId("process-root-b".into()),
        ],
        vec![CaptureGroupPlan {
            source_indices: vec![0, 1],
            input_ids: vec![session.inputs[0].id],
        }],
    )
    .unwrap();
    let mut producers: Vec<_> = render
        .captures
        .iter()
        .map(|capture| Producer::new(capture.consumer.ring.clone()))
        .collect();
    for producer in &mut producers {
        for _ in 0..5 {
            producer.push_frames(256, |_| [0.4, -0.4]).unwrap();
        }
    }
    let mut pcm = [0_f32; 512];
    render.write(&mut pcm);
    // Two independent per-process limiters would produce 0.8 here. The one
    // logical channel must limit the complete 0.8 sum to the -6 dB ceiling.
    let ceiling = 10_f32.powf(-6.0 / 20.0);
    for frame in pcm.chunks_exact(2) {
        assert!((frame[0] - ceiling).abs() < 0.00001, "{}", frame[0]);
        assert!((frame[1] + ceiling).abs() < 0.00001);
    }
    assert_eq!(render.telemetry.fault.load(Ordering::Acquire), 0);
    session.inputs[0].controls.muted = true;
    let version = render
        .bank
        .publish(&PreparedControls::compile(&session, GRAPH_BYTES).unwrap())
        .unwrap();
    for producer in &mut producers {
        producer.push_frames(256, |_| [0.4, -0.4]).unwrap();
    }
    render.write(&mut pcm);
    assert_eq!(
        render
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        version
    );
    assert!(pcm[480..].iter().all(|sample| *sample == 0.0));
}

#[test]
fn incomplete_or_duplicated_capture_selection_is_rejected_before_owners_start() {
    let (session, _) = input_session(native_test_id());
    let id = session.inputs[0].id;
    let one = vec![InputId("one".into())];
    let group = CaptureGroupPlan {
        source_indices: vec![0],
        input_ids: vec![id],
    };
    validate_capture_layout(&session, &one, std::slice::from_ref(&group)).unwrap();
    assert!(validate_capture_layout(&session, &one, &[]).is_err());
    assert!(validate_capture_layout(&session, &[], std::slice::from_ref(&group)).is_err());
    assert!(validate_capture_layout(
        &session,
        &[InputId("one".into()), InputId("one".into())],
        &[CaptureGroupPlan {
            source_indices: vec![0, 1],
            input_ids: vec![id]
        }]
    )
    .is_err());
    assert!(validate_capture_layout(
        &session,
        &[InputId("one".into()), InputId("unused".into())],
        &[group]
    )
    .is_err());
    assert!(validate_capture_layout(
        &session,
        &one,
        &[CaptureGroupPlan {
            source_indices: vec![0],
            input_ids: vec![Uuid::new_v4()]
        }]
    )
    .is_err());
}
