//! Generated buffers only; this test never constructs AudioRuntime or devices.
use relink_audio::{OutputId, SoundCheckAudition, SoundCheckSelection};
use relink_core::{CompressorConfig, Input, InputKind, Session, SoundCheck, StereoInput};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

struct Allocator;
thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<[usize; 3]> = const { Cell::new([0; 3]) };
}
fn count(index: usize) {
    let _ = ENABLED.try_with(|enabled| {
        if enabled.get() {
            let _ = COUNTS.try_with(|counts| {
                let mut value = counts.get();
                value[index] += 1;
                counts.set(value);
            });
        }
    });
}
unsafe impl GlobalAlloc for Allocator {
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
static ALLOCATOR: Allocator = Allocator;
fn measured<T>(work: impl FnOnce() -> T) -> (T, [usize; 3]) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ENABLED.with(|flag| flag.set(false));
        }
    }
    COUNTS.with(|counts| counts.set([0; 3]));
    ENABLED.with(|flag| flag.set(true));
    let reset = Reset;
    let value = work();
    drop(reset);
    (value, COUNTS.with(Cell::get))
}

#[test]
fn instrument_detects_all_three_allocator_operations() {
    let (_, counts) = measured(|| {
        let mut value = Vec::<u8>::with_capacity(8);
        value.extend([1; 8]);
        value.reserve_exact(32);
        std::hint::black_box(&value);
        drop(value);
    });
    assert!(counts.iter().all(|count| *count > 0), "{counts:?}");
}

#[test]
fn first_callback_effect_edits_stale_calls_stop_clear_and_eof_use_no_allocator() {
    let mut studio = Session::new("Generated fixture", 48000);
    let source = studio
        .add_input(Input::new(
            "Chosen",
            InputKind::TestTone {
                frequency_hz: 432.0,
                amplitude_db: -9.0,
            },
        ))
        .unwrap();
    #[cfg(target_os = "macos")]
    let host = cpal::HostId::CoreAudio;
    #[cfg(target_os = "windows")]
    let host = cpal::HostId::Wasapi;
    let selection = SoundCheckSelection::prepare(
        &studio,
        source,
        OutputId(format!("{host}:synthetic-endpoint")),
    )
    .unwrap();
    let mut check = SoundCheck::prepare(source, 48000, 41, 65536).unwrap();
    let token = check.begin_recording(check.token()).unwrap();
    check
        .capture(
            token,
            StereoInput {
                id: source,
                left: &[0.25; 17],
                right: &[-0.5; 17],
            },
            17,
        )
        .unwrap();
    check.stop(token).unwrap();
    let clip = check.take_clip(check.token()).unwrap();
    let (mut control, mut audition) =
        SoundCheckAudition::prepare(clip, selection.clone(), &studio, 1024 * 1024)
            .unwrap_or_else(|e| panic!("{}", e.error));
    let mut left = [7.0; 4097];
    let mut right = [9.0; 4097];
    let (token, counts) = measured(|| {
        let idle = audition.snapshot().token;
        let token = audition.begin_playback(idle, true).unwrap();
        assert!(audition.render(idle, &mut left, &mut right).is_err());
        audition.render(token, &mut left, &mut right).unwrap();
        audition.render(token, &mut left, &mut right).unwrap();
        assert!(audition
            .render(token, &mut left[..5], &mut right[..3])
            .is_err());
        token
    });
    assert_eq!(counts, [0; 3]);
    studio.inputs[0].dsp.compressor = Some(CompressorConfig::default());
    let version = control.update(&studio).unwrap(); // control-thread allocation
    let (_, counts) = measured(|| {
        assert_eq!(
            audition
                .render(token, &mut left, &mut right)
                .unwrap()
                .applied_control_version,
            version
        );
        let stopped = audition.stop(token).unwrap();
        assert!(audition.render(token, &mut left, &mut right).is_err());
        assert_eq!(audition.snapshot().token, stopped);
    });
    assert_eq!(counts, [0; 3]);
    let clip = audition.release().unwrap(); // native owner would prove quiescence first
    let (_, mut audition) = SoundCheckAudition::prepare(clip, selection, &studio, 1024 * 1024)
        .unwrap_or_else(|e| panic!("{}", e.error));
    let (_, counts) = measured(|| {
        let token = audition
            .begin_playback(audition.snapshot().token, false)
            .unwrap();
        let eof = audition.render(token, &mut left, &mut right).unwrap();
        assert_eq!(eof.rendered_frames, 17);
        assert!(eof.finished);
        assert!(left[17..].iter().chain(&right[17..]).all(|v| *v == 0.0));
        let cleared = audition.clear(eof.snapshot.token).unwrap();
        assert!(audition.render(cleared, &mut left, &mut right).is_err());
        assert!(left.iter().chain(&right).all(|v| *v == 0.0));
    });
    assert_eq!(counts, [0; 3]);
    assert!(audition.release().is_none());
}
