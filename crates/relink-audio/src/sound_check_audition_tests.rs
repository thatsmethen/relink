use super::*;
use relink_core::{Bus, CompressorConfig, HighPassConfig, Input, InputKind, Send};

fn studio() -> (Session, Uuid, SoundCheckSelection) {
    let mut session = Session::new("Generated audition", 48000);
    let source = session
        .add_input(Input::new(
            "Selected",
            InputKind::TestTone {
                frequency_hz: 440.0,
                amplitude_db: -18.0,
            },
        ))
        .unwrap();
    let other = session
        .add_input(Input::new(
            "Unselected",
            InputKind::TestTone {
                frequency_hz: 903.0,
                amplitude_db: -3.0,
            },
        ))
        .unwrap();
    for name in ["Stream", "Monitor"] {
        let bus = session.add_bus(Bus::new(name)).unwrap();
        session.add_send(Send::new(source, bus)).unwrap();
        session.add_send(Send::new(other, bus)).unwrap();
    }
    #[cfg(target_os = "macos")]
    let host = cpal::HostId::CoreAudio;
    #[cfg(target_os = "windows")]
    let host = cpal::HostId::Wasapi;
    let selection = SoundCheckSelection::prepare(
        &session,
        source,
        crate::OutputId(format!("{host}:generated-only")),
    )
    .unwrap();
    (session, source, selection)
}
fn clip(source: Uuid, samples: &[[f32; 2]], capacity: usize) -> SoundCheckClip {
    let mut check = SoundCheck::prepare(source, 48000, capacity, GRAPH_BYTES).unwrap();
    let token = check.begin_recording(check.token()).unwrap();
    let left: Vec<_> = samples.iter().map(|s| s[0]).collect();
    let right: Vec<_> = samples.iter().map(|s| s[1]).collect();
    let progress = check
        .capture(
            token,
            StereoInput {
                id: source,
                left: &left,
                right: &right,
            },
            samples.len(),
        )
        .unwrap();
    if !progress.reached_limit {
        check.stop(progress.snapshot.token).unwrap();
    }
    check.take_clip(check.token()).unwrap()
}
fn prepared(
    samples: &[[f32; 2]],
    hp: bool,
) -> (Session, SoundCheckAuditionControl, SoundCheckAudition) {
    let (mut session, source, selection) = studio();
    if hp {
        session.inputs[0].dsp.high_pass = Some(HighPassConfig { cutoff_hz: 120.0 });
    }
    let (control, audition) = SoundCheckAudition::prepare(
        clip(source, samples, samples.len() + 41),
        selection,
        &session,
        GRAPH_BYTES,
    )
    .unwrap_or_else(|error| panic!("{}", error.error));
    (session, control, audition)
}

#[test]
fn preparation_is_idle_and_partial_eof_never_emits_hidden_tone_or_dsp_tail() {
    let samples: Vec<_> = (0..RENDER_CHUNK + 7)
        .map(|i| if i == 0 { [0.8, -0.6] } else { [0.0; 2] })
        .collect();
    let (_, _, mut audition) = prepared(&samples, true);
    let idle = audition.snapshot().token;
    let mut left = [17.0; 4096];
    let mut right = [19.0; 4096];
    assert!(audition.render(idle, &mut left, &mut right).is_err());
    assert!(left.iter().chain(&right).all(|s| *s == 0.0));
    let token = audition.begin_playback(idle, false).unwrap();
    let progress = audition.render(token, &mut left, &mut right).unwrap();
    assert!(progress.finished);
    assert_eq!(progress.rendered_frames, samples.len());
    assert_eq!(progress.snapshot.state, SoundCheckState::Ready);
    assert_eq!(progress.snapshot.playhead_frames, 0);
    assert!(left[..samples.len()].iter().any(|s| *s != 0.0));
    assert!(left[samples.len()..]
        .iter()
        .chain(&right[samples.len()..])
        .all(|s| *s == 0.0));
    assert!(audition.render(token, &mut left, &mut right).is_err());
    assert!(left.iter().chain(&right).all(|s| *s == 0.0));
    assert_eq!(
        audition.begin_playback(progress.snapshot.token, false),
        Err(SoundCheckAuditionError::AlreadyStarted)
    );
    let returned = audition.release().unwrap();
    assert_eq!(returned.view().frames, samples);
    returned.erase_and_reclaim();
}

#[test]
fn exact_loop_is_partition_independent_with_continuous_current_effect_history() {
    let samples = [
        [0.25, -0.75],
        [0.5, -0.25],
        [0.0, 1.2],
        [-0.8, 0.4],
        [0.1, -0.2],
    ];
    let (_, _, mut whole) = prepared(&samples, true);
    let (_, _, mut split) = prepared(&samples, true);
    let whole_token = whole.begin_playback(whole.snapshot().token, true).unwrap();
    let split_token = split.begin_playback(split.snapshot().token, true).unwrap();
    let mut left = [0.0; 7049];
    let mut right = [0.0; 7049];
    let result = whole.render(whole_token, &mut left, &mut right).unwrap();
    assert_eq!(result.wraps, 7049 / samples.len());
    let mut split_left = [0.0; 7049];
    let mut split_right = [0.0; 7049];
    let mut offset = 0;
    for count in [1, 3, 2047, 0, 2, 4096, 900] {
        split
            .render(
                split_token,
                &mut split_left[offset..offset + count],
                &mut split_right[offset..offset + count],
            )
            .unwrap();
        offset += count;
    }
    assert_eq!(offset, left.len());
    assert_eq!(left, split_left);
    assert_eq!(right, split_right);
    whole.release().unwrap().erase_and_reclaim();
    split.release().unwrap().erase_and_reclaim();
}

#[test]
fn current_controls_apply_once_and_rejected_edits_preserve_clip_and_bank() {
    let (mut session, source, selection) = studio();
    session.inputs[0].controls.set_gain_db(-6.0);
    session.inputs[0].dsp.compressor = Some(CompressorConfig {
        ratio: 1.0,
        makeup_gain_db: -3.0,
        ..Default::default()
    });
    let samples = [[2.0, -0.5]; 11];
    let (mut control, mut audition) =
        SoundCheckAudition::prepare(clip(source, &samples, 27), selection, &session, GRAPH_BYTES)
            .unwrap_or_else(|e| panic!("{}", e.error));
    let token = audition
        .begin_playback(audition.snapshot().token, true)
        .unwrap();
    let mut left = [0.0; 512];
    let mut right = [0.0; 512];
    audition.render(token, &mut left, &mut right).unwrap();
    let gain = 10.0_f32.powf(-9.0 / 20.0);
    assert!(left.iter().all(|v| (*v - 2.0 * gain).abs() < 1e-6));
    assert!(right.iter().all(|v| (*v + 0.5 * gain).abs() < 1e-6));
    session.inputs[0].controls.set_gain_db(0.0);
    session.inputs[0].dsp.bypass.compressor = true;
    let version = control.update(&session).unwrap();
    let empty = audition.render(token, &mut [], &mut []).unwrap();
    assert_eq!(empty.applied_control_version, 0);
    let applied = audition.render(token, &mut left, &mut right).unwrap();
    assert_eq!(applied.applied_control_version, version);
    assert!(
        left[240..].iter().all(|v| (*v - 2.0).abs() < 1e-6),
        "no private master limiter or unselected source"
    );
    assert!(right[240..].iter().all(|v| (*v + 0.5).abs() < 1e-6));
    session.inputs[0].kind = InputKind::TestTone {
        frequency_hz: 221.0,
        amplitude_db: -12.0,
    };
    assert!(control.update(&session).is_err());
    assert_eq!(
        audition
            .render(token, &mut left, &mut right)
            .unwrap()
            .applied_control_version,
        version
    );
    let returned = audition.release().unwrap();
    assert_eq!(returned.view().frames, samples);
    returned.erase_and_reclaim();
}

#[test]
fn stop_clear_and_invalid_buffers_silence_every_destination_without_advancing() {
    let (_, _, mut audition) = prepared(&[[1.0, -1.0]; 17], true);
    let token = audition
        .begin_playback(audition.snapshot().token, true)
        .unwrap();
    let mut left = [42.0; 33];
    let mut right = [42.0; 17];
    let before = audition.snapshot();
    assert_eq!(
        audition.render(token, &mut left, &mut right),
        Err(SoundCheckAuditionError::OutputLengthMismatch)
    );
    assert_eq!(audition.snapshot(), before);
    assert!(left.iter().chain(&right).all(|s| *s == 0.0));
    let mut large_l = vec![42.0; MAX_CALLBACK_FRAMES + 1];
    let mut large_r = large_l.clone();
    assert_eq!(
        audition.render(token, &mut large_l, &mut large_r),
        Err(SoundCheckAuditionError::CallbackTooLarge)
    );
    assert_eq!(audition.snapshot(), before);
    assert!(large_l.iter().chain(&large_r).all(|s| *s == 0.0));
    audition.render(token, &mut left[..17], &mut right).unwrap();
    let stopped = audition.stop(token).unwrap();
    assert!(audition.render(token, &mut left[..17], &mut right).is_err());
    let cleared = audition.clear(stopped).unwrap();
    assert!(audition
        .render(cleared, &mut left[..17], &mut right)
        .is_err());
    assert!(left.iter().chain(&right).all(|s| *s == 0.0));
    assert!(audition
        .left
        .iter()
        .chain(&audition.right)
        .all(|s| *s == 0.0));
    assert!(audition.release().is_none());
}

#[test]
fn budget_and_identity_failure_return_the_same_unique_clip() {
    let (session, source, selection) = studio();
    let dry = clip(source, &[[0.25, -0.5]; 17], 100);
    let pointer = dry.view().frames.as_ptr();
    let error = match SoundCheckAudition::prepare(dry, selection.clone(), &session, 1) {
        Err(error) => error,
        Ok(_) => panic!("budget should reject"),
    };
    assert_eq!(error.clip.view().frames.as_ptr(), pointer);
    let mut changed = session.clone();
    changed.sample_rate = 96000;
    let error =
        match SoundCheckAudition::prepare(*error.clip, selection.clone(), &changed, GRAPH_BYTES) {
            Err(error) => error,
            Ok(_) => panic!("identity should reject"),
        };
    assert_eq!(error.clip.view().frames.as_ptr(), pointer);
    let (_, audition) = SoundCheckAudition::prepare(*error.clip, selection, &session, GRAPH_BYTES)
        .unwrap_or_else(|e| panic!("{}", e.error));
    assert!(audition.working_bytes() <= GRAPH_BYTES);
    assert!(audition.working_bytes() >= 100 * 8 + 4 * RENDER_CHUNK * size_of::<f32>());
    let returned = audition.release().unwrap();
    assert_eq!(returned.view().frames.as_ptr(), pointer);
    returned.erase_and_reclaim();
}

#[test]
fn exact_working_budget_boundary_charges_capacity_and_preserves_rejected_clip() {
    let (session, source, selection) = studio();
    let samples = [[0.25, -0.5]; 17];
    let mut measurements = Vec::new();
    for capacity in [100, 100_000] {
        let dry = clip(source, &samples, capacity);
        let pointer = dry.view().frames.as_ptr();
        let allocated = dry.allocated_bytes();
        let (control, audition) =
            SoundCheckAudition::prepare(dry, selection.clone(), &session, GRAPH_BYTES)
                .unwrap_or_else(|error| panic!("{}", error.error));
        let required = audition.working_bytes();
        let dry = audition.release().unwrap();
        drop(control);
        let failure =
            match SoundCheckAudition::prepare(dry, selection.clone(), &session, required - 1) {
                Err(error) => error,
                Ok(_) => panic!("one byte below the reported requirement must reject"),
            };
        assert_eq!(failure.clip.view().frames.as_ptr(), pointer);
        assert_eq!(failure.clip.view().frames, samples);
        assert_eq!(failure.clip.allocated_bytes(), allocated);
        let (control, audition) =
            SoundCheckAudition::prepare(*failure.clip, selection.clone(), &session, required)
                .unwrap_or_else(|error| panic!("exact budget should succeed: {}", error.error));
        assert_eq!(audition.working_bytes(), required);
        let returned = audition.release().unwrap();
        assert_eq!(returned.view().frames.as_ptr(), pointer);
        assert_eq!(returned.view().frames, samples);
        drop(control);
        returned.erase_and_reclaim();
        measurements.push((required, allocated));
    }
    // Both clips have the same recorded length. Unused owned PCM capacity must
    // still be charged, rather than budgeting only the 17 playable frames.
    assert_eq!(
        measurements[1].0 - measurements[0].0,
        measurements[1].1 - measurements[0].1,
    );
}
