use relink_core::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

struct CountingAllocator;
thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = COUNTING.try_with(|enabled| {
            if enabled.get() {
                let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let _ = COUNTING.try_with(|enabled| {
            if enabled.get() {
                let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn fixture() -> (Session, Uuid, Uuid, Uuid, Uuid) {
    let mut session = Session::new("Two sources, two mixes", 48_000);
    let a = session
        .add_input(Input::new(
            "Mic",
            InputKind::Hardware {
                device_id: "mic-1".into(),
            },
        ))
        .unwrap();
    let b = session
        .add_input(Input::new(
            "App",
            InputKind::Application {
                application_id: "app-1".into(),
            },
        ))
        .unwrap();
    let mut first = Bus::new("Headphones");
    first.dsp = DspConfig::default();
    let mut second = Bus::new("Broadcast");
    second.dsp = DspConfig::default();
    let x = session.add_bus(first).unwrap();
    let y = session.add_bus(second).unwrap();
    session.add_send(Send::new(a, x)).unwrap();
    session.add_send(Send::new(b, x)).unwrap();
    session.add_send(Send::new(b, y)).unwrap();
    (session, a, b, x, y)
}

fn render_pair(session: &Session, a: Uuid, b: Uuid, x: Uuid, y: Uuid) -> ([f32; 2], [f32; 2]) {
    let mut mixer = CompiledMixer::compile(session, ResourceBudget::default()).unwrap();
    let (mut xl, mut xr, mut yl, mut yr) = ([0.0], [0.0], [0.0], [0.0]);
    mixer
        .render(
            &[
                StereoInput {
                    id: b,
                    left: &[0.3],
                    right: &[-0.1],
                },
                StereoInput {
                    id: a,
                    left: &[0.2],
                    right: &[0.4],
                },
            ],
            &mut [
                StereoOutput {
                    id: y,
                    left: &mut yl,
                    right: &mut yr,
                },
                StereoOutput {
                    id: x,
                    left: &mut xl,
                    right: &mut xr,
                },
            ],
            1,
        )
        .unwrap();
    ([xl[0], xr[0]], [yl[0], yr[0]])
}

fn close(a: f32, b: f32) {
    assert!((a - b).abs() < 1.0e-5, "{a} != {b}");
}

#[test]
fn distinct_stereo_sources_sum_only_into_their_destinations() {
    let (session, a, b, x, y) = fixture();
    let (first, second) = render_pair(&session, a, b, x, y);
    close(first[0], 0.5);
    close(first[1], 0.3);
    close(second[0], 0.3);
    close(second[1], -0.1);
}

#[test]
fn input_send_and_bus_solo_have_distinct_scopes_and_mute_wins() {
    let (mut session, a, b, x, y) = fixture();
    session.inputs[0].controls.solo = true;
    let (first, second) = render_pair(&session, a, b, x, y);
    assert_eq!(first, [0.2, 0.4]);
    assert_eq!(second, [0.0, 0.0]);
    session.inputs[0].controls.muted = true;
    assert_eq!(render_pair(&session, a, b, x, y), ([0.0; 2], [0.0; 2]));
    session.inputs[0].controls = ChannelControls::default();
    session.sends[0].controls.solo = true;
    let (first, second) = render_pair(&session, a, b, x, y);
    assert_eq!(first, [0.2, 0.4]);
    assert_eq!(second, [0.3, -0.1]);
    session.sends[0].controls.muted = true;
    assert_eq!(render_pair(&session, a, b, x, y).0, [0.0; 2]);
    session.sends[0].controls = ChannelControls::default();
    session.buses[1].controls.solo = true;
    let (first, second) = render_pair(&session, a, b, x, y);
    assert_eq!(first, [0.0; 2]);
    assert_eq!(second, [0.3, -0.1]);
    session.buses[1].controls.muted = true;
    assert_eq!(render_pair(&session, a, b, x, y), ([0.0; 2], [0.0; 2]));
}

#[test]
fn stereo_gain_linking_and_pan_preserve_channel_identity() {
    let (mut session, a, b, x, y) = fixture();
    session.sends[1].controls.muted = true;
    session.sends[0].controls.set_gain_db(-6.020_6);
    close(render_pair(&session, a, b, x, y).0[0], 0.1);
    close(render_pair(&session, a, b, x, y).0[1], 0.2);
    session.sends[0].controls.set_linked(false);
    session.sends[0].controls.set_right_gain_db(0.0);
    close(render_pair(&session, a, b, x, y).0[1], 0.4);
    session.sends[0].controls.pan = 1.0;
    assert_eq!(render_pair(&session, a, b, x, y).0, [0.0, 0.4]);
    session.sends[0].controls.set_linked(true);
    close(session.sends[0].controls.right_gain_db, -6.020_6);
}

#[test]
fn persistence_round_trip_preserves_every_stable_id_and_rejects_future_versions() {
    let (session, _, _, _, _) = fixture();
    let json = session.to_json().unwrap();
    assert_eq!(Session::from_json(&json).unwrap(), session);
    assert!(matches!(
        Session::from_json(r#"{"schema_version":2,"new_future_shape":true}"#),
        Err(SessionError::UnsupportedVersion {
            found: 2,
            supported: 1
        })
    ));
    assert!(Session::from_json("{}").is_err());
    assert!(Session::from_json("{not json}").is_err());
}

#[test]
fn deleting_nodes_cleans_routes_and_rejected_mutations_roll_back() {
    let (mut session, a, b, x, y) = fixture();
    let original = session.clone();
    assert!(matches!(
        session.add_send(Send::new(a, x)),
        Err(SessionError::DuplicateRoute { .. })
    ));
    assert_eq!(session, original);
    let mut duplicate = Input::new(
        "Duplicate",
        InputKind::Virtual {
            endpoint_id: "virtual".into(),
        },
    );
    duplicate.id = b;
    assert!(matches!(
        session.add_input(duplicate),
        Err(SessionError::DuplicateId(_))
    ));
    assert_eq!(session, original);
    session.remove_input(a).unwrap();
    assert_eq!(session.sends.len(), 2);
    session.remove_bus(y).unwrap();
    assert_eq!(session.sends.len(), 1);
    assert_eq!(session.sends[0].input_id, b);
    assert_eq!(session.sends[0].bus_id, x);
    session.validate().unwrap();
}

#[test]
fn malformed_numeric_values_and_dangling_routes_never_reach_render() {
    let (session, _, _, _, _) = fixture();
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, -121.0] {
        let mut bad = session.clone();
        bad.inputs[0].controls.gain_db = value;
        assert!(CompiledMixer::compile(&bad, ResourceBudget::default()).is_err());
        assert!(bad.to_json().is_err());
    }
    let mut bad = session.clone();
    bad.sends[0].bus_id = Uuid::new_v4();
    assert!(matches!(bad.validate(), Err(SessionError::MissingBus(_))));
    let mut bad = session.clone();
    bad.inputs[0].dsp.compressor = Some(CompressorConfig {
        ratio: 0.0,
        ..Default::default()
    });
    assert!(bad.validate().is_err());
    let mut bad = session.clone();
    bad.buses[0].dsp.high_pass = Some(HighPassConfig {
        cutoff_hz: 24_000.0,
    });
    assert!(bad.validate().is_err());
    let mut bad = session;
    bad.id = Uuid::nil();
    assert!(bad.validate().is_err());
}

#[test]
fn resource_limits_are_explicit_and_overflow_is_an_error() {
    let (session, _, _, _, _) = fixture();
    assert!(matches!(
        CompiledMixer::compile(
            &session,
            ResourceBudget {
                max_block_frames: 128,
                max_working_bytes: 1
            }
        ),
        Err(SessionError::ResourceBudget { .. })
    ));
    assert!(matches!(
        CompiledMixer::compile(
            &session,
            ResourceBudget {
                max_block_frames: usize::MAX,
                max_working_bytes: usize::MAX
            }
        ),
        Err(SessionError::ResourceBudget { .. })
    ));
    assert!(CompiledMixer::compile(
        &session,
        ResourceBudget {
            max_block_frames: 0,
            max_working_bytes: usize::MAX
        }
    )
    .is_err());
    let mut many = Session::new("Many inputs", 48_000);
    for index in 0..300 {
        many.inputs.push(Input::new(
            format!("Input {index}"),
            InputKind::Virtual {
                endpoint_id: format!("input-{index}"),
            },
        ));
    }
    let mixer = CompiledMixer::compile(
        &many,
        ResourceBudget {
            max_block_frames: 16,
            max_working_bytes: 1_000_000,
        },
    )
    .unwrap();
    assert_eq!(mixer.input_meters().len(), 300);
}

#[test]
fn bad_buffers_return_before_mutating_output_or_advancing_tone() {
    let mut session = Session::new("Tone", 48_000);
    let input = session
        .add_input(Input::new(
            "Tone",
            InputKind::TestTone {
                frequency_hz: 1000.0,
                amplitude_db: -20.0,
            },
        ))
        .unwrap();
    let bus = session.add_bus(Bus::new("Mix")).unwrap();
    session.add_send(Send::new(input, bus)).unwrap();
    let mut mixer = CompiledMixer::compile(
        &session,
        ResourceBudget {
            max_block_frames: 4,
            ..Default::default()
        },
    )
    .unwrap();
    let (mut l, mut r) = ([42.0; 4], [42.0; 4]);
    assert!(matches!(
        mixer.render(
            &[],
            &mut [StereoOutput {
                id: bus,
                left: &mut l,
                right: &mut r
            }],
            5
        ),
        Err(RenderError::BlockTooLarge { .. })
    ));
    assert_eq!(l, [42.0; 4]);
    assert_eq!(r, [42.0; 4]);
    assert!(matches!(
        mixer.render(
            &[StereoInput {
                id: input,
                left: &[],
                right: &[]
            }],
            &mut [],
            1
        ),
        Err(RenderError::BufferTooShort { .. })
    ));
    assert!(matches!(
        mixer.render(
            &[StereoInput {
                id: Uuid::nil(),
                left: &[0.0],
                right: &[0.0]
            }],
            &mut [],
            1
        ),
        Err(RenderError::UnknownInput(_))
    ));
    assert!(matches!(
        mixer.render(
            &[
                StereoInput {
                    id: input,
                    left: &[0.0],
                    right: &[0.0]
                },
                StereoInput {
                    id: input,
                    left: &[0.0],
                    right: &[0.0]
                }
            ],
            &mut [],
            1
        ),
        Err(RenderError::DuplicateInput(_))
    ));
    mixer
        .render(
            &[],
            &mut [StereoOutput {
                id: bus,
                left: &mut l,
                right: &mut r,
            }],
            4,
        )
        .unwrap();
    assert_eq!(l[0], 0.0);
    assert!(l[1] > 0.0);
    assert_eq!(l, r);
}

#[test]
fn render_sanitizes_nonfinite_input_and_keeps_meters_finite() {
    let (session, a, _, x, _) = fixture();
    let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let source = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.25];
    let (mut l, mut r) = ([0.0; 4], [0.0; 4]);
    mixer
        .render(
            &[StereoInput {
                id: a,
                left: &source,
                right: &source,
            }],
            &mut [StereoOutput {
                id: x,
                left: &mut l,
                right: &mut r,
            }],
            4,
        )
        .unwrap();
    assert_eq!(l, [0.0, 0.0, 0.0, 0.25]);
    assert_eq!(l, r);
    let meter = mixer
        .bus_meters()
        .iter()
        .find(|meter| meter.id == x)
        .unwrap();
    assert_eq!(meter.peak, [0.25; 2]);
    assert_eq!(meter.rms, [0.125; 2]);
}

fn process_constant(config: DspConfig, value: f32, frames: usize) -> Vec<f32> {
    let mut session = Session::new("DSP", 48_000);
    let mut input = Input::new(
        "Source",
        InputKind::Virtual {
            endpoint_id: "source".into(),
        },
    );
    input.dsp = config;
    let input_id = session.add_input(input).unwrap();
    let mut bus = Bus::new("Mix");
    bus.dsp = DspConfig::default();
    let bus_id = session.add_bus(bus).unwrap();
    session.add_send(Send::new(input_id, bus_id)).unwrap();
    let mut mixer = CompiledMixer::compile(
        &session,
        ResourceBudget {
            max_block_frames: frames,
            ..Default::default()
        },
    )
    .unwrap();
    let source = vec![value; frames];
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    mixer
        .render(
            &[StereoInput {
                id: input_id,
                left: &source,
                right: &source,
            }],
            &mut [StereoOutput {
                id: bus_id,
                left: &mut left,
                right: &mut right,
            }],
            frames,
        )
        .unwrap();
    assert_eq!(left, right);
    left
}

#[test]
fn highpass_removes_dc_and_gate_rejects_below_threshold_signal() {
    let highpass = process_constant(
        DspConfig {
            high_pass: Some(HighPassConfig { cutoff_hz: 100.0 }),
            ..Default::default()
        },
        0.5,
        4096,
    );
    assert!(highpass[0] > 0.4);
    assert!(highpass[4095].abs() < 1.0e-5);
    let gate = process_constant(
        DspConfig {
            gate: Some(GateConfig::default()),
            ..Default::default()
        },
        0.0001,
        4096,
    );
    assert!(gate.iter().all(|value| *value == 0.0));
    let gate = process_constant(
        DspConfig {
            gate: Some(GateConfig::default()),
            ..Default::default()
        },
        0.5,
        4096,
    );
    assert!(gate[4095] > 0.49);
}

#[test]
fn compressor_reduces_level_by_ratio_and_limiter_contains_first_transient() {
    let compressor = process_constant(
        DspConfig {
            compressor: Some(CompressorConfig {
                threshold_db: -20.0,
                ratio: 4.0,
                attack_ms: 0.01,
                release_ms: 100.0,
                makeup_gain_db: 0.0,
            }),
            ..Default::default()
        },
        1.0,
        1024,
    );
    close(compressor[1023], 10.0_f32.powf(-15.0 / 20.0));
    let limiter = process_constant(
        DspConfig {
            limiter: Some(LimiterConfig {
                ceiling_db: -6.020_6,
                release_ms: 50.0,
            }),
            ..Default::default()
        },
        100.0,
        1024,
    );
    assert!(limiter
        .iter()
        .all(|sample| *sample <= 0.500_001 && *sample > 0.499_99));
}

#[test]
fn bus_master_limits_the_combined_sum() {
    let (mut session, a, b, x, y) = fixture();
    session.buses[0].dsp.limiter = Some(LimiterConfig {
        ceiling_db: -12.041_2,
        release_ms: 50.0,
    });
    let (first, second) = render_pair(&session, a, b, x, y);
    close(first[0], 0.25);
    close(first[1], 0.15);
    assert_eq!(second, [0.3, -0.1]);
}

#[test]
fn rendering_and_meter_reads_allocate_nothing_including_first_callback() {
    let (mut session, a, _, x, _) = fixture();
    session.inputs[0].dsp = DspConfig {
        high_pass: Some(HighPassConfig { cutoff_hz: 80.0 }),
        gate: Some(GateConfig::default()),
        compressor: Some(CompressorConfig::default()),
        limiter: Some(LimiterConfig::default()),
    };
    session.buses[0].dsp = DspConfig::master_default();
    let tone = session
        .add_input(Input::new(
            "Generated tone",
            InputKind::TestTone {
                frequency_hz: 997.0,
                amplitude_db: -30.0,
            },
        ))
        .unwrap();
    session.add_send(Send::new(tone, x)).unwrap();
    let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let source = [0.25; 256];
    let (mut left, mut right) = ([0.0; 256], [0.0; 256]);
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|enabled| enabled.set(true));
    let mut succeeded = true;
    for _ in 0..100 {
        succeeded &= mixer
            .render(
                &[StereoInput {
                    id: a,
                    left: &source,
                    right: &source,
                }],
                &mut [StereoOutput {
                    id: x,
                    left: &mut left,
                    right: &mut right,
                }],
                256,
            )
            .is_ok();
        std::hint::black_box(mixer.input_meters());
        std::hint::black_box(mixer.bus_meters());
    }
    COUNTING.with(|enabled| enabled.set(false));
    assert!(succeeded);
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
}

#[test]
fn applying_and_retargeting_live_ramps_allocate_nothing_on_the_audio_thread() {
    let (mut session, a, _, x, _) = fixture();
    let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let first = PreparedControls::compile(&session, 1024 * 1024).unwrap();
    session.inputs[0].controls.pan = 0.5;
    session.inputs[0].controls.muted = true;
    session.sends[0].controls.set_gain_db(-12.0);
    session.sends[1].controls.muted = true;
    session.buses[0].controls.set_gain_db(-6.0);
    session.buses[1].controls.solo = true;
    let second = PreparedControls::compile(&session, 1024 * 1024).unwrap();
    let source = [0.25; 32];
    let (mut left, mut right) = ([0.0; 32], [0.0; 32]);
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|enabled| enabled.set(true));
    let mut succeeded = true;
    for index in 0..100 {
        succeeded &= mixer
            .apply_controls(if index % 2 == 0 { &second } else { &first })
            .is_ok();
        succeeded &= mixer
            .render(
                &[StereoInput {
                    id: a,
                    left: &source,
                    right: &source,
                }],
                &mut [StereoOutput {
                    id: x,
                    left: &mut left,
                    right: &mut right,
                }],
                32,
            )
            .is_ok();
    }
    COUNTING.with(|enabled| enabled.set(false));
    assert!(succeeded);
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
}

#[test]
fn mapped_source_iterator_preserves_routing_without_descriptor_allocation() {
    let (session, a, b, x, _) = fixture();
    let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let planes = [
        (b, vec![0.3_f32; 128], vec![-0.1_f32; 128]),
        (a, vec![0.2_f32; 128], vec![0.4_f32; 128]),
    ];
    let (mut left, mut right) = ([0.0; 128], [0.0; 128]);
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|enabled| enabled.set(true));
    let result = mixer.render_sources(
        planes.iter().map(|(id, l, r)| StereoInput {
            id: *id,
            left: l,
            right: r,
        }),
        &mut [StereoOutput {
            id: x,
            left: &mut left,
            right: &mut right,
        }],
        128,
    );
    COUNTING.with(|enabled| enabled.set(false));
    result.unwrap();
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(left.iter().all(|s| (*s - 0.5).abs() < 1e-6));
    assert!(right.iter().all(|s| (*s - 0.3).abs() < 1e-6));
    // Duplicate IDs from a mapped iterator still fail before output mutation.
    let saved = left;
    assert_eq!(
        mixer.render_sources(
            planes.iter().map(|(_, l, r)| StereoInput {
                id: a,
                left: l,
                right: r
            }),
            &mut [StereoOutput {
                id: x,
                left: &mut left,
                right: &mut right
            }],
            128
        ),
        Err(RenderError::DuplicateInput(a))
    );
    assert_eq!(left, saved);
}
