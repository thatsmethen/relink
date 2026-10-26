use super::*;

const SOURCE: Uuid = Uuid::from_u128(0x1234);
fn prepared(capacity: usize) -> SoundCheck {
    let bytes = SoundCheck::required_bytes(48000, capacity).unwrap();
    SoundCheck::prepare(SOURCE, 48000, capacity, bytes + 1024).unwrap()
}
fn input<'a>(left: &'a [f32], right: &'a [f32]) -> StereoInput<'a> {
    StereoInput {
        id: SOURCE,
        left,
        right,
    }
}
fn ready(left: &[f32], right: &[f32], capacity: usize) -> SoundCheck {
    let mut check = prepared(capacity);
    let token = check.begin_recording(check.token()).unwrap();
    let progress = check
        .capture(token, input(left, right), left.len())
        .unwrap();
    if progress.snapshot.state == SoundCheckState::Recording {
        check.stop(progress.snapshot.token).unwrap();
    }
    check
}
fn bits(frames: &[[f32; 2]]) -> Vec<[u32; 2]> {
    frames
        .iter()
        .map(|frame| [frame[0].to_bits(), frame[1].to_bits()])
        .collect()
}

#[test]
fn preparation_is_idle_and_checks_duration_rate_and_exact_budget() {
    for rate in [8000, 44100, 48000, 96000, 384000] {
        assert_eq!(
            SoundCheck::maximum_frames(rate).unwrap(),
            rate as usize * 30
        );
        for capacity in [1, 37, rate as usize * 30] {
            let required = SoundCheck::required_bytes(rate, capacity).unwrap();
            assert!(
                matches!(SoundCheck::prepare(SOURCE, rate, capacity, required - 1), Err(SoundCheckError::ResourceBudget { required: actual, allowed }) if actual == required && allowed == required - 1)
            );
        }
        assert!(matches!(
            SoundCheck::required_bytes(rate, rate as usize * 30 + 1),
            Err(SoundCheckError::InvalidCapacity { .. })
        ));
    }
    for rate in [0, 7999, 384001, u32::MAX] {
        assert_eq!(
            SoundCheck::maximum_frames(rate),
            Err(SoundCheckError::InvalidSampleRate)
        );
    }
    for capacity in [0, usize::MAX] {
        assert!(matches!(
            SoundCheck::required_bytes(48000, capacity),
            Err(SoundCheckError::InvalidCapacity { .. })
        ));
    }
    let mut check = prepared(37);
    assert_eq!(check.snapshot().state, SoundCheckState::Idle);
    assert_eq!(check.snapshot().recorded_frames, 0);
    assert_eq!(check.snapshot().capacity_frames, 37);
    assert_eq!(check.snapshot().source, SOURCE);
    assert!(check.working_bytes() >= SoundCheck::required_bytes(48000, 37).unwrap());
    assert_eq!(
        check.begin_playback(check.token(), false),
        Err(SoundCheckError::EmptyClip)
    );
    assert!(matches!(
        check.clip(check.token()),
        Err(SoundCheckError::InvalidState(SoundCheckState::Idle))
    ));
}

#[test]
fn surplus_allocation_is_charged_without_extending_logical_capture_duration() {
    let mut check = prepared(7);
    // Model allocator excess explicitly; playback/capture still expose seven
    // initialized frames. Public preparation never permits runtime growth.
    check.storage.reserve_exact(31);
    let allocated = check.storage.capacity() * size_of::<[f32; 2]>();
    assert!(allocated > 7 * size_of::<[f32; 2]>());
    assert_eq!(check.allocated_bytes(), allocated);
    assert_eq!(check.working_bytes(), size_of::<SoundCheck>() + allocated);
    let record = check.begin_recording(check.token()).unwrap();
    let completed = check
        .capture(record, input(&[0.25; 9], &[-0.75; 9]), 9)
        .unwrap();
    assert_eq!(completed.accepted_frames, 7);
    assert_eq!(completed.snapshot.capacity_frames, 7);
    let clip = check.take_clip(completed.snapshot.token).unwrap();
    assert_eq!(clip.capacity_frames(), 7);
    assert_eq!(clip.view().frames.len(), 7);
    assert_eq!(clip.allocated_bytes(), allocated);
    assert_eq!(
        clip.working_bytes(),
        size_of::<SoundCheckClip>() + allocated
    );
    assert_eq!(check.allocated_bytes(), 0);
    let resumed = SoundCheck::from_clip(clip);
    assert_eq!(resumed.allocated_bytes(), allocated);
    assert_eq!(resumed.working_bytes(), size_of::<SoundCheck>() + allocated);
    assert_eq!(resumed.snapshot().capacity_frames, 7);
}

#[test]
fn selected_source_and_whole_input_shape_validate_before_any_capture_mutation() {
    let mut check = prepared(7);
    let idle = check.token();
    let token = check.begin_recording(idle).unwrap();
    let before = check.snapshot();
    let samples = [1.0, 2.0, 3.0, 4.0];
    assert_eq!(
        check.capture(idle, input(&samples, &samples), 4),
        Err(SoundCheckError::StaleToken)
    );
    let wrong = StereoInput {
        id: Uuid::from_u128(0x9876),
        left: &samples,
        right: &samples,
    };
    assert!(matches!(
        check.capture(token, wrong, 4),
        Err(SoundCheckError::SourceMismatch { .. })
    ));
    assert!(matches!(
        check.capture(token, input(&samples, &samples[..2]), 4),
        Err(SoundCheckError::InputTooShort { .. })
    ));
    assert_eq!(check.snapshot(), before);
    assert!(check.storage.iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(
        check.capture(token, input(&[], &[]), 0).unwrap().snapshot,
        before
    );
    check.capture(token, input(&samples, &samples), 4).unwrap();
    let other_left = [-1.0, -2.0, -3.0, -4.0, -5.0];
    let other_right = [10.0, 20.0, 30.0, 40.0, 50.0];
    let full = check
        .capture(token, input(&other_left, &other_right), 5)
        .unwrap();
    assert_eq!(full.accepted_frames, 3);
    assert!(full.reached_limit);
    assert_eq!(full.snapshot.state, SoundCheckState::Ready);
    assert_eq!(full.snapshot.recorded_frames, 7);
    assert_ne!(full.snapshot.token, token);
    assert_eq!(
        check.clip(full.snapshot.token).unwrap().frames,
        [
            [1.0, 1.0],
            [2.0, 2.0],
            [3.0, 3.0],
            [4.0, 4.0],
            [-1.0, 10.0],
            [-2.0, 20.0],
            [-3.0, 30.0]
        ]
    );
    assert_eq!(
        check.capture(token, input(&samples, &samples), 4),
        Err(SoundCheckError::StaleToken)
    );
    assert!(matches!(
        check.capture(full.snapshot.token, input(&samples, &samples), 4),
        Err(SoundCheckError::InvalidState(SoundCheckState::Ready))
    ));
}

#[test]
fn captured_pcm_is_owned_immutable_and_bit_exact_before_any_effects() {
    let mut left = [0.0, -0.0, f32::from_bits(0x7fc01234), f32::INFINITY, -17.25];
    let mut right = [
        -0.0,
        0.0,
        f32::from_bits(0xffc05678),
        f32::NEG_INFINITY,
        0.0000001,
    ];
    let expected: Vec<_> = left
        .iter()
        .zip(&right)
        .map(|(&l, &r)| [l.to_bits(), r.to_bits()])
        .collect();
    let mut check = ready(&left, &right, 10);
    left.fill(42.0);
    right.fill(-42.0);
    assert_eq!(bits(check.clip(check.token()).unwrap().frames), expected);
    let token = check.begin_playback(check.token(), false).unwrap();
    let mut played_left = [0.0; 7];
    let mut played_right = [0.0; 7];
    check
        .render_playback(token, &mut played_left, &mut played_right)
        .unwrap();
    let played: Vec<_> = played_left[..5]
        .iter()
        .zip(&played_right[..5])
        .map(|(&l, &r)| [l.to_bits(), r.to_bits()])
        .collect();
    assert_eq!(played, expected);
    assert_eq!(&played_left[5..], &[0.0; 2]);
    assert_eq!(&played_right[5..], &[0.0; 2]);
}

#[test]
fn cap_is_exact_thirty_seconds_with_a_partial_final_callback() {
    let rate = 8000;
    let capacity = SoundCheck::maximum_frames(rate).unwrap();
    let bytes = SoundCheck::required_bytes(rate, capacity).unwrap();
    let mut check = SoundCheck::prepare(SOURCE, rate, capacity, bytes).unwrap();
    let token = check.begin_recording(check.token()).unwrap();
    let left: Vec<_> = (0..1021).map(|n| n as f32 + 0.25).collect();
    let right: Vec<_> = (0..1021).map(|n| -n as f32 - 0.75).collect();
    let mut recorded = 0;
    loop {
        let progress = check
            .capture(token, input(&left, &right), left.len())
            .unwrap();
        recorded += progress.accepted_frames;
        if progress.reached_limit {
            assert_eq!(progress.accepted_frames, capacity % left.len());
            assert_eq!(progress.snapshot.state, SoundCheckState::Ready);
            break;
        }
    }
    assert_eq!(recorded, capacity);
    let clip = check.clip(check.token()).unwrap();
    for (index, frame) in clip.frames.iter().enumerate() {
        assert_eq!(
            *frame,
            [left[index % left.len()], right[index % right.len()]]
        );
    }
}

#[test]
fn nonloop_playhead_stop_rewind_and_tail_silence_are_exact() {
    let mut check = ready(
        &[1.0, 2.0, 3.0, 4.0, 5.0],
        &[-1.0, -2.0, -3.0, -4.0, -5.0],
        9,
    );
    let ready_token = check.token();
    let token = check.begin_playback(ready_token, false).unwrap();
    let mut left = [99.0; 3];
    let mut right = [99.0; 3];
    let first = check.render_playback(token, &mut left, &mut right).unwrap();
    assert_eq!(left, [1.0, 2.0, 3.0]);
    assert_eq!(right, [-1.0, -2.0, -3.0]);
    assert_eq!(first.rendered_frames, 3);
    assert_eq!(first.snapshot.playhead_frames, 3);
    assert!(!first.finished);
    let mut left = [99.0; 4];
    let mut right = [99.0; 4];
    let last = check.render_playback(token, &mut left, &mut right).unwrap();
    assert_eq!(left, [4.0, 5.0, 0.0, 0.0]);
    assert_eq!(right, [-4.0, -5.0, 0.0, 0.0]);
    assert_eq!(last.rendered_frames, 2);
    assert!(last.finished);
    assert_eq!(last.wraps, 0);
    assert_eq!(last.snapshot.state, SoundCheckState::Ready);
    assert_eq!(last.snapshot.playhead_frames, 0);
    left.fill(99.0);
    right.fill(99.0);
    assert_eq!(
        check.render_playback(token, &mut left, &mut right),
        Err(SoundCheckError::StaleToken)
    );
    assert_eq!(left, [0.0; 4]);
    assert_eq!(right, [0.0; 4]);
    assert_eq!(
        check.begin_playback(ready_token, false),
        Err(SoundCheckError::StaleToken)
    );
    let token = check.begin_playback(check.token(), false).unwrap();
    check.render_playback(token, &mut left, &mut right).unwrap();
    let stopped = check.stop(token).unwrap();
    assert_eq!(check.snapshot().playhead_frames, 0);
    let token = check.begin_playback(stopped, false).unwrap();
    check.render_playback(token, &mut left, &mut right).unwrap();
    assert_eq!(left, [1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn explicit_looping_is_partition_exact_and_never_smooths_or_rewrites_clip() {
    let source_left = [0.25, -0.5, 0.75, 1.0, -1.25];
    let source_right = [1.5, 1.75, -2.0, 2.25, 2.5];
    let run = |parts: &[usize]| {
        let mut check = ready(&source_left, &source_right, 13);
        let before = bits(check.clip(check.token()).unwrap().frames);
        let token = check.begin_playback(check.token(), true).unwrap();
        let mut all = Vec::new();
        let mut wraps = 0;
        for &count in parts {
            let mut left = vec![99.0; count];
            let mut right = vec![99.0; count];
            let progress = check.render_playback(token, &mut left, &mut right).unwrap();
            assert!(!progress.finished);
            assert_eq!(progress.rendered_frames, count);
            wraps += progress.wraps;
            all.extend(left.into_iter().zip(right).map(|(l, r)| [l, r]));
        }
        assert_eq!(wraps, all.len() / source_left.len());
        assert_eq!(
            check.snapshot().playhead_frames,
            all.len() % source_left.len()
        );
        assert_eq!(check.token(), token);
        check.stop(token).unwrap();
        assert_eq!(bits(check.clip(check.token()).unwrap().frames), before);
        all
    };
    let whole = run(&[41]);
    assert_eq!(whole, run(&[0, 1, 7, 3, 0, 9, 21]));
    for (index, frame) in whole.iter().enumerate() {
        assert_eq!(*frame, [source_left[index % 5], source_right[index % 5]]);
    }
}

#[test]
fn clear_and_shorter_replacement_never_expose_old_pcm_even_with_current_token() {
    let mut check = ready(
        &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        &[-1.0, -2.0, -3.0, -4.0, -5.0, -6.0],
        9,
    );
    let old_ready = check.token();
    let playing = check.begin_playback(old_ready, true).unwrap();
    let clear = check.clear(playing).unwrap();
    assert_eq!(check.snapshot().state, SoundCheckState::Idle);
    assert_eq!(check.snapshot().recorded_frames, 0);
    assert_eq!(check.snapshot().capacity_frames, 9); // storage was not deallocated
    assert_eq!(
        check.begin_playback(clear, false),
        Err(SoundCheckError::EmptyClip)
    );
    assert!(check.clip(clear).is_err());
    assert!(check.take_clip(clear).is_err());
    assert_eq!(
        check.begin_playback(old_ready, false),
        Err(SoundCheckError::StaleToken)
    );
    let new_capture = check.begin_recording(clear).unwrap();
    assert_eq!(
        check.capture(playing, input(&[90.0], &[90.0]), 1),
        Err(SoundCheckError::StaleToken)
    );
    check
        .capture(new_capture, input(&[7.0, 8.0], &[-7.0, -8.0]), 2)
        .unwrap();
    check.stop(new_capture).unwrap();
    assert_eq!(
        check.clip(check.token()).unwrap().frames,
        [[7.0, -7.0], [8.0, -8.0]]
    );
    let play = check.begin_playback(check.token(), false).unwrap();
    let mut left = [90.0; 9];
    let mut right = [90.0; 9];
    check.render_playback(play, &mut left, &mut right).unwrap();
    assert_eq!(left, [7.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(right, [-7.0, -8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
}

#[test]
fn stop_empty_recording_and_cross_session_tokens_cannot_enable_playback() {
    let mut check = prepared(5);
    let other = prepared(5);
    assert_eq!(
        check.begin_recording(other.token()),
        Err(SoundCheckError::StaleToken)
    );
    let recording = check.begin_recording(check.token()).unwrap();
    assert_eq!(
        check.begin_playback(recording, false),
        Err(SoundCheckError::InvalidState(SoundCheckState::Recording))
    );
    let stopped = check.stop(recording).unwrap();
    assert_eq!(check.snapshot().state, SoundCheckState::Idle);
    assert_eq!(
        check.begin_playback(stopped, false),
        Err(SoundCheckError::EmptyClip)
    );
    assert_eq!(
        check.capture(recording, input(&[1.0], &[1.0]), 1),
        Err(SoundCheckError::StaleToken)
    );
    assert_eq!(
        check.capture(stopped, input(&[1.0], &[1.0]), 1),
        Err(SoundCheckError::InvalidState(SoundCheckState::Idle))
    );
}

#[test]
fn close_is_permanent_and_all_stale_or_invalid_playback_calls_zero_destinations() {
    let mut check = ready(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0], 7);
    let ready = check.token();
    let playing = check.begin_playback(ready, true).unwrap();
    let before = check.snapshot();
    let mut left = [99.0; 3];
    let mut right = [99.0; 2];
    assert_eq!(
        check.render_playback(playing, &mut left, &mut right),
        Err(SoundCheckError::OutputLengthMismatch)
    );
    assert_eq!(left, [0.0; 3]);
    assert_eq!(right, [0.0; 2]);
    assert_eq!(check.snapshot(), before);
    let closed = check.close(playing).unwrap();
    assert_eq!(check.snapshot().state, SoundCheckState::Closed);
    assert_eq!(check.snapshot().recorded_frames, 0);
    assert!(check.begin_recording(closed).is_err());
    assert!(check.begin_playback(closed, true).is_err());
    assert!(check.clip(closed).is_err());
    assert!(check.take_clip(closed).is_err());
    for token in [ready, playing, closed] {
        left.fill(99.0);
        right.fill(99.0);
        assert!(check.render_playback(token, &mut left, &mut right).is_err());
        assert_eq!(left, [0.0; 3]);
        assert_eq!(right, [0.0; 2]);
    }
}

#[test]
fn owned_clip_transfer_closes_old_owner_and_preserves_only_exact_dry_length() {
    let mut recorder = ready(&[0.25, -0.5, 0.75], &[-1.0, 1.25, -1.5], 11);
    let before_transfer = recorder.token();
    let clip = recorder.take_clip(before_transfer).unwrap();
    assert_eq!(recorder.snapshot().state, SoundCheckState::Closed);
    assert_eq!(recorder.snapshot().capacity_frames, 0);
    assert!(recorder.clip(recorder.token()).is_err());
    assert_eq!(clip.capacity_frames(), 11);
    assert_eq!(
        clip.view().frames,
        [[0.25, -1.0], [-0.5, 1.25], [0.75, -1.5]]
    );
    let mut playback = SoundCheck::from_clip(clip);
    assert_ne!(playback.token(), before_transfer);
    assert_eq!(playback.snapshot().state, SoundCheckState::Ready);
    let token = playback.begin_playback(playback.token(), false).unwrap();
    let mut left = [99.0; 7];
    let mut right = [99.0; 7];
    playback
        .render_playback(token, &mut left, &mut right)
        .unwrap();
    assert_eq!(left, [0.25, -0.5, 0.75, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(right, [-1.0, 1.25, -1.5, 0.0, 0.0, 0.0, 0.0]);
    assert!(recorder.begin_recording(recorder.token()).is_err());
}

#[test]
fn control_thread_erasure_clears_initialized_slots_and_refuses_live_clip() {
    let mut check = ready(
        &[1.0, 2.0, 3.0, 4.0, 5.0],
        &[-1.0, -2.0, -3.0, -4.0, -5.0],
        9,
    );
    assert!(check.erase_cleared_storage(check.token()).is_err());
    let cleared = check.clear(check.token()).unwrap();
    assert_eq!(check.storage[4], [5.0, -5.0]); // logical deletion is immediate; physical erasure deferred
    check.erase_cleared_storage(cleared).unwrap();
    assert!(check.storage.iter().all(|frame| *frame == [0.0; 2]));
    let recording = check.begin_recording(cleared).unwrap();
    check.capture(recording, input(&[7.0], &[-7.0]), 1).unwrap();
    let closed = check.close(recording).unwrap();
    check.erase_cleared_storage(closed).unwrap();
    assert!(check.storage.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn generation_and_session_identity_exhaustion_fail_closed_without_token_reuse() {
    let counter = AtomicU64::new(u64::MAX - 1);
    assert_eq!(next_session(&counter), Ok(u64::MAX - 1));
    assert_eq!(
        next_session(&counter),
        Err(SoundCheckError::SessionIdsExhausted)
    );
    assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    let mut check = prepared(1);
    check.token.generation = u64::MAX - 1;
    let recording = check.begin_recording(check.token()).unwrap();
    assert_eq!(
        check.capture(recording, input(&[1.0], &[2.0]), 1),
        Err(SoundCheckError::GenerationExhausted)
    );
    assert_eq!(check.snapshot().state, SoundCheckState::Closed);
    assert_eq!(check.snapshot().recorded_frames, 0);
    assert_eq!(check.storage, [[0.0; 2]]);
    let mut check = ready(&[1.0], &[2.0], 3);
    check.token.generation = u64::MAX - 1;
    let playing = check.begin_playback(check.token(), false).unwrap();
    let mut left = [99.0; 3];
    let mut right = [99.0; 3];
    assert_eq!(
        check.render_playback(playing, &mut left, &mut right),
        Err(SoundCheckError::GenerationExhausted)
    );
    assert_eq!(left, [0.0; 3]);
    assert_eq!(right, [0.0; 3]);
    assert_eq!(check.snapshot().state, SoundCheckState::Closed);
    assert!(check.clip(check.token()).is_err());
}
