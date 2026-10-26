use super::*;
use crate::{CompressorConfig, GateConfig, HighPassConfig, LimiterConfig};

fn all_stages() -> DspConfig {
    DspConfig {
        high_pass: Some(HighPassConfig { cutoff_hz: 90.0 }),
        gate: Some(GateConfig::default()),
        compressor: Some(CompressorConfig::default()),
        limiter: Some(LimiterConfig::default()),
        bypass: Default::default(),
    }
}
fn close(actual: f32, expected: f32, tolerance: f32) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} != {expected}"
    );
}

#[test]
fn wire_rejects_nonfinite_unstable_flags_hold_and_rate() {
    let valid = PreparedDsp::compile(&all_stages(), 48000);
    assert_eq!(PreparedDsp::from_words(valid.to_words()), Ok(valid));
    for index in [2, 3, 4, 5, 7, 8, 9, 10, 11, 12, 13] {
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0] {
            let mut words = valid.to_words();
            words[index] = invalid.to_bits();
            assert_eq!(
                PreparedDsp::from_words(words),
                Err(crate::ControlError::InvalidDsp)
            );
        }
    }
    for (index, word) in [
        (0, 7999),
        (0, 384001),
        (1, 16),
        (6, 240001),
        (2, 1.001_f32.to_bits()),
        (3, 0),
        (7, 0),
        (8, 1.0_f32.to_bits()),
        (11, 17.0_f32.to_bits()),
        (12, 0),
        (12, 1.1_f32.to_bits()),
    ] {
        let mut words = valid.to_words();
        words[index] = word;
        assert!(PreparedDsp::from_words(words).is_err());
    }
}

#[test]
fn all_supported_integer_rates_accept_compiled_extreme_coefficients() {
    for rate in 8000..=384000 {
        for maximum in [false, true] {
            let config = DspConfig {
                high_pass: Some(HighPassConfig {
                    cutoff_hz: if maximum {
                        rate as f32 * 0.49999
                    } else {
                        f32::MIN_POSITIVE
                    },
                }),
                gate: Some(GateConfig {
                    threshold_db: if maximum { 0.0 } else { -120.0 },
                    attack_ms: if maximum { 5000.0 } else { 0.01 },
                    hold_ms: if maximum { 5000.0 } else { 0.0 },
                    release_ms: if maximum { 10000.0 } else { 0.01 },
                }),
                compressor: Some(CompressorConfig {
                    threshold_db: if maximum { 0.0 } else { -120.0 },
                    ratio: if maximum { 100.0 } else { 1.0 },
                    attack_ms: if maximum { 5000.0 } else { 0.01 },
                    release_ms: if maximum { 10000.0 } else { 0.01 },
                    makeup_gain_db: if maximum { 24.0 } else { -24.0 },
                }),
                limiter: Some(LimiterConfig {
                    ceiling_db: if maximum { 0.0 } else { -60.0 },
                    release_ms: if maximum { 10000.0 } else { 0.01 },
                }),
                bypass: Default::default(),
            };
            let prepared = PreparedDsp::compile(&config, rate);
            assert_eq!(
                PreparedDsp::from_words(prepared.to_words()),
                Ok(prepared),
                "rate {rate}, maximum {maximum}"
            );
        }
    }
}

#[test]
fn bypass_reaches_exact_dry_and_wet_endpoints_over_five_milliseconds() {
    for rate in [8000, 44100, 48000, 96000, 384000] {
        let dry = DspConfig::default();
        let gated = DspConfig {
            gate: Some(GateConfig::default()),
            ..Default::default()
        };
        let mut processor = Processor::new(&dry, rate);
        let input = [0.0000001, -0.00000005]; // always below the gate threshold
        assert_eq!(processor.process(input), input);
        let frames = rate.div_ceil(200);
        processor.retarget(PreparedDsp::compile(&gated, rate), frames);
        for frame in 1..=frames {
            let output = processor.process(input);
            let remaining = 1.0 - frame as f32 / frames as f32;
            close(output[0], input[0] * remaining, 2.0e-14);
            close(output[1], input[1] * remaining, 1.0e-14);
        }
        assert_eq!(processor.process(input), [0.0; 2]);
        processor.retarget(PreparedDsp::compile(&dry, rate), frames);
        for frame in 1..=frames {
            let output = processor.process(input);
            close(output[0], input[0] * frame as f32 / frames as f32, 2.0e-14);
        }
        assert_eq!(processor.process(input), input);
    }
}

#[test]
fn repeated_identical_updates_preserve_transition_and_envelope_continuation() {
    let config = all_stages();
    let mut reference = Processor::new(&config, 48000);
    let mut repeated = Processor::new(&config, 48000);
    let mut changed = config.clone();
    changed.high_pass.as_mut().unwrap().cutoff_hz = 700.0;
    changed.gate.as_mut().unwrap().threshold_db = -26.0;
    changed.compressor.as_mut().unwrap().makeup_gain_db = 8.0;
    changed.limiter.as_mut().unwrap().ceiling_db = -4.0;
    let original = PreparedDsp::compile(&config, 48000);
    let target = PreparedDsp::compile(&changed, 48000);
    for index in 0..4000 {
        let input = [
            (index as f32 * 0.13).sin() * 0.8,
            (index as f32 * 0.19).cos() * 0.4,
        ];
        let bank = if index >= 1500 { target } else { original };
        if index == 1500 {
            reference.retarget(target, 240);
        }
        repeated.retarget(bank, 240);
        assert_eq!(repeated.process(input), reference.process(input));
    }
}

#[test]
fn gate_hold_and_compressor_gain_survive_parameter_updates() {
    let mut gate_config = DspConfig {
        gate: Some(GateConfig {
            attack_ms: 0.01,
            hold_ms: 100.0,
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut gate = Processor::new(&gate_config, 48000);
    for _ in 0..100 {
        gate.process([1.0; 2]);
    }
    let held = gate.gate.processor.remaining_hold;
    let gain = gate.gate.processor.gain;
    gate_config.gate.as_mut().unwrap().release_ms = 900.0;
    gate.retarget(PreparedDsp::compile(&gate_config, 48000), 240);
    assert_eq!(gate.gate.processor.remaining_hold, held);
    assert_eq!(gate.gate.processor.gain, gain);
    for _ in 0..300 {
        close(gate.process([0.001; 2])[0], 0.001, 1.0e-9);
    }

    let mut compressor_config = DspConfig {
        compressor: Some(CompressorConfig {
            threshold_db: -24.0,
            ratio: 100.0,
            attack_ms: 0.01,
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut compressor = Processor::new(&compressor_config, 48000);
    for _ in 0..100 {
        compressor.process([1.0; 2]);
    }
    let held_gain = compressor.compressor.processor.gain;
    assert!(held_gain < 0.07);
    compressor_config
        .compressor
        .as_mut()
        .unwrap()
        .makeup_gain_db = 6.0;
    compressor.retarget(PreparedDsp::compile(&compressor_config, 48000), 240);
    assert_eq!(compressor.compressor.processor.gain, held_gain);
    assert!(compressor.process([1.0; 2])[0] < 0.071);
}

#[test]
fn enabled_compressor_is_applied_once_and_limiter_transition_claims_are_explicit() {
    let compressor = DspConfig {
        compressor: Some(CompressorConfig {
            ratio: 1.0,
            makeup_gain_db: -6.0,
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut processor = Processor::new(&compressor, 48000);
    let gain = db_to_gain(-6.0);
    for _ in 0..1000 {
        assert_eq!(processor.process([0.4, -0.2]), [0.4 * gain, -0.2 * gain]);
    }

    let mut processor = Processor::new(&DspConfig::default(), 48000);
    let mut limited = DspConfig {
        limiter: Some(LimiterConfig {
            ceiling_db: -6.0,
            ..Default::default()
        }),
        ..Default::default()
    };
    processor.retarget(PreparedDsp::compile(&limited, 48000), 240);
    assert!(processor.process([2.0; 2])[0] > gain); // dry path remains during fade
    for _ in 1..240 {
        processor.process([2.0; 2]);
    }
    assert_eq!(processor.process([2.0, -2.0]), [gain, -gain]);
    limited.limiter.as_mut().unwrap().ceiling_db = -12.0;
    let lower = db_to_gain(-12.0);
    processor.retarget(PreparedDsp::compile(&limited, 48000), 240);
    let first = processor.process([2.0; 2])[0];
    assert!(first <= gain && first > lower); // current interpolated ceiling
    for _ in 1..240 {
        processor.process([2.0; 2]);
    }
    assert_eq!(processor.process([2.0, -2.0]), [lower, -lower]);
}

#[test]
fn rapid_retargets_remain_finite_with_extreme_input_and_parameter_ranges() {
    for rate in [8000, 11025, 44100, 48000, 192000, 384000] {
        let mut processor = Processor::new(&DspConfig::default(), rate);
        let mut active = all_stages();
        active.high_pass.as_mut().unwrap().cutoff_hz = rate as f32 * 0.49;
        active.compressor.as_mut().unwrap().makeup_gain_db = 24.0;
        let targets = [
            PreparedDsp::compile(&active, rate),
            PreparedDsp::compile(&all_stages(), rate),
            PreparedDsp::compile(&DspConfig::default(), rate),
        ];
        for index in 0..20000 {
            if index % 3 == 0 {
                processor.retarget(targets[(index / 3) % targets.len()], rate.div_ceil(200));
            }
            let sample = [f32::NAN, f32::INFINITY, 1000000.0, -1000000.0, 0.0][index % 5];
            let output = processor.process([sample, -sample]);
            assert!(output
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 1000000.0));
            assert!(processor
                .high_pass
                .processor
                .previous_output
                .iter()
                .all(|value| value.is_finite()));
        }
    }
}

#[test]
fn settled_high_pass_changes_generated_spectrum_without_state_reset() {
    let rate = 48000;
    let mut config = DspConfig {
        high_pass: Some(HighPassConfig { cutoff_hz: 500.0 }),
        ..Default::default()
    };
    let mut processor = Processor::new(&config, rate);
    let measure = |processor: &mut Processor| {
        let mut low = [0.0_f64; 2];
        let mut high = [0.0_f64; 2];
        for frame in 0..96000 {
            let phase = std::f64::consts::TAU * frame as f64 / f64::from(rate);
            let input = ((phase * 10.0).sin() + (phase * 5000.0).sin()) as f32 * 0.25;
            let output = f64::from(processor.process([input; 2])[0]);
            if frame >= 48000 {
                low[0] += output * (phase * 10.0).sin();
                low[1] += output * (phase * 10.0).cos();
                high[0] += output * (phase * 5000.0).sin();
                high[1] += output * (phase * 5000.0).cos();
            }
        }
        [
            (low[0].hypot(low[1]) / 24000.0),
            (high[0].hypot(high[1]) / 24000.0),
        ]
    };
    let before = measure(&mut processor);
    config.high_pass.as_mut().unwrap().cutoff_hz = 5000.0;
    processor.retarget(PreparedDsp::compile(&config, rate), 240);
    let after = measure(&mut processor);
    assert!(before[0] < 0.006 && before[1] > 0.22);
    assert!(after[0] < 0.0005 && after[1] < before[1] * 0.7);
}

#[test]
fn cancelled_enable_before_first_sample_never_uses_unused_compressor_defaults() {
    let rate = 48000;
    let dry = PreparedDsp::compile(&DspConfig::default(), rate);
    let mut config = DspConfig {
        compressor: Some(CompressorConfig {
            ratio: 1.0,
            makeup_gain_db: -6.0,
            ..Default::default()
        }),
        ..Default::default()
    };
    let enabled = PreparedDsp::compile(&config, rate);
    let mut actual = Processor::new(&DspConfig::default(), rate);
    let mut reference = Processor::new(&DspConfig::default(), rate);
    actual.retarget(enabled, 240);
    actual.retarget(dry, 240);
    config.compressor.as_mut().unwrap().makeup_gain_db = -12.0;
    let final_target = PreparedDsp::compile(&config, rate);
    actual.retarget(final_target, 240);
    reference.retarget(final_target, 240);
    for _ in 0..1000 {
        assert_eq!(actual.process([0.8, -0.4]), reference.process([0.8, -0.4]));
    }
    assert_eq!(actual.compressor.processor.gain, 1.0);
}

#[test]
fn fully_bypassed_stages_freeze_history_and_resume_without_reset() {
    let config = all_stages();
    let enabled = PreparedDsp::compile(&config, 48000);
    let mut processor = Processor::new(&config, 48000);
    for n in 0..1000 {
        processor.process([(n as f32 * 0.1).sin() * 0.9; 2]);
    }
    processor.retarget(PreparedDsp::compile(&DspConfig::default(), 48000), 240);
    for n in 0..240 {
        processor.process([(n as f32 * 0.1).sin() * 0.9; 2]);
    }
    let input_history = processor.high_pass.processor.previous_input;
    let output_history = processor.high_pass.processor.previous_output;
    let gate_gain = processor.gate.processor.gain;
    let hold = processor.gate.processor.remaining_hold;
    let compressor_gain = processor.compressor.processor.gain;
    let limiter_gain = processor.limiter.processor.gain;
    for _ in 0..500 {
        assert_eq!(processor.process([0.6, -0.7]), [0.6, -0.7]);
    }
    processor.retarget(enabled, 240);
    assert_eq!(processor.high_pass.processor.previous_input, input_history);
    assert_eq!(
        processor.high_pass.processor.previous_output,
        output_history
    );
    assert_eq!(processor.gate.processor.gain, gate_gain);
    assert_eq!(processor.gate.processor.remaining_hold, hold);
    assert_eq!(processor.compressor.processor.gain, compressor_gain);
    assert_eq!(processor.limiter.processor.gain, limiter_gain);
}

#[test]
fn malformed_late_dsp_rejection_preserves_entire_running_bank() {
    use crate::{
        Bus, CompiledMixer, ControlError, Input, InputKind, PreparedControls, ResourceBudget, Send,
        Session, StereoInput, StereoOutput,
    };
    let mut session = Session::new("Generated malformed coefficient control", 48000);
    let mut input = Input::new(
        "Source",
        InputKind::Hardware {
            device_id: "in-memory-only".into(),
        },
    );
    input.dsp = all_stages();
    let input_id = session.add_input(input).unwrap();
    let bus_id = session.add_bus(Bus::new("Bus")).unwrap();
    session.add_send(Send::new(input_id, bus_id)).unwrap();
    let mut actual = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let mut reference = CompiledMixer::compile(&session, ResourceBudget::default()).unwrap();
    let source: Vec<_> = (0..511).map(|n| (n as f32 * 0.17).sin() * 0.8).collect();
    let render = |mixer: &mut CompiledMixer| {
        let mut left = vec![0.0; source.len()];
        let mut right = vec![0.0; source.len()];
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
                source.len(),
            )
            .unwrap();
        left
    };
    assert_eq!(render(&mut actual), render(&mut reference));
    let mut bank = PreparedControls::compile(&session, 1024 * 1024).unwrap();
    bank.inputs[0].gains = [0.0; 2];
    bank.input_dsp[0] = PreparedDsp::compile(&DspConfig::default(), 48000);
    // Deliberate private test corruption: public wire construction rejects this.
    bank.bus_dsp[0].words[13] = f32::NAN.to_bits();
    assert_eq!(actual.apply_controls(&bank), Err(ControlError::InvalidDsp));
    assert_eq!(render(&mut actual), render(&mut reference));
}

#[test]
fn editing_fully_bypassed_parameters_uses_latest_target_without_resetting_envelope() {
    let mut config = DspConfig {
        compressor: Some(CompressorConfig {
            threshold_db: -24.0,
            ratio: 100.0,
            attack_ms: 0.01,
            makeup_gain_db: 0.0,
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut processor = Processor::new(&config, 48000);
    for _ in 0..100 {
        processor.process([1.0; 2]);
    }
    config.bypass.compressor = true;
    processor.retarget(PreparedDsp::compile(&config, 48000), 240);
    for _ in 0..240 {
        processor.process([1.0; 2]);
    }
    let envelope = processor.compressor.processor.gain;
    assert!(envelope < 0.07);
    // Several edits while fully dry must keep the original envelope and use
    // only the final prepared target when processing becomes audible again.
    for makeup in [-12.0, 6.0, 12.0] {
        config.compressor.as_mut().unwrap().makeup_gain_db = makeup;
        processor.retarget(PreparedDsp::compile(&config, 48000), 240);
        for _ in 0..300 {
            assert_eq!(processor.process([1.0; 2]), [1.0; 2]);
        }
        assert_eq!(processor.compressor.processor.gain, envelope);
    }
    config.bypass.compressor = false;
    processor.retarget(PreparedDsp::compile(&config, 48000), 240);
    assert_eq!(processor.compressor.processor.gain, envelope);
    let first = processor.process([1.0; 2]);
    assert_eq!(processor.compressor.processor.makeup_gain, db_to_gain(12.0));
    close(
        first[0],
        1.0 + (envelope * db_to_gain(12.0) - 1.0) / 240.0,
        1.0e-7,
    );
    for _ in 1..240 {
        processor.process([1.0; 2]);
    }
    close(
        processor.process([1.0; 2])[0],
        envelope * db_to_gain(12.0),
        1.0e-7,
    );
}
