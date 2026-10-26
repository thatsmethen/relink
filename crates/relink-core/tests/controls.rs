use relink_core::{
    Bus, CompiledMixer, ControlError, DspConfig, HighPassConfig, Input, InputKind,
    PreparedControls, ResourceBudget, Send, Session, StereoInput, StereoOutput,
};

fn fixture() -> Session {
    let mut session = Session::new("Control fixture", 48000);
    let input = Input::new(
        "External",
        InputKind::Hardware {
            device_id: "offline-fixture".into(),
        },
    );
    let bus = Bus::new("Mix");
    let route = Send::new(input.id, bus.id);
    session.add_input(input).unwrap();
    session.add_bus(bus).unwrap();
    session.add_send(route).unwrap();
    session
}

#[test]
fn malformed_late_control_does_not_partially_apply_earlier_controls() {
    let session = fixture();
    let mut mixer = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let mut controls = PreparedControls::compile(&session, 1024 * 1024).unwrap();
    controls.inputs[0].gains = [0.0; 2];
    controls.buses[0].gains[0] = f32::NAN;
    assert_eq!(
        mixer.apply_controls(&controls),
        Err(ControlError::InvalidGain)
    );
    let input = [0.25_f32; 8];
    let mut l = [0.0; 8];
    let mut r = [0.0; 8];
    mixer
        .render(
            &[StereoInput {
                id: session.inputs[0].id,
                left: &input,
                right: &input,
            }],
            &mut [StereoOutput {
                id: session.buses[0].id,
                left: &mut l,
                right: &mut r,
            }],
            8,
        )
        .unwrap();
    assert_eq!(l, input);
    assert_eq!(r, input);
    controls.buses[0].gains = [1.0; 2];
    controls.sends.clear();
    assert_eq!(
        mixer.apply_controls(&controls),
        Err(ControlError::LayoutMismatch)
    );
}

#[test]
fn controls_preserve_high_pass_history() {
    let mut session = fixture();
    session.inputs[0].dsp.high_pass = Some(HighPassConfig { cutoff_hz: 120.0 });
    session.buses[0].dsp = DspConfig::default();
    let mut actual = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let mut reference = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let impulse = [0.5_f32];
    let zero = [0.0_f32];
    let mut a = [0.0];
    let mut ar = [0.0];
    let mut b = [0.0];
    let mut br = [0.0];
    for mixer in [&mut actual, &mut reference] {
        mixer
            .render(
                &[StereoInput {
                    id: session.inputs[0].id,
                    left: &impulse,
                    right: &impulse,
                }],
                &mut [StereoOutput {
                    id: session.buses[0].id,
                    left: &mut a,
                    right: &mut ar,
                }],
                1,
            )
            .unwrap();
    }
    session.buses[0].controls.set_gain_db(-6.0);
    actual
        .apply_controls(&PreparedControls::compile(&session, 1024 * 1024).unwrap())
        .unwrap();
    actual
        .render(
            &[StereoInput {
                id: session.inputs[0].id,
                left: &zero,
                right: &zero,
            }],
            &mut [StereoOutput {
                id: session.buses[0].id,
                left: &mut a,
                right: &mut ar,
            }],
            1,
        )
        .unwrap();
    reference
        .render(
            &[StereoInput {
                id: session.inputs[0].id,
                left: &zero,
                right: &zero,
            }],
            &mut [StereoOutput {
                id: session.buses[0].id,
                left: &mut b,
                right: &mut br,
            }],
            1,
        )
        .unwrap();
    assert!(
        b[0].abs() > 0.001,
        "filter history must be nonzero after the impulse"
    );
    let first_ramp_gain =
        1.0 + (10.0_f32.powf(-6.0 / 20.0) - 1.0) / actual.control_ramp_frames() as f32;
    assert!((a[0] - b[0] * first_ramp_gain).abs() < 0.000001);
}

#[test]
fn prepared_control_memory_budget_is_enforced_before_allocation() {
    let session = fixture();
    assert!(PreparedControls::compile(&session, 1).is_err());
    assert!(PreparedControls::compile(&session, 1024).is_ok());
}

#[test]
fn solo_scopes_resolve_independently_with_mute_winning() {
    let mut session = fixture();
    let second = Input::new(
        "Second",
        InputKind::TestTone {
            frequency_hz: 220.0,
            amplitude_db: -18.0,
        },
    );
    let second_id = second.id;
    session.add_input(second).unwrap();
    session
        .add_send(Send::new(second_id, session.buses[0].id))
        .unwrap();
    let second_bus = Bus::new("Second mix");
    let second_bus_id = second_bus.id;
    session.add_bus(second_bus).unwrap();
    session
        .add_send(Send::new(second_id, second_bus_id))
        .unwrap();
    session.inputs[1].controls.solo = true;
    session.sends[0].controls.solo = true;
    session.sends[0].controls.muted = true;
    let controls = PreparedControls::compile(&session, 4096).unwrap();
    assert!(!controls.inputs[0].active);
    assert!(controls.inputs[1].active);
    assert!(!controls.sends[0].active);
    assert!(!controls.sends[1].active);
    assert!(
        controls.sends[2].active,
        "send solo must not leak across mixes"
    );
}
