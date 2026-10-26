use super::*;
use ravelink_core::{Bus, DspConfig, Input, Send};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

struct CountingAllocator;
thread_local! {
    pub(super) static COUNTING: Cell<bool> = const { Cell::new(false) };
    pub(super) static OPERATIONS: Cell<usize> = const { Cell::new(0) };
}
fn allocation_event() {
    let _ = COUNTING.try_with(|on| {
        if on.get() {
            OPERATIONS.with(|n| n.set(n.get() + 1));
        }
    });
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocation_event();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocation_event();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocation_event();
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        allocation_event();
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn session() -> (Session, Uuid) {
    let mut session = Session::new("Offline transport fixture", 48000);
    let input = Input::new(
        "440 Hz",
        InputKind::TestTone {
            frequency_hz: 440.0,
            amplitude_db: -12.0,
        },
    );
    let input_id = input.id;
    session.add_input(input).unwrap();
    let bus = Bus::new("Chosen mix");
    let bus_id = bus.id;
    session.add_bus(bus).unwrap();
    session.add_send(Send::new(input_id, bus_id)).unwrap();
    (session, bus_id)
}
pub(super) fn render(session: &Session, bus: Uuid, channels: usize) -> RenderState {
    let shared = Arc::new(Shared {
        state: Mutex::new(ControlState::default()),
        epoch: AtomicU64::new(0),
    });
    RenderState::new(
        session,
        bus,
        channels,
        0,
        Arc::new(AtomicBool::new(true)),
        shared,
    )
    .unwrap()
}
pub(super) fn config(
    channels: u16,
    format: cpal::SampleFormat,
    min: u32,
    max: u32,
) -> cpal::SupportedStreamConfigRange {
    cpal::SupportedStreamConfigRange::new(
        channels,
        min,
        max,
        cpal::SupportedBufferSize::Unknown,
        format,
    )
}

#[test]
fn creation_and_stop_never_open_a_device() {
    let runtime = AudioRuntime::new().unwrap();
    let first = runtime.snapshot();
    assert!(!first.running);
    assert_eq!(first.callback_count, 0);
    assert!(first.configuration.is_none());
    runtime.stop().unwrap();
    assert!(!runtime.snapshot().running);
}

#[test]
fn cancelled_queued_start_never_attempts_device_discovery() {
    let runtime = AudioRuntime::new().unwrap();
    let token = StartCancellation::default();
    let queued = token.clone();
    token.cancel();
    let (session, bus) = session();
    // An invalid identity would produce InvalidOutputId if device start ran.
    assert_eq!(
        runtime.start_cancellable(session, bus, OutputId("not-a-device".into()), queued),
        Err(AudioError::Cancelled)
    );
    assert_eq!(runtime.snapshot().callback_count, 0);
    assert!(runtime.snapshot().configuration.is_none());
}

#[test]
fn emergency_stop_revokes_callbacks_without_the_control_mutex_or_queue() {
    let (session, bus) = session();
    let mut render = render(&session, bus, 2);
    let (commands, _receiver) = mpsc::sync_channel(1);
    commands.send(Command::Shutdown).unwrap(); // Saturated control queue.
    let runtime = AudioRuntime {
        commands,
        shared: render.shared.clone(),
    };
    let shared = runtime.shared.clone();
    let _held_control_lock = shared.state();
    runtime.cancel_output();
    let mut data = [999_i16; 128];
    render.write(&mut data);
    assert!(data.iter().all(|v| *v == 0));
    assert_eq!(render.telemetry.frames.load(Ordering::Relaxed), 0);
}

#[test]
fn start_token_cancellation_remains_effective_in_an_active_callback() {
    let (session, bus) = session();
    let mut render = render(&session, bus, 2);
    let token = StartCancellation::default();
    render.wanted = token.0.clone();
    let mut data = [0.0_f32; 128];
    render.write(&mut data);
    assert!(data.iter().any(|v| *v != 0.0));
    token.cancel();
    render.write(&mut data);
    assert!(data.iter().all(|v| *v == 0.0));
}

#[test]
fn configuration_requires_exact_session_rate_and_stereo_pcm() {
    let choices = vec![
        config(1, cpal::SampleFormat::F32, 48000, 48000),
        config(8, cpal::SampleFormat::F32, 44100, 96000),
        config(2, cpal::SampleFormat::I16, 48000, 48000),
        config(2, cpal::SampleFormat::F32, 48000, 48000),
    ];
    let chosen = select_config(choices.into_iter(), 48000).unwrap();
    assert_eq!(chosen.channels(), 2);
    assert_eq!(chosen.sample_format(), cpal::SampleFormat::F32);
    assert_eq!(chosen.sample_rate(), 48000);
    assert!(select_config(
        vec![config(2, cpal::SampleFormat::F32, 44100, 44100)].into_iter(),
        48000
    )
    .is_none());
    assert!(select_config(
        vec![config(2, cpal::SampleFormat::DsdU8, 48000, 48000)].into_iter(),
        48000
    )
    .is_none());
}

#[test]
fn output_selection_skips_unsupported_native_formats_before_channel_ranking() {
    for unsupported in [
        cpal::SampleFormat::I8,
        cpal::SampleFormat::I64,
        cpal::SampleFormat::U16,
        cpal::SampleFormat::U24,
        cpal::SampleFormat::U32,
        cpal::SampleFormat::U64,
    ] {
        let choices = [
            config(2, unsupported, 48000, 48000),
            config(4, cpal::SampleFormat::F32, 48000, 48000),
        ];
        let selected = select_config(choices.into_iter(), 48000).unwrap();
        assert_eq!(selected.channels(), 4);
        assert_eq!(selected.sample_format(), cpal::SampleFormat::F32);
    }
}

#[test]
fn output_owner_budget_is_reserved_before_compiling_or_opening_native_devices() {
    let (session, bus) = session();
    let runtime = AudioRuntime::new().unwrap();
    for bytes in [GRAPH_BYTES, usize::MAX] {
        let result = RenderState::new_with_output_budget(
            &session,
            bus,
            2,
            0,
            Arc::new(AtomicBool::new(true)),
            runtime.shared.clone(),
            bytes,
        );
        assert!(matches!(result, Err(AudioError::InvalidSession(_))));
    }
    assert!(RenderState::new_with_output_budget(
        &session,
        bus,
        2,
        0,
        Arc::new(AtomicBool::new(true)),
        runtime.shared.clone(),
        4 * 1024 * 1024,
    )
    .is_ok());
    assert!(!runtime.snapshot().running);
    assert!(runtime.snapshot().output_timing.is_none());
}

#[test]
fn selected_bus_pcm_matches_independent_sine_across_chunks_and_callbacks() {
    let (mut session, bus) = session();
    session.sends[0].controls.set_gain_db(-6.0);
    session.buses[0].controls.set_gain_db(-3.0);
    session.buses[0].controls.pan = 0.75;
    let other = Bus::new("Unselected mix");
    let other_id = other.id;
    session.add_bus(other).unwrap();
    session
        .add_send(Send::new(session.inputs[0].id, other_id))
        .unwrap();
    let mut render = render(&session, bus, 8);
    let mut data = vec![99.0_f32; 5003 * 8];
    render.write(&mut data);
    for frame in 0..5003 {
        let expected = (frame as f64 * std::f64::consts::TAU * 440.0 / 48000.0).sin() as f32
            * 10.0_f32.powf(-21.0 / 20.0);
        assert!(
            (data[frame * 8] - expected * 0.25).abs() < 0.000002,
            "left frame {frame}"
        );
        assert!(
            (data[frame * 8 + 1] - expected).abs() < 0.000002,
            "right frame {frame}"
        );
        assert!(data[frame * 8 + 2..frame * 8 + 8].iter().all(|v| *v == 0.0));
    }
    let mut next = [0.0_f32; 8];
    render.write(&mut next);
    let expected = (5003.0_f64 * std::f64::consts::TAU * 440.0 / 48000.0).sin() as f32
        * 10.0_f32.powf(-21.0 / 20.0);
    assert!((next[1] - expected).abs() < 0.000002);
    assert_eq!(render.telemetry.frames.load(Ordering::Relaxed), 5004);
    assert_eq!(render.telemetry.callbacks.load(Ordering::Relaxed), 2);
    assert!(render.telemetry.buses[0].read(true).peak[1] > 0.0);
}

#[test]
fn callback_has_no_application_allocation_or_deallocation() {
    let (session, bus) = session();
    let mut render = render(&session, bus, 2);
    let mut data = vec![0_i16; 8192];
    OPERATIONS.with(|n| n.set(0));
    COUNTING.with(|on| on.set(true));
    for _ in 0..8 {
        render.write(&mut data);
    }
    COUNTING.with(|on| on.set(false));
    assert_eq!(OPERATIONS.with(Cell::get), 0);
    assert!(data.iter().any(|sample| *sample != 0));
}

#[test]
fn timed_out_or_stopped_start_cannot_emit_late_audio() {
    let (session, bus) = session();
    let mut cancelled = render(&session, bus, 2);
    cancelled.wanted.store(false, Ordering::Release);
    let mut data = [999_i16; 128];
    cancelled.write(&mut data);
    assert!(data.iter().all(|v| *v == 0));
    assert_eq!(cancelled.telemetry.frames.load(Ordering::Relaxed), 0);
    let mut stopped = render(&session, bus, 2);
    stopped.shared.epoch.fetch_add(1, Ordering::AcqRel);
    data.fill(999);
    stopped.write(&mut data);
    assert!(data.iter().all(|v| *v == 0));
}

#[test]
fn faults_are_latched_and_unsigned_silence_uses_midpoint() {
    let (session, bus) = session();
    let mut render = render(&session, bus, 2);
    render.telemetry.fail(AudioFault::DeviceUnavailable);
    render.telemetry.fail(AudioFault::Backend);
    let mut data = [0_u16; 128];
    render.write(&mut data);
    assert!(data.iter().all(|v| *v == 32768));
    assert_eq!(
        AudioFault::from_code(render.telemetry.fault.load(Ordering::Acquire)),
        AudioFault::DeviceUnavailable
    );
}

#[test]
fn invalid_callback_buffers_stop_without_partially_writing_pcm() {
    let (session, bus) = session();
    for len in [3, (MAX_CALLBACK_FRAMES + 1) * 2] {
        let mut render = render(&session, bus, 2);
        let mut data = vec![0.9_f32; len];
        render.write(&mut data);
        assert!(data.iter().all(|v| *v == 0.0));
        assert_eq!(
            AudioFault::from_code(render.telemetry.fault.load(Ordering::Acquire)),
            AudioFault::InvalidBuffer
        );
        assert_eq!(render.telemetry.frames.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn native_transport_clamps_unlimited_float_mix_before_integer_conversion() {
    let (mut session, bus) = session();
    session.inputs[0].kind = InputKind::TestTone {
        frequency_hz: 12000.0,
        amplitude_db: 0.0,
    };
    session.buses[0].dsp = DspConfig::default();
    session.buses[0].controls.set_gain_db(24.0);
    let mut render = render(&session, bus, 2);
    let mut data = [0_i16; 8];
    render.write(&mut data);
    assert_eq!(data[2], i16::MAX);
    assert_eq!(data[6], i16::MIN);
    assert!(render.telemetry.buses[0].read(true).clipped_samples[0] >= 2);
}

#[test]
fn unavailable_application_capture_and_missing_mix_are_explicit_errors() {
    let (mut session, bus) = session();
    assert!(matches!(
        validate_session(&session, Uuid::new_v4()),
        Err(AudioError::InvalidSession(_))
    ));
    session.inputs[0].kind = InputKind::Application {
        application_id: "not-yet-supported".into(),
    };
    let error = validate_session(&session, bus).unwrap_err();
    assert!(error.to_string().contains("not yet implemented"));
    let runtime = AudioRuntime::new().unwrap();
    assert!(matches!(
        runtime.start(session, bus, OutputId("must-not-be-queried".into())),
        Err(AudioError::InvalidSession(_))
    ));
    assert!(runtime.snapshot().configuration.is_none());
}

#[test]
fn stopped_snapshot_hides_stale_meter_levels_and_keeps_diagnostics() {
    let (session, bus) = session();
    let runtime = AudioRuntime::new().unwrap();
    let mut render = render(&session, bus, 2);
    render.write(&mut [0_f32; 256]);
    {
        let mut state = runtime.shared.state();
        state.running = true;
        state.telemetry = Some(render.telemetry.clone());
        state.authorization = Some((render.wanted.clone(), 0));
    }
    assert!(runtime.snapshot().bus_meters[0].peak[0] > 0.0);
    runtime.shared.epoch.fetch_add(1, Ordering::AcqRel);
    let stopped = runtime.snapshot();
    assert!(!stopped.running);
    assert_eq!(stopped.bus_meters[0].peak, [0.0; 2]);
    assert_eq!(stopped.rendered_frames, 128);
}

#[test]
fn live_controls_preserve_phase_through_gain_pan_mute_and_unmute() {
    let (session, bus) = session();
    let mut render = render(&session, bus, 2);
    render.write(&mut [0.0_f32; 254]);
    let mut changed = session.clone();
    changed.inputs[0].controls.set_gain_db(-6.0);
    changed.sends[0].controls.set_gain_db(-3.0);
    changed.buses[0].controls.pan = -0.5;
    let prepared = prepare_live_update(&session, &changed).unwrap();
    let version = render.bank.publish(&prepared).unwrap();
    let mut pcm = [0.0_f32; 514];
    OPERATIONS.with(|n| n.set(0));
    COUNTING.with(|on| on.set(true));
    render.write(&mut pcm);
    COUNTING.with(|on| on.set(false));
    assert_eq!(
        OPERATIONS.with(Cell::get),
        0,
        "live bank application must not allocate or free"
    );
    for frame in 0..257 {
        let progress = ((frame + 1) as f32 / 240.0).min(1.0);
        let input_gain = 1.0 + (10.0_f32.powf(-6.0 / 20.0) - 1.0) * progress;
        let send_gain = 1.0 + (10.0_f32.powf(-3.0 / 20.0) - 1.0) * progress;
        let expected = (((127 + frame) as f64 * std::f64::consts::TAU * 440.0) / 48000.0).sin()
            as f32
            * 10.0_f32.powf(-12.0 / 20.0)
            * input_gain
            * send_gain;
        assert!((pcm[frame * 2] - expected).abs() < 0.000002);
        assert!((pcm[frame * 2 + 1] - expected * (1.0 - 0.5 * progress)).abs() < 0.000002);
    }
    assert_eq!(
        render
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        version
    );
    changed.inputs[0].controls.muted = true;
    render
        .bank
        .publish(&prepare_live_update(&session, &changed).unwrap())
        .unwrap();
    let mut muted = [9.0_f32; 514];
    render.write(&mut muted);
    for frame in 0..239 {
        let activity = 1.0 - (frame + 1) as f32 / 240.0;
        let expected = (((384 + frame) as f64 * std::f64::consts::TAU * 440.0) / 48000.0).sin()
            as f32
            * 10.0_f32.powf(-21.0 / 20.0)
            * activity;
        assert!((muted[frame * 2] - expected).abs() < 0.000002);
        assert!((muted[frame * 2 + 1] - expected * 0.5).abs() < 0.000002);
    }
    assert!(muted[239 * 2..].iter().all(|v| *v == 0.0));
    changed.inputs[0].controls.muted = false;
    render
        .bank
        .publish(&prepare_live_update(&session, &changed).unwrap())
        .unwrap();
    let mut resumed = [0.0_f32; 46];
    render.write(&mut resumed);
    for frame in 0..23 {
        let expected = (((641 + frame) as f64 * std::f64::consts::TAU * 440.0) / 48000.0).sin()
            as f32
            * 10.0_f32.powf(-21.0 / 20.0)
            * (frame + 1) as f32
            / 240.0;
        assert!((resumed[frame * 2] - expected).abs() < 0.000002);
        assert!((resumed[frame * 2 + 1] - expected * 0.5).abs() < 0.000002);
    }
    assert_eq!(render.telemetry.frames.load(Ordering::Relaxed), 664);
}

#[test]
fn in_flight_control_write_keeps_the_previous_complete_bank() {
    let (session, bus) = session();
    let mut render = render(&session, bus, 2);
    render.write(&mut [0.0_f32; 102]);
    let mut changed = session.clone();
    changed.buses[0].controls.muted = true;
    changed.inputs[0].controls.set_gain_db(-90.0);
    render
        .bank
        .begin_incomplete_test_write(&prepare_live_update(&session, &changed).unwrap());
    let mut pcm = [0.0_f32; 256];
    render.write(&mut pcm);
    for frame in 0..128 {
        let expected = (((51 + frame) as f64 * std::f64::consts::TAU * 440.0) / 48000.0).sin()
            as f32
            * 10.0_f32.powf(-12.0 / 20.0);
        assert!((pcm[frame * 2] - expected).abs() < 0.000002);
    }
    assert_eq!(
        render
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        0
    );
    assert_eq!(render.telemetry.fault.load(Ordering::Acquire), 0);
}

#[test]
fn empty_callbacks_do_not_acknowledge_unrendered_controls() {
    let (session, bus) = session();
    let mut render = render(&session, bus, 2);
    let mut changed = session.clone();
    changed.buses[0].controls.set_gain_db(-6.0);
    let version = render
        .bank
        .publish(&prepare_live_update(&session, &changed).unwrap())
        .unwrap();
    render.write(&mut [] as &mut [f32]);
    assert_eq!(render.applied_version, version);
    assert_eq!(
        render
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        0
    );
    assert_eq!(render.telemetry.frames.load(Ordering::Relaxed), 0);
    render.write(&mut [0.0_f32; 2]);
    assert_eq!(
        render
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        version
    );
    assert_eq!(render.telemetry.frames.load(Ordering::Relaxed), 1);
}

#[test]
fn concurrent_bank_reader_never_observes_mixed_mute_and_gain_records() {
    let (session, _) = session();
    let mut a = PreparedControls::compile(&session, GRAPH_BYTES).unwrap();
    let mut b = a.clone();
    for control in a.inputs.iter_mut().chain(&mut a.sends).chain(&mut a.buses) {
        control.gains = [0.25, 0.5];
        control.active = false;
    }
    for control in b.inputs.iter_mut().chain(&mut b.sends).chain(&mut b.buses) {
        control.gains = [0.75, 1.0];
        control.active = true;
    }
    let bank = Arc::new(ControlBank::new(&a));
    bank.publish(&a).unwrap();
    let mut scratch = a.clone();
    let writer_bank = bank.clone();
    let final_controls = b.clone();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let writer_barrier = barrier.clone();
    let writer = thread::spawn(move || {
        writer_barrier.wait();
        for index in 0..20000 {
            writer_bank
                .publish(if index % 2 == 0 { &a } else { &b })
                .unwrap();
        }
    });
    barrier.wait();
    for _ in 0..20000 {
        if bank.copy_if_new(&mut scratch, 0).is_some() {
            let expected = if scratch.inputs[0].active {
                [0.75, 1.0]
            } else {
                [0.25, 0.5]
            };
            let active = scratch.inputs[0].active;
            for control in scratch
                .inputs
                .iter()
                .chain(&scratch.sends)
                .chain(&scratch.buses)
            {
                assert_eq!(control.active, active);
                assert_eq!(control.gains, expected);
            }
        }
    }
    writer.join().unwrap();
    assert!(bank.copy_if_new(&mut scratch, 0).is_some());
    // This nonblocking reader may reject every contended attempt. It must
    // accept and return the whole final bank once the writer has finished.
    assert_eq!(scratch, final_controls);
}

#[test]
fn live_edit_rejects_source_dsp_rate_and_route_changes_before_publication() {
    let (session, _) = session();
    let mut metadata = session.clone();
    metadata.name = "Renamed".into();
    metadata.inputs[0].name = "Tone renamed".into();
    assert!(prepare_live_update(&session, &metadata).is_ok());
    let mut changed = session.clone();
    changed.sample_rate = 44100;
    assert!(prepare_live_update(&session, &changed).is_err());
    changed = session.clone();
    changed.inputs[0].kind = InputKind::TestTone {
        frequency_hz: 880.0,
        amplitude_db: -12.0,
    };
    assert!(prepare_live_update(&session, &changed).is_err());
    changed = session.clone();
    changed.buses[0].dsp = DspConfig::default();
    assert!(prepare_live_update(&session, &changed).is_err());
    changed = session.clone();
    changed.sends[0].id = Uuid::new_v4();
    assert!(prepare_live_update(&session, &changed).is_err());
    changed = session.clone();
    changed.inputs[0].controls.gain_db = f32::NAN;
    assert!(prepare_live_update(&session, &changed).is_err());
}

#[test]
fn live_update_while_stopped_cannot_create_a_stream() {
    let runtime = AudioRuntime::new().unwrap();
    let (session, _) = session();
    assert_eq!(
        runtime.update_controls(session).unwrap_err(),
        AudioError::NotRunning
    );
    assert!(!runtime.snapshot().running);
    assert_eq!(runtime.snapshot().applied_control_version, 0);
    assert!(runtime.snapshot().configuration.is_none());
}

#[test]
fn output_error_callback_keeps_original_fault_code_and_distinct_output_origin_without_allocation() {
    for kind in [cpal::ErrorKind::Xrun, cpal::ErrorKind::DeviceChanged] {
        let (session, bus) = session();
        let mut render = render(&session, bus, 2);
        OPERATIONS.with(|n| n.set(0));
        COUNTING.with(|on| on.set(true));
        record_output_error(&render.telemetry, 31, kind);
        let mut out = [0.5_f32; 128];
        render.write(&mut out);
        COUNTING.with(|on| on.set(false));
        assert_eq!(OPERATIONS.with(Cell::get), 0);
        assert!(out.iter().all(|v| *v == 0.0));
        let first = render.telemetry.fault.snapshot().unwrap();
        assert_eq!(first.fault, stream_fault(kind));
        assert_eq!(first.direction, FaultDirection::Output);
        assert_eq!(first.endpoint_index, Some(0));
        assert_eq!(first.metadata.unwrap().epoch, 31);
        assert_eq!(
            first.origin,
            if cfg!(target_os = "macos") && kind == cpal::ErrorKind::Xrun {
                FaultOrigin::CpalCoreAudioOutputOverload
            } else {
                FaultOrigin::CpalOutputError
            }
        );
        assert_eq!(first.metadata.unwrap().native_object, None);
        assert_eq!(first.metadata.unwrap().property, None);
    }
}

#[test]
fn snapshot_resolves_fault_against_its_unique_capture_identity_and_keeps_code_coupled() {
    let (mut session, bus) = session();
    for id in ["test:z", "test:a", "test:z"] {
        session
            .add_input(Input::new(
                id,
                InputKind::Hardware {
                    device_id: id.into(),
                },
            ))
            .unwrap();
    }
    let render = render(&session, bus, 2);
    assert_eq!(render.telemetry.captures.len(), 2);
    render.telemetry.fault.record(
        AudioFault::Xrun,
        FaultOrigin::NativeInputOverload,
        FaultSource::input(1),
        FaultMetadata::now(0),
    );
    let runtime = AudioRuntime::new().unwrap(); // stopped; never discovers/opens devices
    runtime.shared.state().telemetry = Some(render.telemetry.clone());
    let snapshot = runtime.snapshot();
    assert!(!snapshot.running);
    let first = snapshot.first_fault.unwrap();
    assert_eq!(snapshot.fault, first.fault);
    assert_eq!(first.endpoint_index, Some(1));
    assert_eq!(first.endpoint_id.as_deref(), Some("test:z"));
    assert_eq!(first.origin, FaultOrigin::NativeInputOverload);
}
