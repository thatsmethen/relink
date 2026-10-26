use relink_core::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

struct CountingAllocator;
thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static OPERATIONS: Cell<[usize; 3]> = const { Cell::new([0; 3]) };
}
fn count(kind: usize) {
    let _ = COUNTING.try_with(|enabled| {
        if enabled.get() {
            let _ = OPERATIONS.try_with(|operations| {
                let mut value = operations.get();
                value[kind] += 1;
                operations.set(value);
            });
        }
    });
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(0);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count(1);
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(2);
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn measured<T>(operation: impl FnOnce() -> T) -> (T, [usize; 3]) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            COUNTING.with(|enabled| enabled.set(false));
        }
    }
    OPERATIONS.with(|operations| operations.set([0; 3]));
    COUNTING.with(|enabled| enabled.set(true));
    let reset = Reset;
    let value = operation();
    drop(reset);
    (value, OPERATIONS.with(Cell::get))
}
const SOURCE: Uuid = Uuid::from_u128(0x5678);
fn input<'a>(left: &'a [f32], right: &'a [f32]) -> StereoInput<'a> {
    StereoInput {
        id: SOURCE,
        left,
        right,
    }
}

#[test]
fn allocator_control_observes_alloc_realloc_and_dealloc() {
    let (_, operations) = measured(|| {
        let mut samples = Vec::<u8>::with_capacity(8);
        samples.extend([1; 8]);
        samples.reserve_exact(24);
        std::hint::black_box(&samples);
        drop(samples);
    });
    assert!(operations.iter().all(|count| *count > 0), "{operations:?}");
}

#[test]
fn invalid_preparation_rejects_before_any_allocation() {
    let required = SoundCheck::required_bytes(48000, 17).unwrap();
    let (_, operations) = measured(|| {
        assert!(matches!(
            SoundCheck::prepare(SOURCE, 48000, 17, required - 1),
            Err(SoundCheckError::ResourceBudget { .. })
        ));
        assert!(matches!(
            SoundCheck::prepare(SOURCE, 7999, 1, usize::MAX),
            Err(SoundCheckError::InvalidSampleRate)
        ));
        for capacity in [0, 48000 * 30 + 1, usize::MAX] {
            assert!(matches!(
                SoundCheck::prepare(SOURCE, 48000, capacity, usize::MAX),
                Err(SoundCheckError::InvalidCapacity { .. })
            ));
        }
    });
    assert_eq!(operations, [0; 3]);
}

#[test]
fn first_and_repeated_callback_paths_never_allocate_reallocate_or_deallocate() {
    let mut check = SoundCheck::prepare(SOURCE, 48000, 7, 65536).unwrap();
    let samples = [0.25, -0.5, 0.75, -1.0, 0.125, 0.625];
    let mut left = [42.0; 23];
    let mut right = [42.0; 23];
    let (playback, operations) = measured(|| {
        let idle = check.token();
        let record = check.begin_recording(idle).unwrap();
        check
            .capture(record, input(&samples[..3], &samples[..3]), 3)
            .unwrap();
        assert!(matches!(
            check.capture(idle, input(&samples, &samples), 6),
            Err(SoundCheckError::StaleToken)
        ));
        assert!(matches!(
            check.capture(record, input(&samples, &samples[..2]), 6),
            Err(SoundCheckError::InputTooShort { .. })
        ));
        let full = check.capture(record, input(&samples, &samples), 6).unwrap();
        assert_eq!(full.accepted_frames, 4);
        assert!(full.reached_limit);
        assert_eq!(check.clip(full.snapshot.token).unwrap().frames.len(), 7);
        // Moving an allocation is permitted here. Its eventual drop is held
        // outside the measured callback region by returning the new owner.
        let clip = check.take_clip(full.snapshot.token).unwrap();
        let mut playback = SoundCheck::from_clip(clip);
        let play = playback.begin_playback(playback.token(), true).unwrap();
        let looped = playback
            .render_playback(play, &mut left, &mut right)
            .unwrap();
        assert_eq!(looped.wraps, 3);
        for index in 0..23 {
            assert_eq!(
                left[index],
                samples[if index % 7 < 3 {
                    index % 7
                } else {
                    index % 7 - 3
                }]
            );
            assert_eq!(left[index], right[index]);
        }
        let before_error = playback.snapshot();
        assert!(matches!(
            playback.render_playback(play, &mut left, &mut right[..2]),
            Err(SoundCheckError::OutputLengthMismatch)
        ));
        assert_eq!(playback.snapshot(), before_error);
        let ready = playback.stop(play).unwrap();
        assert!(matches!(
            playback.render_playback(play, &mut left, &mut right),
            Err(SoundCheckError::StaleToken)
        ));
        assert_eq!(left, [0.0; 23]);
        assert_eq!(right, [0.0; 23]);
        let play = playback.begin_playback(ready, false).unwrap();
        let ended = playback
            .render_playback(play, &mut left, &mut right)
            .unwrap();
        assert!(ended.finished);
        assert_eq!(ended.rendered_frames, 7);
        assert_eq!(&left[7..], &[0.0; 16]);
        let idle = playback.clear(ended.snapshot.token).unwrap();
        assert!(playback.clip(idle).is_err());
        let record = playback.begin_recording(idle).unwrap();
        playback
            .capture(record, input(&[-0.125], &[0.375]), 1)
            .unwrap();
        let ready = playback.stop(record).unwrap();
        let play = playback.begin_playback(ready, false).unwrap();
        let ended = playback
            .render_playback(play, &mut left, &mut right)
            .unwrap();
        assert_eq!(left[0], -0.125);
        assert_eq!(right[0], 0.375);
        assert_eq!(&left[1..], &[0.0; 22]);
        assert_eq!(&right[1..], &[0.0; 22]);
        let closed = playback.close(ended.snapshot.token).unwrap();
        assert!(playback
            .render_playback(closed, &mut left, &mut right)
            .is_err());
        assert_eq!(left, [0.0; 23]);
        assert_eq!(right, [0.0; 23]);
        playback
    });
    assert_eq!(operations, [0; 3]);
    // Physical erasure and dropping the allocation deliberately occur off callback.
    playback.erase_and_reclaim();
    check.erase_and_reclaim();
}

#[test]
fn immutable_dry_replay_uses_current_effects_once_and_only_explicit_offline_send() {
    let mut session = Session::new("Generated Sound Check effect replay", 48000);
    let mut source = Input::new(
        "Generated source",
        InputKind::Hardware {
            device_id: "in-memory-only".into(),
        },
    );
    source.dsp.compressor = Some(CompressorConfig {
        ratio: 1.0,
        makeup_gain_db: -6.0,
        ..Default::default()
    });
    let source_id = session.add_input(source).unwrap();
    for name in ["Explicit audition", "Unrelated bus"] {
        let mut bus = Bus::new(name);
        bus.dsp = DspConfig::default();
        session.add_bus(bus).unwrap();
    }
    session
        .add_send(Send::new(source_id, session.buses[0].id))
        .unwrap();
    let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let audition = mixer.resolve_bus(session.buses[0].id).unwrap();
    let unrelated = mixer.resolve_bus(session.buses[1].id).unwrap();
    let generated: [f32; 512] = std::array::from_fn(|n| ((n % 17) as f32 - 8.0) * 0.0625);
    let mut check = SoundCheck::prepare(source_id, 48000, 512, 65536).unwrap();
    let record = check.begin_recording(check.token()).unwrap();
    check
        .capture(
            record,
            StereoInput {
                id: source_id,
                left: &generated,
                right: &generated,
            },
            512,
        )
        .unwrap();
    session.inputs[0]
        .dsp
        .compressor
        .as_mut()
        .unwrap()
        .makeup_gain_db = -12.0;
    let changed = PreparedControls::compile(&session, 65536).unwrap();
    session.inputs[0].dsp.bypass.compressor = true;
    let bypassed = PreparedControls::compile(&session, 65536).unwrap();
    let mut left = [0.0; 512];
    let mut right = [0.0; 512];
    let (_, operations) = measured(|| {
        for (controls, gain_db) in [
            (None, -6.0),
            (Some(&changed), -12.0),
            (Some(&bypassed), 0.0),
        ] {
            if let Some(controls) = controls {
                mixer.apply_controls(controls).unwrap();
            }
            let play = check.begin_playback(check.token(), false).unwrap();
            check.render_playback(play, &mut left, &mut right).unwrap();
            assert_eq!(left, generated);
            assert_eq!(right, generated);
            mixer
                .render(
                    &[StereoInput {
                        id: source_id,
                        left: &left,
                        right: &right,
                    }],
                    &mut [],
                    512,
                )
                .unwrap();
            let output = mixer.rendered_bus(audition).unwrap();
            let gain = 10.0_f32.powf(gain_db / 20.0);
            // Current coefficients/bypass settle after the 240-frame ramp.
            // Independent linear-gain oracle distinguishes one pass from two.
            for index in 240..512 {
                assert!((output[index][0] - generated[index] * gain).abs() < 2.0e-7);
                assert_eq!(output[index][0], output[index][1]);
            }
            assert_eq!(mixer.rendered_bus(unrelated).unwrap(), &[[0.0; 2]; 512]);
            let raw = check.clip(check.token()).unwrap();
            for (index, frame) in raw.frames.iter().enumerate() {
                assert_eq!(*frame, [generated[index]; 2]);
            }
        }
    });
    assert_eq!(operations, [0; 3]);
}
