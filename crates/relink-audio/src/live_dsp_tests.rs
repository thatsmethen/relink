//! Generated buffers only: these tests never enumerate or open audio devices.
use super::*;
use crate::tests::{render, COUNTING, OPERATIONS};
use relink_core::{Bus, CompressorConfig, DspConfig, Input, Send};
use std::cell::Cell;

fn fixture() -> (Session, Uuid) {
    let mut session = Session::new("Generated live effects", 48000);
    let input = session
        .add_input(Input::new(
            "Generated 440 Hz",
            InputKind::TestTone {
                frequency_hz: 440.0,
                amplitude_db: -12.0,
            },
        ))
        .unwrap();
    let mut bus = Bus::new("Generated sink");
    bus.dsp = DspConfig::default();
    let bus = session.add_bus(bus).unwrap();
    session.add_send(Send::new(input, bus)).unwrap();
    (session, bus)
}
fn gain_effect(db: f32) -> DspConfig {
    DspConfig {
        compressor: Some(CompressorConfig {
            ratio: 1.0,
            makeup_gain_db: db,
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn tone(frame: usize) -> f32 {
    (frame as f64 * std::f64::consts::TAU * 440.0 / 48000.0).sin() as f32
        * 10.0_f32.powf(-12.0 / 20.0)
}

#[test]
fn callback_applies_live_input_and_mix_effects_once_without_reset_or_allocation() {
    let (initial, bus) = fixture();
    let mut callback = render(&initial, bus, 2);
    callback.write(&mut [0.0_f32; 74]);
    let mut changed = initial.clone();
    changed.inputs[0].dsp = gain_effect(-6.0);
    changed.buses[0].dsp = gain_effect(-3.0);
    let version = callback
        .bank
        .publish(&prepare_live_update(&initial, &changed).unwrap())
        .unwrap();
    let mut output = [0.0_f32; 1200];
    OPERATIONS.with(|n| n.set(0));
    COUNTING.with(|on| on.set(true));
    callback.write(&mut output);
    COUNTING.with(|on| on.set(false));
    assert_eq!(OPERATIONS.with(Cell::get), 0);
    let input_gain = 10.0_f32.powf(-6.0 / 20.0);
    let bus_gain = 10.0_f32.powf(-3.0 / 20.0);
    for (frame, stereo) in output.chunks_exact(2).enumerate() {
        let wet = ((frame + 1) as f32 / 240.0).min(1.0);
        let expected =
            tone(37 + frame) * (1.0 + (input_gain - 1.0) * wet) * (1.0 + (bus_gain - 1.0) * wet);
        assert!((stereo[0] - expected).abs() < 0.000002, "frame {frame}");
        assert_eq!(stereo[0], stereo[1]);
    }
    assert_eq!(
        callback
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        version
    );
    assert_eq!(callback.telemetry.frames.load(Ordering::Relaxed), 637);

    let mut bypassed = changed.clone();
    bypassed.inputs[0].dsp.bypass.compressor = true;
    bypassed.buses[0].dsp.bypass.compressor = true;
    let bypass = callback
        .bank
        .publish(&prepare_live_update(&changed, &bypassed).unwrap())
        .unwrap();
    callback.write(&mut output);
    for (frame, stereo) in output.chunks_exact(2).enumerate().skip(240) {
        assert!((stereo[0] - tone(637 + frame)).abs() < 0.000002);
    }
    assert_eq!(
        callback
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        bypass
    );
    callback
        .bank
        .publish(&prepare_live_update(&bypassed, &changed).unwrap())
        .unwrap();
    callback.write(&mut output);
    for (frame, stereo) in output.chunks_exact(2).enumerate().skip(240) {
        let expected = tone(1237 + frame) * input_gain * bus_gain;
        assert!((stereo[0] - expected).abs() < 0.000002);
    }
    assert_eq!(callback.telemetry.fault.load(Ordering::Acquire), 0);
}

#[test]
fn invalid_effect_shape_or_rate_cannot_publish_a_partial_control_bank() {
    let (session, _) = fixture();
    let initial = PreparedControls::compile(&session, GRAPH_BYTES).unwrap();
    let bank = ControlBank::new(&initial);
    let version = bank.publish(&initial).unwrap();
    let mut other_rate = session.clone();
    other_rate.sample_rate = 96000;
    let other = PreparedControls::compile(&other_rate, GRAPH_BYTES).unwrap();
    for problem in 0..5 {
        let mut invalid = initial.clone();
        invalid.inputs[0].gains = [0.0; 2];
        match problem {
            0 => invalid.sample_rate = 96000,
            1 => invalid.input_dsp.clear(),
            2 => invalid.bus_dsp.push(initial.bus_dsp[0]),
            3 => invalid.input_dsp[0] = other.input_dsp[0],
            _ => invalid.bus_dsp[0] = other.bus_dsp[0],
        }
        assert!(bank.publish(&invalid).is_err());
        assert_eq!(bank.version(), version);
        let mut scratch = initial.clone();
        assert_eq!(bank.copy_if_new(&mut scratch, 0), Some(version));
        assert_eq!(scratch, initial);
    }
}

#[test]
fn stalled_effect_publication_renders_previous_complete_state_and_does_not_ack() {
    let (session, bus) = fixture();
    let mut callback = render(&session, bus, 2);
    callback.write(&mut [0.0_f32; 58]);
    let mut changed = session.clone();
    changed.inputs[0].dsp = gain_effect(-18.0);
    changed.buses[0].dsp = gain_effect(-12.0);
    changed.buses[0].controls.muted = true;
    callback
        .bank
        .begin_incomplete_test_write(&prepare_live_update(&session, &changed).unwrap());
    let mut output = [0.0_f32; 1024];
    callback.write(&mut output);
    for (frame, stereo) in output.chunks_exact(2).enumerate() {
        assert!((stereo[0] - tone(29 + frame)).abs() < 0.000002);
    }
    assert_eq!(callback.applied_version, 0);
    assert_eq!(
        callback
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        0
    );
}

#[test]
fn cancelled_callback_cannot_acknowledge_or_emit_pending_effect_changes() {
    let (session, bus) = fixture();
    let mut callback = render(&session, bus, 2);
    let mut changed = session.clone();
    changed.inputs[0].dsp = gain_effect(12.0);
    callback
        .bank
        .publish(&prepare_live_update(&session, &changed).unwrap())
        .unwrap();
    callback.wanted.store(false, Ordering::Release);
    let mut output = [1.0_f32; 512];
    callback.write(&mut output);
    assert!(output.iter().all(|&sample| sample == 0.0));
    assert_eq!(
        callback
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        0
    );
    assert_eq!(callback.telemetry.frames.load(Ordering::Relaxed), 0);
}
