use relink_core::{
    Bus, ChannelControls, CompiledMixer, ControlError, DspConfig, HighPassConfig, Input, InputKind,
    PreparedControls, ResourceBudget, Send, Session, SessionError, StereoInput, StereoOutput,
};

#[derive(Clone, Copy, Debug)]
enum Stage {
    Input,
    Send,
    Bus,
}

const STAGES: [Stage; 3] = [Stage::Input, Stage::Send, Stage::Bus];

fn fixture(sample_rate: u32) -> Session {
    let mut session = Session::new("Ramp fixture", sample_rate);
    let input = Input::new(
        "Source",
        InputKind::Hardware {
            device_id: "offline-only".into(),
        },
    );
    let mut bus = Bus::new("Mix");
    bus.dsp = DspConfig::default();
    let send = Send::new(input.id, bus.id);
    session.add_input(input).unwrap();
    session.add_bus(bus).unwrap();
    session.add_send(send).unwrap();
    session
}

fn controls(session: &Session) -> PreparedControls {
    PreparedControls::compile(session, 1024 * 1024).unwrap()
}

fn channel_controls(session: &mut Session, stage: Stage) -> &mut ChannelControls {
    match stage {
        Stage::Input => &mut session.inputs[0].controls,
        Stage::Send => &mut session.sends[0].controls,
        Stage::Bus => &mut session.buses[0].controls,
    }
}

fn set_gains(controls: &mut PreparedControls, stage: Stage, gains: [f32; 2]) {
    match stage {
        Stage::Input => controls.inputs[0].gains = gains,
        Stage::Send => controls.sends[0].gains = gains,
        Stage::Bus => controls.buses[0].gains = gains,
    }
}

fn render(
    mixer: &mut CompiledMixer,
    session: &Session,
    frames: usize,
    external: bool,
) -> [Vec<f32>; 2] {
    let source_left = vec![0.25; frames];
    let source_right = vec![-0.125; frames];
    let sources = [StereoInput {
        id: session.inputs[0].id,
        left: &source_left,
        right: &source_right,
    }];
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    mixer
        .render(
            if external { &sources } else { &[] },
            &mut [StereoOutput {
                id: session.buses[0].id,
                left: &mut left,
                right: &mut right,
            }],
            frames,
        )
        .unwrap();
    [left, right]
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 2.0e-7,
        "actual {actual}, expected {expected}"
    );
}

#[test]
fn gain_changes_follow_a_five_millisecond_continuous_ramp_at_each_stage_and_rate() {
    for sample_rate in [8000, 44100, 48000, 96000, 384000] {
        for stage in STAGES {
            let session = fixture(sample_rate);
            let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
            let count = sample_rate.div_ceil(200) as usize;
            assert_eq!(mixer.control_ramp_frames() as usize, count);
            // Initial graph controls have no implicit startup ramp.
            assert_eq!(
                render(&mut mixer, &session, 1, true),
                [vec![0.25], vec![-0.125]]
            );
            let mut bank = controls(&session);
            let target = [0.2, 0.7];
            set_gains(&mut bank, stage, target);
            mixer.apply_controls(&bank).unwrap();
            let output = render(&mut mixer, &session, count + 1, true);
            for channel in 0..2 {
                let source: f32 = [0.25, -0.125][channel];
                let maximum_step = source.abs() * (target[channel] - 1.0).abs() / count as f32;
                let mut previous = source;
                for (index, sample) in output[channel].iter().copied().enumerate() {
                    let fraction = ((index + 1) as f64 / count as f64).min(1.0);
                    let gain = (1.0 + (f64::from(target[channel]) - 1.0) * fraction) as f32;
                    close(sample, source * gain);
                    assert!((sample - previous).abs() <= maximum_step + 3.0e-8);
                    previous = sample;
                }
                assert_eq!(output[channel][count - 1], source * target[channel]);
                assert_eq!(output[channel][count], source * target[channel]);
            }
        }
    }
}

#[test]
fn pan_and_unlinked_stereo_levels_ramp_to_the_resolved_targets() {
    for stage in STAGES {
        let mut session = fixture(48000);
        let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
        let changed = channel_controls(&mut session, stage);
        changed.set_linked(false);
        changed.set_right_gain_db(-6.0);
        changed.pan = 1.0;
        mixer.apply_controls(&controls(&session)).unwrap();
        let output = render(&mut mixer, &session, 241, true);
        close(output[0][0], 0.25 * 239.0 / 240.0);
        assert_eq!(output[0][239], 0.0);
        assert_eq!(output[0][240], 0.0);
        assert_eq!(output[1][239], -0.125 * 10.0_f32.powf(-6.0 / 20.0));
        assert!(output[1][0] < output[1][239]);
    }
}

#[test]
fn mute_and_unmute_reach_exact_endpoints_at_each_stage() {
    for stage in STAGES {
        let mut session = fixture(48000);
        let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
        channel_controls(&mut session, stage).muted = true;
        mixer.apply_controls(&controls(&session)).unwrap();
        let muted = render(&mut mixer, &session, 300, true);
        for channel in 0..2 {
            let source = [0.25, -0.125][channel];
            close(muted[channel][0], source * 239.0 / 240.0);
            assert!(muted[channel][238].abs() > 0.0);
            assert!(muted[channel][239..].iter().all(|s| s.to_bits() == 0));
        }
        channel_controls(&mut session, stage).muted = false;
        mixer.apply_controls(&controls(&session)).unwrap();
        let restored = render(&mut mixer, &session, 241, true);
        close(restored[0][0], 0.25 / 240.0);
        assert_eq!(restored[0][239..], [0.25, 0.25]);
        close(restored[1][0], -0.125 / 240.0);
        assert_eq!(restored[1][239..], [-0.125, -0.125]);
    }
}

#[test]
fn resolved_input_send_and_bus_solo_changes_use_the_same_ramp() {
    for stage in STAGES {
        let mut session = fixture(48000);
        let second = Input::new(
            "Silent second source",
            InputKind::Hardware {
                device_id: "offline-second".into(),
            },
        );
        let second_id = second.id;
        session.add_input(second).unwrap();
        session
            .add_send(Send::new(second_id, session.buses[0].id))
            .unwrap();
        session.add_bus(Bus::new("Second mix")).unwrap();
        let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
        for solo in [true, false] {
            match stage {
                Stage::Input => session.inputs[1].controls.solo = solo,
                Stage::Send => session.sends[1].controls.solo = solo,
                Stage::Bus => session.buses[1].controls.solo = solo,
            }
            mixer.apply_controls(&controls(&session)).unwrap();
            let output = render(&mut mixer, &session, 241, true);
            for (index, sample) in output[0].iter().enumerate() {
                let fraction = ((index + 1) as f64 / 240.0).min(1.0);
                let activity = if solo { 1.0 - fraction } else { fraction } as f32;
                close(*sample, 0.25 * activity);
            }
            assert_eq!(output[0][239], if solo { 0.0 } else { 0.25 });
        }
    }
}

#[test]
fn stationary_muted_routes_resume_and_finish_retargeted_ramps() {
    let mut session = fixture(48000);
    session.sends[0].controls.muted = true;
    let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    assert_eq!(
        render(&mut mixer, &session, 700, true),
        [vec![0.0; 700], vec![0.0; 700]]
    );
    session.sends[0].controls.muted = false;
    mixer.apply_controls(&controls(&session)).unwrap();
    let opening = render(&mut mixer, &session, 80, true);
    close(opening[0][0], 0.25 / 240.0);
    close(opening[0][79], 0.25 / 3.0);
    session.sends[0].controls.muted = true;
    mixer.apply_controls(&controls(&session)).unwrap();
    let closing = render(&mut mixer, &session, 700, true);
    close(closing[0][0], opening[0][79] * 239.0 / 240.0);
    assert!(
        closing[0][238] > 0.0,
        "a zero target must not skip an active ramp"
    );
    assert!(closing
        .iter()
        .all(|plane| plane[239..].iter().all(|s| *s == 0.0)));
    session.sends[0].controls.muted = false;
    mixer.apply_controls(&controls(&session)).unwrap();
    let reopened = render(&mut mixer, &session, 300, true);
    close(reopened[0][0], 0.25 / 240.0);
    assert!(reopened[0][239..].iter().all(|s| *s == 0.25));
}

#[test]
fn rapid_retarget_starts_from_last_rendered_gain_and_repeated_banks_do_not_extend_it() {
    for stage in STAGES {
        let session = fixture(48000);
        let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
        let mut bank = controls(&session);
        set_gains(&mut bank, stage, [0.0; 2]);
        mixer.apply_controls(&bank).unwrap();
        let falling = render(&mut mixer, &session, 80, true);
        let last_gain = falling[0][79] / 0.25;
        set_gains(&mut bank, stage, [0.9; 2]);
        mixer.apply_controls(&bank).unwrap();
        for index in 0..240 {
            // Publishing an unchanged bank each frame cannot stall smoothing.
            mixer.apply_controls(&bank).unwrap();
            let output = render(&mut mixer, &session, 1, true);
            let gain = (f64::from(last_gain)
                + (f64::from(0.9_f32) - f64::from(last_gain)) * f64::from(index + 1) / 240.0)
                as f32;
            close(output[0][0], 0.25 * gain);
            close(output[1][0], -0.125 * gain);
            if index == 239 {
                assert_eq!(output, [vec![0.25 * 0.9], vec![-0.125 * 0.9]]);
            }
        }
    }
}

#[test]
fn arbitrary_callback_boundaries_are_bit_exact_with_one_contiguous_render() {
    let mut session = fixture(44100);
    session.inputs[0].kind = InputKind::TestTone {
        frequency_hz: 997.0,
        amplitude_db: -12.0,
    };
    session.inputs[0].dsp.high_pass = Some(HighPassConfig { cutoff_hz: 120.0 });
    session.buses[0].dsp = DspConfig::master_default();
    let mut whole = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let mut chunks = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    for phase in 0..3 {
        let mut bank = controls(&session);
        bank.inputs[0].gains = [0.3, 0.7];
        bank.sends[0].gains = [0.8, 0.2];
        bank.buses[0].gains = [0.6, 0.9];
        bank.sends[0].active = phase != 1;
        whole.apply_controls(&bank).unwrap();
        chunks.apply_controls(&bank).unwrap();
        let expected = render(&mut whole, &session, 400, false);
        let mut observed = [Vec::new(), Vec::new()];
        for count in [1, 31, 128, 7, 54, 128, 51] {
            chunks.apply_controls(&bank).unwrap();
            let block = render(&mut chunks, &session, count, false);
            for channel in 0..2 {
                observed[channel].extend_from_slice(&block[channel]);
            }
        }
        assert_eq!(observed, expected);
    }
}

#[test]
fn mute_ramps_preserve_oscillator_phase_and_input_or_bus_filter_history() {
    for stage in [Stage::Input, Stage::Bus] {
        let mut session = fixture(48000);
        session.inputs[0].kind = InputKind::TestTone {
            frequency_hz: 997.0,
            amplitude_db: -12.0,
        };
        let high_pass = Some(HighPassConfig { cutoff_hz: 120.0 });
        match stage {
            Stage::Input => session.inputs[0].dsp.high_pass = high_pass,
            Stage::Bus => session.buses[0].dsp.high_pass = high_pass,
            Stage::Send => unreachable!(),
        }
        let mut actual = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
        let mut reference = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
        assert_eq!(
            render(&mut actual, &session, 400, false),
            render(&mut reference, &session, 400, false)
        );
        for muted in [true, false] {
            channel_controls(&mut session, stage).muted = muted;
            actual.apply_controls(&controls(&session)).unwrap();
            let output = render(&mut actual, &session, 600, false);
            let uninterrupted = render(&mut reference, &session, 600, false);
            for channel in 0..2 {
                for index in 0..600 {
                    let fraction = ((index + 1) as f64 / 240.0).min(1.0);
                    let activity = if muted { 1.0 - fraction } else { fraction } as f32;
                    close(
                        output[channel][index],
                        uninterrupted[channel][index] * activity,
                    );
                }
                if muted {
                    assert!(output[channel][239..].iter().all(|s| s.to_bits() == 0));
                } else {
                    assert_eq!(output[channel][239..], uninterrupted[channel][239..]);
                }
            }
        }
    }
}

#[test]
fn missing_sources_and_omitted_outputs_still_advance_ramps() {
    for stage in STAGES {
        let mut session = fixture(48000);
        let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
        channel_controls(&mut session, stage).muted = true;
        mixer.apply_controls(&controls(&session)).unwrap();
        mixer.render(&[], &mut [], 240).unwrap();
        channel_controls(&mut session, stage).muted = false;
        mixer.apply_controls(&controls(&session)).unwrap();
        let output = render(&mut mixer, &session, 1, true);
        close(output[0][0], 0.25 / 240.0);
        close(output[1][0], -0.125 / 240.0);
    }
}

#[test]
fn invalid_control_banks_and_rejected_or_empty_renders_leave_ramps_untouched() {
    let session = fixture(48000);
    let mut actual = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let mut expected = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let mut bank = controls(&session);
    bank.inputs[0].gains = [0.0; 2];
    for mixer in [&mut actual, &mut expected] {
        mixer.apply_controls(&bank).unwrap();
        render(mixer, &session, 83, true);
    }
    bank.inputs[0].gains = [1.0; 2];
    bank.buses[0].gains = [f32::NAN; 2];
    assert_eq!(actual.apply_controls(&bank), Err(ControlError::InvalidGain));
    assert!(actual
        .render(
            &[StereoInput {
                id: session.inputs[0].id,
                left: &[],
                right: &[]
            }],
            &mut [],
            1,
        )
        .is_err());
    actual.render(&[], &mut [], 0).unwrap();
    assert_eq!(
        render(&mut actual, &session, 200, true),
        render(&mut expected, &session, 200, true)
    );
}

#[test]
fn compiled_budget_includes_ramp_state_and_enforces_the_exact_boundary() {
    let session = fixture(48000);
    let compiled = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let required = compiled.working_bytes();
    let budget = ResourceBudget {
        max_working_bytes: required,
        ..ResourceBudget::default()
    };
    assert_eq!(
        CompiledMixer::compile(&session, budget)
            .unwrap()
            .working_bytes(),
        required
    );
    assert!(matches!(
        CompiledMixer::compile(&session, ResourceBudget {
            max_working_bytes: required - 1,
            ..budget
        }),
        Err(SessionError::ResourceBudget { required: bytes, .. }) if bytes == required
    ));
}
