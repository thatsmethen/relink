//! Synthetic callback/lifecycle controls only; no native device operations.
use super::*;
use crate::{
    native_packet::{FLOAT, LINEAR_PCM, NON_INTERLEAVED, PACKED},
    tests::{COUNTING, OPERATIONS},
    Session, Telemetry,
};
use objc2_core_audio_types::AudioTimeStampFlags;
use relink_core::{Bus, Input, InputKind, Send};

fn fixture(unsigned: bool, planar: bool) -> Arc<Context> {
    let mut session = Session::new("ReLink synthetic output", 48000);
    let input = Input::new(
        "Tone",
        InputKind::TestTone {
            frequency_hz: 440.0,
            amplitude_db: -12.0,
        },
    );
    let input_id = input.id;
    session.add_input(input).unwrap();
    let bus = Bus::new("Mix");
    let bus_id = bus.id;
    session.add_bus(bus).unwrap();
    session.add_send(Send::new(input_id, bus_id)).unwrap();
    let render = crate::tests::render(&session, bus_id, 3);
    assert!(render.telemetry.gate.activate());
    let render = OutputRenderer::from(render);
    let width = if unsigned { 1 } else { 4 };
    let frame_bytes = width * if planar { 1 } else { 3 };
    let layout = OutputLayout::new(
        &[Format {
            rate: 48000.0,
            id: LINEAR_PCM,
            flags: PACKED
                | if unsigned { 0 } else { FLOAT }
                | if planar { NON_INTERLEAVED } else { 0 },
            packet_bytes: frame_bytes,
            packet_frames: 1,
            frame_bytes,
            channels: 3,
            bits: width * 8,
        }],
        48000,
    )
    .unwrap();
    Arc::new(Context {
        object: 71,
        layout: Arc::new(layout),
        gate: Arc::new(Gate {
            permit: render.permit(),
            live: AtomicBool::new(true),
            program: AtomicBool::new(false),
            started: AtomicBool::new(false),
            start_progress: AtomicU64::new(0),
        }),
        timing: Arc::new(NativeOutputTiming::new()),
        in_flight: AtomicUsize::new(0),
        entered: AtomicBool::new(false),
        state: UnsafeCell::new(PacketState {
            render,
            tracker: Tracker::default(),
        }),
    })
}
fn timestamp(sample: f64, host: u64) -> AudioTimeStamp {
    let mut value: AudioTimeStamp = unsafe { std::mem::zeroed() };
    value.mSampleTime = sample;
    value.mHostTime = host;
    value.mRateScalar = 1.0;
    value.mFlags = AudioTimeStampFlags(7);
    value
}
fn buffers(bytes: &mut [u8], channels: u32) -> AudioBufferList {
    AudioBufferList {
        mNumberBuffers: 1,
        mBuffers: [AudioBuffer {
            mNumberChannels: channels,
            mDataByteSize: bytes.len() as u32,
            mData: bytes.as_mut_ptr().cast(),
        }],
    }
}
fn invoke(block: &OutputBlock, output: &mut AudioBufferList, sample: f64, host: u64) {
    let mut now = timestamp(sample - 32.0, host);
    let mut presentation = timestamp(sample, host + 64);
    let mut unused = buffers(&mut [], 0);
    unused.mNumberBuffers = 0;
    block.call((
        NonNull::from(&mut now),
        NonNull::from(&mut unused),
        NonNull::from(&mut now),
        NonNull::from(output),
        NonNull::from(&mut presentation),
    ));
}
fn fault(telemetry: &Telemetry) -> AudioFault {
    AudioFault::from_code(telemetry.fault.load(Ordering::Acquire))
}

#[test]
fn post_start_silent_progress_precedes_activation_and_program_encoding() {
    let context = fixture(false, false);
    let callback = output_block(context.clone());
    let mut bytes = [0xa5; 128 * 12];
    let mut output = buffers(&mut bytes, 3);
    invoke(&callback, &mut output, 0.0, 1000);
    assert!(bytes.iter().all(|b| *b == 0));
    assert!(!context.gate.progressed(&context.timing));
    assert!(context.gate.activate(&context.timing).is_err());
    acknowledge_start(&context.gate, &context.timing).unwrap();
    assert!(!context.gate.progressed(&context.timing));
    invoke(&callback, &mut output, 128.0, 2000);
    assert!(context.gate.progressed(&context.timing));
    context.gate.activate(&context.timing).unwrap();
    invoke(&callback, &mut output, 256.0, 3000);
    assert!(bytes.iter().any(|b| *b != 0));
    for (index, frame) in bytes.chunks_exact(12).enumerate() {
        let expected = (index as f64 * std::f64::consts::TAU * 440.0 / 48000.0).sin() as f32
            * 10.0_f32.powf(-12.0 / 20.0);
        let left = f32::from_le_bytes(frame[..4].try_into().unwrap());
        let right = f32::from_le_bytes(frame[4..8].try_into().unwrap());
        assert!((left - expected).abs() < 2.0e-7);
        assert_eq!(left, right);
        assert_eq!(&frame[8..12], &[0; 4]);
    }
    let completion = context.timing.snapshot().last_completion.unwrap();
    assert_eq!(
        completion.outcome,
        NativeOutputOutcome::Encoded {
            accepted_version: Some(0)
        }
    );
    assert_eq!(completion.packet.presentation.sample_time, 256.0);
    assert_eq!(
        context.gate.permit.telemetry.frames.load(Ordering::Relaxed),
        128
    );
    assert_eq!(fault(&context.gate.permit.telemetry), AudioFault::None);
}

#[test]
fn every_valid_u8_plane_is_silent_before_activation_after_cancel_and_stale_epoch() {
    let context = fixture(true, false);
    let callback = output_block(context.clone());
    let mut bytes = [0; 384];
    let mut output = buffers(&mut bytes, 3);
    invoke(&callback, &mut output, 0.0, 1000);
    assert_eq!(bytes, [128; 384]);
    context.gate.program.store(true, Ordering::Release);
    context
        .gate
        .permit
        .shared
        .epoch
        .fetch_add(1, Ordering::AcqRel);
    bytes.fill(255);
    invoke(&callback, &mut output, 128.0, 2000);
    assert_eq!(bytes, [128; 384]);
    context.gate.revoke();
    bytes.fill(33);
    invoke(&callback, &mut output, 256.0, 3000);
    assert_eq!(bytes, [128; 384]);
    assert_eq!(context.timing.progress_count(), 1);
    assert_eq!(
        context.timing.snapshot().last_completion.unwrap().outcome,
        NativeOutputOutcome::Cancelled
    );
}

#[test]
fn native_sample_gap_clears_pcm_and_preserves_presentation_pair() {
    let context = fixture(false, false);
    context.gate.program.store(true, Ordering::Release);
    let callback = output_block(context.clone());
    let mut bytes = [0xa5; 128 * 12];
    let mut output = buffers(&mut bytes, 3);
    invoke(&callback, &mut output, 1000.0, 1000);
    invoke(&callback, &mut output, 1129.0, 2000);
    assert!(bytes.iter().all(|b| *b == 0));
    let pair = context.timing.snapshot().first_fault.unwrap();
    assert_eq!(pair.reason, NativeOutputFaultReason::SampleDiscontinuity);
    assert_eq!(pair.previous.unwrap().presentation.sample_time, 1000.0);
    assert_eq!(pair.current.presentation.sample_time, 1129.0);
    assert_eq!(
        context
            .gate
            .permit
            .telemetry
            .fault
            .snapshot()
            .unwrap()
            .origin,
        FaultOrigin::NativeOutputPacket
    );
    invoke(&callback, &mut output, 1257.0, 3000);
    assert_eq!(context.timing.snapshot().first_fault, Some(pair));
}

#[repr(C)]
struct ThreeBuffers {
    count: u32,
    buffers: [AudioBuffer; 3],
}
#[test]
fn valid_disjoint_planar_native_output_encodes_only_the_first_pair() {
    let context = fixture(true, true);
    context.gate.program.store(true, Ordering::Release);
    let callback = output_block(context.clone());
    let mut a = [0; 128];
    let mut b = a;
    let mut c = a;
    let mut list = ThreeBuffers {
        count: 3,
        buffers: [
            AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: 128,
                mData: a.as_mut_ptr().cast(),
            },
            AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: 128,
                mData: b.as_mut_ptr().cast(),
            },
            AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: 128,
                mData: c.as_mut_ptr().cast(),
            },
        ],
    };
    invoke(
        &callback,
        unsafe { &mut *(&mut list as *mut ThreeBuffers).cast::<AudioBufferList>() },
        0.0,
        1000,
    );
    assert_eq!(a, b);
    assert_eq!(c, [128; 128]);
    for (index, value) in a.iter().enumerate() {
        let sample = (index as f64 * std::f64::consts::TAU * 440.0 / 48000.0).sin()
            * 10.0_f64.powf(-12.0 / 20.0);
        assert_eq!(*value, (sample * 128.0 + 128.0).round() as u8);
    }
    assert_eq!(context.timing.progress_count(), 1);
}
#[test]
fn planar_native_ranges_reject_alias_null_and_descriptor_overlap_before_writes() {
    let context = fixture(true, true);
    let mut a = [0x55; 128];
    let mut b = a;
    let mut c = a;
    let mut list = ThreeBuffers {
        count: 3,
        buffers: [
            AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: 128,
                mData: a.as_mut_ptr().cast(),
            },
            AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: 128,
                mData: b.as_mut_ptr().cast(),
            },
            AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: 128,
                mData: c.as_mut_ptr().cast(),
            },
        ],
    };
    assert!(validate_ranges(
        &context.layout,
        &list.buffers,
        &[(&list as *const _ as usize, size_of::<ThreeBuffers>())]
    )
    .is_ok());
    list.buffers[1].mData = list.buffers[0].mData;
    assert_eq!(
        validate_ranges(&context.layout, &list.buffers, &[]),
        Err(AudioFault::InvalidBuffer)
    );
    list.buffers[1].mData = ptr::null_mut();
    assert_eq!(
        validate_ranges(&context.layout, &list.buffers, &[]),
        Err(AudioFault::InvalidBuffer)
    );
    list.buffers[1].mData = b.as_mut_ptr().cast();
    list.buffers[2].mData = &mut list.count as *mut _ as *mut c_void;
    assert_eq!(
        validate_ranges(
            &context.layout,
            &list.buffers,
            &[(&list as *const _ as usize, size_of::<ThreeBuffers>())]
        ),
        Err(AudioFault::InvalidBuffer)
    );
    list.buffers[2].mData = c.as_mut_ptr().cast();
    assert_eq!(
        validate_ranges(&context.layout, &list.buffers, &[(c.as_ptr() as usize, 32)]),
        Err(AudioFault::InvalidBuffer)
    );
    list.buffers[2].mDataByteSize = 127;
    let callback = output_block(context.clone());
    let output = unsafe { &mut *(&mut list as *mut ThreeBuffers).cast::<AudioBufferList>() };
    invoke(&callback, output, 0.0, 1000);
    assert_eq!(a, [0x55; 128]);
    assert_eq!(b, a);
    assert_eq!(c, a);
    assert_eq!(
        fault(&context.gate.permit.telemetry),
        AudioFault::InvalidBuffer
    );
}

#[test]
fn retained_native_block_owns_context_and_clears_late_output_after_cancel() {
    let context = fixture(true, false);
    let weak = Arc::downgrade(&context);
    let callback = output_block(context.clone());
    let retained = callback.clone();
    context.gate.revoke();
    drop(callback);
    drop(context);
    assert!(weak.upgrade().is_some());
    let mut bytes = [0; 384];
    let mut output = buffers(&mut bytes, 3);
    invoke(&retained, &mut output, 0.0, 1000);
    assert_eq!(bytes, [128; 384]);
    drop(retained);
    assert!(weak.upgrade().is_none());
}

#[test]
fn reentry_cannot_mutably_alias_an_active_native_writer() {
    let context = fixture(true, false);
    let callback = output_block(context.clone());
    context.entered.store(true, Ordering::Release);
    let mut bytes = [0x55; 384];
    let mut output = buffers(&mut bytes, 3);
    invoke(&callback, &mut output, 0.0, 1000);
    assert_eq!(bytes, [0x55; 384]);
    assert_eq!(
        context
            .gate
            .permit
            .telemetry
            .fault
            .snapshot()
            .unwrap()
            .origin,
        FaultOrigin::NativeOutputReentry
    );
    assert_eq!(context.in_flight.load(Ordering::Acquire), 0);
    context.entered.store(false, Ordering::Release);
    invoke(&callback, &mut output, 128.0, 2000);
    assert_eq!(bytes, [128; 384]);
}

#[test]
fn callback_first_silence_program_fault_and_late_clear_allocate_nothing() {
    let context = fixture(true, false);
    let callback = output_block(context.clone());
    let mut bytes = [0x55; 384];
    let mut output = buffers(&mut bytes, 3);
    OPERATIONS.with(|n| n.set(0));
    COUNTING.with(|n| n.set(true));
    invoke(&callback, &mut output, 0.0, 1000);
    acknowledge_start(&context.gate, &context.timing).unwrap();
    invoke(&callback, &mut output, 128.0, 2000);
    context.gate.activate(&context.timing).unwrap();
    invoke(&callback, &mut output, 256.0, 3000);
    invoke(&callback, &mut output, 385.0, 4000);
    invoke(&callback, &mut output, 513.0, 5000);
    COUNTING.with(|n| n.set(false));
    assert_eq!(OPERATIONS.with(|n| n.get()), 0);
    assert_eq!(bytes, [128; 384]);
}

#[test]
fn output_property_fault_preserves_actual_address_without_callback_allocation() {
    let context = fixture(true, false);
    let property = address(
        kAudioDeviceProcessorOverload,
        kAudioObjectPropertyScopeGlobal,
    );
    let callback = listener_block(context.gate.clone(), 71, property, AudioFault::Xrun);
    let mut changed = property;
    OPERATIONS.with(|n| n.set(0));
    COUNTING.with(|n| n.set(true));
    callback.call((1, NonNull::from(&mut changed)));
    COUNTING.with(|n| n.set(false));
    assert_eq!(OPERATIONS.with(|n| n.get()), 0);
    let f = context.gate.permit.telemetry.fault.snapshot().unwrap();
    assert_eq!(f.origin, FaultOrigin::NativeOutputOverload);
    assert_eq!(
        f.metadata.unwrap().property.unwrap().selector,
        kAudioDeviceProcessorOverload
    );
}

struct FakeClose {
    codes: [i32; 4],
    calls: Vec<usize>,
    timing: Arc<NativeOutputTiming>,
}
impl CloseOps for FakeClose {
    fn mark(&self, phase: NativeOutputLifecyclePhase, status: Option<i32>) {
        self.timing.lifecycle(NativeOutputLifecycleEvent {
            phase,
            host_time: 1,
            status,
            callbacks_in_flight: Some(0),
        });
    }
    fn stop(&mut self) -> i32 {
        self.calls.push(0);
        self.codes[0]
    }
    fn destroy(&mut self) -> i32 {
        self.calls.push(1);
        self.codes[1]
    }
    fn remove(&mut self, i: usize) -> i32 {
        self.calls.push(i + 2);
        self.codes[i + 2]
    }
}
#[test]
fn teardown_calls_all_boundaries_once_retains_first_failure_and_never_invents_completion() {
    for failed in 0..5 {
        let mut codes = [0; 4];
        if failed < 4 {
            codes[failed] = -50 - failed as i32;
        }
        let timing = Arc::new(NativeOutputTiming::new());
        let mut ops = FakeClose {
            codes,
            calls: Vec::new(),
            timing: timing.clone(),
        };
        let result = close_native(&mut ops, true, true, 2);
        assert_eq!(ops.calls, [0, 1, 2, 3]);
        assert_eq!(result.quiescent, failed == 4);
        if failed < 4 {
            assert_eq!(
                result.first_failure,
                Some((
                    codes[failed],
                    if failed >= 2 { Some(failed - 2) } else { None }
                ))
            );
        }
        assert!(
            timing.snapshot().lifecycle[NativeOutputLifecyclePhase::OwnerCompleted as usize]
                .is_none()
        );
    }
}

#[test]
fn late_start_and_unproven_callbacks_never_enable_program_or_reuse_uid() {
    let context = fixture(true, false);
    let reservation = Reservation::acquire("synthetic-output-retention").unwrap();
    assert!(Reservation::acquire("synthetic-output-retention").is_err());
    context.gate.revoke();
    assert!(acknowledge_start(&context.gate, &context.timing).is_err());
    assert!(!context.gate.program.load(Ordering::Acquire));
    context.in_flight.store(1, Ordering::Release);
    assert!(!drained(&context, Duration::ZERO));
    // A retained owner keeps the registry entry while any callback remains.
    assert!(Reservation::acquire("synthetic-output-retention").is_err());
    context.in_flight.store(0, Ordering::Release);
    assert!(drained(&context, Duration::ZERO));
    drop(reservation);
    assert!(Reservation::acquire("synthetic-output-retention").is_ok());
}

#[test]
fn blocked_native_teardown_holds_reservation_while_handle_can_revoke() {
    let context = fixture(true, false);
    let gate = context.gate.clone();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let owner_gate = gate.clone();
    let owner = thread::spawn(move || {
        let _reservation = Reservation::acquire("synthetic-blocked-native-output").unwrap();
        owner_gate.revoke();
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(Reservation::acquire("synthetic-blocked-native-output").is_err());
    assert!(!gate.authorized());
    release_tx.send(()).unwrap();
    owner.join().unwrap();
    assert!(Reservation::acquire("synthetic-blocked-native-output").is_ok());
}

fn synthetic_owner(context: Arc<Context>, uid: &str) -> Owner {
    let layout = context.layout.clone();
    let reservation = Reservation::acquire(uid).unwrap();
    Owner {
        selection: Selection {
            id: 71,
            uid: uid.into(),
            rate: 48000,
            streams: Vec::new(),
            layout,
            input_streams: 0,
        },
        callback: Some(output_block(context.clone())),
        gate: context.gate.clone(),
        timing: context.timing.clone(),
        context: Some(context),
        proc_id: None,
        start_attempted: false,
        listeners: Vec::new(),
        reservation: Some(reservation),
    }
}

#[test]
fn actual_owner_finish_preserves_context_on_failure_and_releases_before_completion() {
    let context = fixture(true, false);
    let weak = Arc::downgrade(&context);
    let timing = context.timing.clone();
    let mut owner = synthetic_owner(context, "synthetic-owner-success");
    owner.gate.revoke();
    owner.finish_teardown(
        CloseResult {
            quiescent: true,
            first_failure: None,
        },
        Duration::ZERO,
    );
    assert!(weak.upgrade().is_none());
    assert!(Reservation::acquire("synthetic-owner-success").is_ok());
    assert!(
        timing.snapshot().lifecycle[NativeOutputLifecyclePhase::OwnerCompleted as usize].is_some()
    );
    drop(owner);

    let context = fixture(true, false);
    let weak = Arc::downgrade(&context);
    let timing = context.timing.clone();
    // The failed branch deliberately retains the original application block
    // and context, just as it must when the OS cannot prove native quiescence.
    let mut owner =
        std::mem::ManuallyDrop::new(synthetic_owner(context, "synthetic-owner-failure"));
    owner.gate.revoke();
    owner.finish_teardown(
        CloseResult {
            quiescent: false,
            first_failure: Some((-50, None)),
        },
        Duration::ZERO,
    );
    assert!(weak.upgrade().is_some());
    assert!(Reservation::acquire("synthetic-owner-failure").is_err());
    assert!(
        timing.snapshot().lifecycle[NativeOutputLifecyclePhase::ContextRetained as usize].is_some()
    );
    assert!(
        timing.snapshot().lifecycle[NativeOutputLifecyclePhase::OwnerCompleted as usize].is_none()
    );
    assert_eq!(
        owner
            .gate
            .permit
            .telemetry
            .fault
            .snapshot()
            .unwrap()
            .metadata
            .unwrap()
            .status,
        Some(-50)
    );
    // This fixture does not call a native API or erase retained-state evidence.
}

#[test]
fn startup_timeout_revokes_and_owner_memory_estimate_includes_explicit_thread_stack() {
    let context = fixture(true, false);
    let mut owner = synthetic_owner(context.clone(), "synthetic-output-budget");
    assert!(
        owner.selection.owner_bytes().unwrap()
            >= OWNER_STACK
                + size_of::<Context>()
                + size_of::<NativeOutputTiming>()
                + owner.selection.storage_bytes()
    );
    let (commands, _receiver) = mpsc::sync_channel(1);
    let handle = NativeOutput {
        commands,
        gate: context.gate.clone(),
        timing: context.timing.clone(),
    };
    assert!(matches!(
        handle.wait_for_progress(Duration::ZERO),
        Err(AudioError::Timeout { .. })
    ));
    assert!(!context.gate.authorized());
    assert!(!context.gate.program.load(Ordering::Acquire));
    assert_eq!(
        context
            .gate
            .permit
            .telemetry
            .fault
            .snapshot()
            .unwrap()
            .origin,
        FaultOrigin::NativeOutputStartup
    );
    owner.finish_teardown(
        CloseResult {
            quiescent: true,
            first_failure: None,
        },
        Duration::ZERO,
    );
}

#[test]
fn fixed_registry_bounds_pending_retained_and_distinct_uid_owners() {
    let mut registry = Reservations::new();
    for index in 0..MAX_RESERVATIONS {
        registry.insert(&format!("owner-{index}")).unwrap();
    }
    assert!(registry.insert("owner-extra").is_err());
    assert!(registry.insert("owner-0").is_err());
    registry.remove("owner-0");
    registry.insert("owner-extra").unwrap();
    assert!(registry.insert("owner-0").is_err());
    registry.remove("owner-extra");
    registry.insert("owner-0").unwrap();
    assert_eq!(registry.slots.iter().flatten().count(), MAX_RESERVATIONS);
}
