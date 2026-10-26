use super::*;
use relink_core::{CompiledMixer, CompressorConfig, ResourceBudget, StereoInput, StereoOutput};

fn output() -> OutputId {
    #[cfg(target_os = "macos")]
    let host = cpal::HostId::CoreAudio;
    #[cfg(target_os = "windows")]
    let host = cpal::HostId::Wasapi;
    OutputId(format!("{host}:explicit-generated-endpoint"))
}
fn studio() -> (Session, Uuid) {
    let mut session = Session::new("Unrelated studio routes", 48000);
    let input = session
        .add_input(Input::new(
            "Chosen source",
            InputKind::TestTone {
                frequency_hz: 440.0,
                amplitude_db: -6.0,
            },
        ))
        .unwrap();
    session.inputs[0].controls.set_gain_db(-6.0);
    session.inputs[0].dsp.compressor = Some(CompressorConfig {
        ratio: 1.0,
        makeup_gain_db: -3.0,
        ..Default::default()
    });
    let other = session
        .add_input(Input::new(
            "Unselected",
            InputKind::TestTone {
                frequency_hz: 991.0,
                amplitude_db: -18.0,
            },
        ))
        .unwrap();
    for name in ["Stream", "Voice", "Monitor"] {
        let bus = session.add_bus(Bus::new(name)).unwrap();
        session.add_send(Send::new(input, bus)).unwrap();
        session.add_send(Send::new(other, bus)).unwrap();
    }
    (session, input)
}

#[test]
fn private_graph_and_single_endpoint_exclude_all_studio_routes_and_other_sources() {
    let (session, source) = studio();
    let original = session.clone();
    let selection = SoundCheckSelection::prepare(&session, source, output()).unwrap();
    for graph in [
        selection.recording_graph(&session).unwrap(),
        selection.playback_graph(&session).unwrap(),
    ] {
        assert_eq!(graph.inputs.len(), 1);
        assert_eq!(graph.inputs[0].id, source);
        assert_eq!(graph.buses.len(), 1);
        assert!(!session.buses.iter().any(|bus| bus.id == graph.buses[0].id));
        assert_eq!(graph.buses[0].dsp, DspConfig::default());
        assert_eq!(graph.buses[0].controls, ChannelControls::default());
        assert_eq!(graph.sends.len(), 1);
        assert_eq!(graph.sends[0].controls, ChannelControls::default());
        let plan = selection.output_plan();
        assert_eq!(plan.routes.len(), 1);
        assert_eq!(plan.routes[0].output, output());
        assert_eq!(plan.routes[0].bus_id, graph.buses[0].id);
        assert_eq!(plan.clock_output, output());
        plan.validate(&graph).unwrap();
    }
    assert_eq!(session, original);
}

#[test]
fn dry_record_graph_ignores_input_effects_and_audition_applies_only_current_source_once() {
    let (session, source) = studio();
    let selection = SoundCheckSelection::prepare(&session, source, output()).unwrap();
    let dry = selection.recording_graph(&session).unwrap();
    assert_eq!(dry.inputs[0].controls, ChannelControls::default());
    assert_eq!(dry.inputs[0].dsp, DspConfig::default());
    let render = |graph: &Session| {
        let mut mixer = CompiledMixer::compile(graph, ResourceBudget::default()).unwrap();
        let source_left = [2.0_f32; 256];
        let source_right = [-0.5_f32; 256];
        let mut left = [0.0_f32; 256];
        let mut right = [0.0_f32; 256];
        mixer
            .render(
                &[StereoInput {
                    id: source,
                    left: &source_left,
                    right: &source_right,
                }],
                &mut [StereoOutput {
                    id: selection.bus_id(),
                    left: &mut left,
                    right: &mut right,
                }],
                256,
            )
            .unwrap();
        (left, right)
    };
    let (left, right) = render(&dry);
    assert_eq!(left, [2.0; 256], "no hidden audition master limiter");
    assert_eq!(right, [-0.5; 256]);
    let (left, right) = render(&selection.playback_graph(&session).unwrap());
    let gain = 10.0_f32.powf(-9.0 / 20.0);
    assert!(left
        .iter()
        .all(|sample| (*sample - 2.0 * gain).abs() < 1e-6));
    assert!(right
        .iter()
        .all(|sample| (*sample + 0.5 * gain).abs() < 1e-6));
}

#[test]
fn live_source_edits_preserve_private_ids_but_identity_rate_and_session_changes_fail() {
    let (mut session, source) = studio();
    let selection = SoundCheckSelection::prepare(&session, source, output()).unwrap();
    let before = selection.playback_graph(&session).unwrap();
    session.inputs[0].name = "Renamed".into();
    session.inputs[0].dsp.bypass.compressor = true;
    session.inputs[0].controls.set_gain_db(-12.0);
    let after = selection.playback_graph(&session).unwrap();
    assert_eq!(before.id, after.id);
    assert_eq!(before.sends, after.sends);
    assert_eq!(before.buses, after.buses);
    assert_eq!(after.inputs[0], session.inputs[0]);
    crate::prepare_live_update(&before, &after).unwrap();
    for problem in 0..4 {
        let mut changed = session.clone();
        match problem {
            0 => changed.id = Uuid::new_v4(),
            1 => changed.sample_rate = 96000,
            2 => {
                changed.inputs[0].kind = InputKind::TestTone {
                    frequency_hz: 220.0,
                    amplitude_db: -6.0,
                }
            }
            _ => {
                changed.remove_input(source).unwrap();
            }
        }
        assert!(selection.recording_graph(&changed).is_err());
        assert!(selection.playback_graph(&changed).is_err());
    }
}

#[test]
fn absent_source_invalid_source_and_implicit_output_are_rejected_without_discovery() {
    let (mut session, source) = studio();
    assert!(SoundCheckSelection::prepare(&session, Uuid::new_v4(), output()).is_err());
    for id in ["", "default", "NotAHost:device"] {
        assert!(SoundCheckSelection::prepare(&session, source, OutputId(id.into())).is_err());
    }
    session.inputs[0].kind = InputKind::Hardware {
        device_id: "not-a-native-ID".into(),
    };
    assert!(SoundCheckSelection::prepare(&session, source, output()).is_err());
    session.inputs[0].kind = InputKind::Application {
        application_id: "transient-process-name".into(),
    };
    assert!(SoundCheckSelection::prepare(&session, source, output()).is_err());
}
