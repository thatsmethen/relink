use relink_core::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

struct CountingAllocator;
thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static OPERATIONS: Cell<usize> = const { Cell::new(0) };
}
fn count() {
    let _ = COUNTING.try_with(|enabled| {
        if enabled.get() {
            let _ = OPERATIONS.try_with(|n| n.set(n.get() + 1));
        }
    });
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count();
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn fixture() -> Session {
    let mut session = Session::new("Offline bus views", 48000);
    let input = session
        .add_input(Input::new(
            "Source",
            InputKind::Hardware {
                device_id: "offline-only".into(),
            },
        ))
        .unwrap();
    for name in ["First", "Second"] {
        let mut bus = Bus::new(name);
        bus.dsp = DspConfig::default();
        let id = session.add_bus(bus).unwrap();
        session.add_send(Send::new(input, id)).unwrap();
    }
    session
}
fn compile(session: &Session, frames: usize) -> CompiledMixer {
    CompiledMixer::compile(
        session,
        ResourceBudget {
            max_block_frames: frames,
            ..Default::default()
        },
    )
    .unwrap()
}
fn source<'a>(session: &Session, left: &'a [f32], right: &'a [f32]) -> StereoInput<'a> {
    StereoInput {
        id: session.inputs[0].id,
        left,
        right,
    }
}
fn close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 3.0e-7, "{actual} != {expected}");
}

#[test]
fn handles_reject_unknown_buses_and_other_graphs_but_survive_moves() {
    let session = fixture();
    let mut mixer = compile(&session, 16);
    let handle = mixer.resolve_bus(session.buses[0].id).unwrap();
    assert_eq!(
        mixer.resolve_bus(Uuid::nil()),
        Err(RenderError::UnknownBus(Uuid::nil()))
    );
    assert_eq!(
        mixer.rendered_bus(handle),
        Err(BusReadError::NoSuccessfulRender)
    );
    let mut replacement = compile(&session, 16);
    assert_eq!(
        replacement.rendered_bus(handle),
        Err(BusReadError::InvalidHandle)
    );
    replacement.render(&[], &mut [], 16).unwrap();
    assert_eq!(
        replacement.rendered_bus(handle),
        Err(BusReadError::InvalidHandle)
    );
    assert_ne!(
        handle,
        replacement.resolve_bus(session.buses[0].id).unwrap()
    );
    mixer.render(&[], &mut [], 16).unwrap();
    let moved = Box::new(mixer);
    assert_eq!(moved.rendered_bus(handle).unwrap(), &[[0.0; 2]; 16]);
}

#[test]
fn multiple_buses_expose_exact_post_dsp_audio_including_omitted_outputs() {
    let mut session = fixture();
    session.buses[0].dsp.limiter = Some(LimiterConfig {
        ceiling_db: -6.0,
        release_ms: 100.0,
    });
    session.buses[1].controls.gain_db = -12.0;
    let mut mixer = compile(&session, 8);
    let first = mixer.resolve_bus(session.buses[0].id).unwrap();
    let second = mixer.resolve_bus(session.buses[1].id).unwrap();
    let mut copied_left = [42.0; 8];
    let mut copied_right = [42.0; 8];
    mixer
        .render_sources(
            [source(&session, &[1.0; 8], &[-0.5; 8])].into_iter(),
            &mut [StereoOutput {
                id: session.buses[0].id,
                left: &mut copied_left,
                right: &mut copied_right,
            }],
            8,
        )
        .unwrap();
    let a = mixer.rendered_bus(first).unwrap();
    let b = mixer.rendered_bus(second).unwrap();
    let same_bus_again = mixer.rendered_bus(first).unwrap();
    assert!(std::ptr::eq(a.as_ptr(), same_bus_again.as_ptr()));
    for index in 0..8 {
        assert_eq!(a[index], [copied_left[index], copied_right[index]]);
        close(a[index][0], 10.0_f32.powf(-6.0 / 20.0));
        close(a[index][1], -0.5 * 10.0_f32.powf(-6.0 / 20.0));
        close(b[index][0], 10.0_f32.powf(-12.0 / 20.0));
        close(b[index][1], -0.5 * 10.0_f32.powf(-12.0 / 20.0));
    }
}

#[test]
fn shorter_zero_and_rejected_attempts_never_expose_an_old_tail() {
    let session = fixture();
    let mut mixer = compile(&session, 16);
    let handle = mixer.resolve_bus(session.buses[0].id).unwrap();
    mixer
        .render(&[source(&session, &[0.75; 16], &[-0.5; 16])], &mut [], 16)
        .unwrap();
    assert_eq!(mixer.rendered_bus(handle).unwrap().len(), 16);
    mixer
        .render(&[source(&session, &[0.25; 3], &[0.125; 3])], &mut [], 3)
        .unwrap();
    assert_eq!(mixer.rendered_bus(handle).unwrap(), &[[0.25, 0.125]; 3]);
    mixer.render(&[], &mut [], 0).unwrap();
    assert!(mixer.rendered_bus(handle).unwrap().is_empty());

    // Every validation failure invalidates the read extent, even when its
    // previous success had zero frames. Caller-owned output is never touched.
    for case in 0..7 {
        mixer
            .render(&[source(&session, &[0.2; 4], &[0.3; 4])], &mut [], 4)
            .unwrap();
        let (mut left, mut right, mut left2, mut right2) =
            ([42.0; 4], [42.0; 4], [43.0; 4], [43.0; 4]);
        let supplied = source(&session, &[0.0; 4], &[0.0; 4]);
        let result = match case {
            0 => mixer.render(&[], &mut [], 17),
            1 => mixer.render(
                &[StereoInput {
                    id: Uuid::nil(),
                    ..supplied
                }],
                &mut [],
                1,
            ),
            2 => mixer.render(&[supplied, supplied], &mut [], 1),
            3 => mixer.render(&[source(&session, &[], &[])], &mut [], 1),
            4 => mixer.render(
                &[],
                &mut [StereoOutput {
                    id: Uuid::nil(),
                    left: &mut left,
                    right: &mut right,
                }],
                1,
            ),
            5 => mixer.render(
                &[],
                &mut [
                    StereoOutput {
                        id: session.buses[0].id,
                        left: &mut left,
                        right: &mut right,
                    },
                    StereoOutput {
                        id: session.buses[0].id,
                        left: &mut left2,
                        right: &mut right2,
                    },
                ],
                1,
            ),
            _ => mixer.render(
                &[],
                &mut [StereoOutput {
                    id: session.buses[0].id,
                    left: &mut left[..0],
                    right: &mut right,
                }],
                1,
            ),
        };
        assert!(result.is_err());
        assert_eq!(
            mixer.rendered_bus(handle),
            Err(BusReadError::NoSuccessfulRender)
        );
        assert_eq!(left, [42.0; 4]);
        assert_eq!(right, [42.0; 4]);
        assert_eq!(left2, [43.0; 4]);
        assert_eq!(right2, [43.0; 4]);
    }
    mixer
        .render_sources(std::iter::empty(), &mut [], 0)
        .unwrap();
    assert!(mixer.rendered_bus(handle).unwrap().is_empty());
}

#[test]
fn repeated_fanout_advances_oscillator_and_control_ramps_only_once_per_frame() {
    let mut session = fixture();
    session.inputs[0].kind = InputKind::TestTone {
        frequency_hz: 997.0,
        amplitude_db: -12.0,
    };
    let mut mixer = compile(&session, 256);
    let handles = [
        mixer.resolve_bus(session.buses[0].id).unwrap(),
        mixer.resolve_bus(session.buses[1].id).unwrap(),
    ];
    let mut bank = PreparedControls::compile(&session, 1024 * 1024).unwrap();
    bank.inputs[0].gains = [0.5; 2];
    bank.sends[0].gains = [0.25; 2];
    bank.buses[0].gains = [0.75; 2];
    mixer.apply_controls(&bank).unwrap();
    let mut phase = 0.0_f64;
    let increment = std::f64::consts::TAU * 997.0 / 48000.0;
    let mut total = 0;
    for count in [17, 103, 120, 1, 13] {
        mixer.render(&[], &mut [], count).unwrap();
        let meters = mixer.bus_meters().to_vec();
        for frame in 0..count {
            total += 1;
            let fraction = (total as f64 / 240.0).min(1.0);
            let gain = |target: f64| (1.0 + (target - 1.0) * fraction) as f32;
            let tone = phase.sin() as f32 * 10.0_f32.powf(-12.0 / 20.0);
            let input = tone * gain(0.5);
            close(
                mixer.rendered_bus(handles[0]).unwrap()[frame][0],
                input * gain(0.25) * gain(0.75),
            );
            close(mixer.rendered_bus(handles[1]).unwrap()[frame][0], input);
            for _ in 0..4 {
                assert_eq!(
                    mixer.rendered_bus(handles[0]).unwrap()[frame][0],
                    mixer.rendered_bus(handles[0]).unwrap()[frame][1]
                );
            }
            phase += increment;
            if phase >= std::f64::consts::TAU {
                phase -= std::f64::consts::TAU;
            }
        }
        assert_eq!(mixer.bus_meters(), meters);
        // Neither zero/rejected attempts nor publishing an unchanged control
        // bank may advance the next oscillator/ramp sample.
        let previous = mixer.rendered_bus(handles[0]).unwrap().to_vec();
        mixer.apply_controls(&bank).unwrap();
        assert_eq!(mixer.rendered_bus(handles[0]).unwrap(), previous);
        mixer.render(&[], &mut [], 0).unwrap();
        assert!(mixer.render(&[], &mut [], 257).is_err());
    }
}

#[test]
fn bus_filter_history_advances_once_despite_repeated_views_and_missing_destinations() {
    let mut session = fixture();
    for (bus, cutoff) in session.buses.iter_mut().zip([80.0, 800.0]) {
        bus.dsp.high_pass = Some(HighPassConfig { cutoff_hz: cutoff });
    }
    let mut mixer = compile(&session, 64);
    let handles = [
        mixer.resolve_bus(session.buses[0].id).unwrap(),
        mixer.resolve_bus(session.buses[1].id).unwrap(),
    ];
    let alpha = [80.0_f32, 800.0].map(|cutoff| (-std::f32::consts::TAU * cutoff / 48000.0).exp());
    let mut previous_input = [[0.0_f32; 2]; 2];
    let mut previous_output = [[0.0_f32; 2]; 2];
    for (count, value) in [(11, 0.5), (31, 0.0), (3, -0.25), (19, 0.0)] {
        let left = [value; 64];
        let right = [-value * 0.5; 64];
        mixer
            .render(&[source(&session, &left, &right)], &mut [], count)
            .unwrap();
        for bus in 0..2 {
            let frames = mixer.rendered_bus(handles[bus]).unwrap();
            for frame in frames {
                for channel in 0..2 {
                    let input = [value, -value * 0.5][channel];
                    let expected = alpha[bus]
                        * (previous_output[bus][channel] + input - previous_input[bus][channel]);
                    close(frame[channel], expected);
                    previous_input[bus][channel] = input;
                    previous_output[bus][channel] = expected;
                }
            }
            for _ in 0..9 {
                assert_eq!(mixer.rendered_bus(handles[bus]).unwrap(), frames);
            }
        }
    }
}

#[test]
fn rendering_and_all_view_paths_allocate_or_deallocate_nothing() {
    let session = fixture();
    let mut mixer = compile(&session, 32);
    let handles = [
        mixer.resolve_bus(session.buses[0].id).unwrap(),
        mixer.resolve_bus(session.buses[1].id).unwrap(),
    ];
    let other = compile(&session, 32);
    let foreign = other.resolve_bus(session.buses[0].id).unwrap();
    let input = source(&session, &[0.25; 32], &[-0.125; 32]);
    OPERATIONS.with(|n| n.set(0));
    COUNTING.with(|enabled| enabled.set(true));
    assert_eq!(
        mixer.rendered_bus(handles[0]),
        Err(BusReadError::NoSuccessfulRender)
    );
    assert_eq!(
        mixer.rendered_bus(foreign),
        Err(BusReadError::InvalidHandle)
    );
    for _ in 0..10 {
        mixer.render(&[input], &mut [], 32).unwrap();
        for handle in handles {
            for _ in 0..8 {
                std::hint::black_box(mixer.rendered_bus(handle).unwrap());
            }
        }
        assert!(mixer.render(&[], &mut [], 33).is_err());
        assert_eq!(
            mixer.rendered_bus(handles[0]),
            Err(BusReadError::NoSuccessfulRender)
        );
        mixer.render(&[], &mut [], 0).unwrap();
        assert!(mixer.rendered_bus(handles[0]).unwrap().is_empty());
    }
    COUNTING.with(|enabled| enabled.set(false));
    assert_eq!(OPERATIONS.with(Cell::get), 0);
}
