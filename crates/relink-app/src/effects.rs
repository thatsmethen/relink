use eframe::egui;
use relink_core::{CompressorConfig, DspConfig, GateConfig, HighPassConfig, LimiterConfig};

/// Shared source/master editor. The caller owns the device-operation guard and
/// submits one validated, undoable session edit after comparing the result.
pub fn editor(ui: &mut egui::Ui, dsp: &mut DspConfig, sample_rate: u32) {
    ui.scope(|ui| {
        ui.spacing_mut().slider_width = 110.0;
        stage_switch(
            ui,
            "High-pass filter",
            &mut dsp.high_pass,
            &mut dsp.bypass.high_pass,
            || HighPassConfig { cutoff_hz: 80.0 },
        );
        ui.add_enabled_ui(!dsp.bypass.high_pass, |ui| {
            if let Some(effect) = &mut dsp.high_pass {
                parameter(
                    ui,
                    "High-pass cutoff",
                    &mut effect.cutoff_hz,
                    0.1..=sample_rate as f32 * 0.49,
                    " Hz",
                    true,
                );
            }
        });

        stage_switch(
            ui,
            "Noise gate",
            &mut dsp.gate,
            &mut dsp.bypass.gate,
            GateConfig::default,
        );
        ui.add_enabled_ui(!dsp.bypass.gate, |ui| {
            if let Some(effect) = &mut dsp.gate {
                egui::CollapsingHeader::new("Gate settings")
                    .default_open(true)
                    .show(ui, |ui| {
                        parameter(
                            ui,
                            "Gate threshold",
                            &mut effect.threshold_db,
                            -120.0..=0.0,
                            " dB",
                            false,
                        );
                        parameter(
                            ui,
                            "Gate attack",
                            &mut effect.attack_ms,
                            0.01..=5000.0,
                            " ms",
                            true,
                        );
                        parameter(
                            ui,
                            "Gate hold",
                            &mut effect.hold_ms,
                            0.0..=5000.0,
                            " ms",
                            true,
                        );
                        parameter(
                            ui,
                            "Gate release",
                            &mut effect.release_ms,
                            0.01..=10_000.0,
                            " ms",
                            true,
                        );
                    });
            }
        });

        stage_switch(
            ui,
            "Compressor",
            &mut dsp.compressor,
            &mut dsp.bypass.compressor,
            CompressorConfig::default,
        );
        ui.add_enabled_ui(!dsp.bypass.compressor, |ui| {
            if let Some(effect) = &mut dsp.compressor {
                egui::CollapsingHeader::new("Compressor settings")
                    .default_open(true)
                    .show(ui, |ui| {
                        parameter(
                            ui,
                            "Compressor threshold",
                            &mut effect.threshold_db,
                            -120.0..=0.0,
                            " dB",
                            false,
                        );
                        parameter(
                            ui,
                            "Compressor ratio",
                            &mut effect.ratio,
                            1.0..=100.0,
                            ":1",
                            true,
                        );
                        parameter(
                            ui,
                            "Compressor attack",
                            &mut effect.attack_ms,
                            0.01..=5000.0,
                            " ms",
                            true,
                        );
                        parameter(
                            ui,
                            "Compressor release",
                            &mut effect.release_ms,
                            0.01..=10_000.0,
                            " ms",
                            true,
                        );
                        parameter(
                            ui,
                            "Compressor makeup",
                            &mut effect.makeup_gain_db,
                            -24.0..=24.0,
                            " dB",
                            false,
                        );
                    });
            }
        });

        stage_switch(
            ui,
            "Peak limiter",
            &mut dsp.limiter,
            &mut dsp.bypass.limiter,
            LimiterConfig::default,
        );
        ui.add_enabled_ui(!dsp.bypass.limiter, |ui| {
            if let Some(effect) = &mut dsp.limiter {
                egui::CollapsingHeader::new("Limiter settings")
                    .default_open(true)
                    .show(ui, |ui| {
                        parameter(
                            ui,
                            "Limiter ceiling",
                            &mut effect.ceiling_db,
                            -60.0..=0.0,
                            " dBFS",
                            false,
                        );
                        parameter(
                            ui,
                            "Limiter release",
                            &mut effect.release_ms,
                            0.01..=10_000.0,
                            " ms",
                            true,
                        );
                        ui.small("Controls sample peaks. It does not measure inter-sample peaks.");
                    });
            }
        });
    });
}

fn stage_switch<T>(
    ui: &mut egui::Ui,
    label: &str,
    config: &mut Option<T>,
    bypass: &mut bool,
    default: impl FnOnce() -> T,
) -> egui::Response {
    let mut enabled = config.is_some() && !*bypass;
    let response = ui
        .checkbox(&mut enabled, label)
        .on_hover_text("Switch off to bypass. Your settings are kept.");
    if response.changed() {
        set_enabled(config, bypass, enabled, default);
    }
    response
}

fn set_enabled<T>(
    config: &mut Option<T>,
    bypass: &mut bool,
    enabled: bool,
    default: impl FnOnce() -> T,
) {
    if enabled {
        config.get_or_insert_with(default);
    }
    *bypass = !enabled;
}

fn parameter(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    unit: &str,
    logarithmic: bool,
) {
    let label = ui.label(label);
    ui.add(
        egui::Slider::new(value, range)
            .logarithmic(logarithmic)
            .clamping(egui::SliderClamping::Edits)
            .suffix(unit),
    )
    .labelled_by(label.id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(
        ctx: &egui::Context,
        config: &mut DspConfig,
        events: Vec<egui::Event>,
    ) -> [egui::Rect; 4] {
        let mut bounds = [egui::Rect::NOTHING; 4];
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    bounds[0] = stage_switch(
                        ui,
                        "High-pass filter",
                        &mut config.high_pass,
                        &mut config.bypass.high_pass,
                        || HighPassConfig { cutoff_hz: 80.0 },
                    )
                    .rect;
                    bounds[1] = stage_switch(
                        ui,
                        "Noise gate",
                        &mut config.gate,
                        &mut config.bypass.gate,
                        GateConfig::default,
                    )
                    .rect;
                    bounds[2] = stage_switch(
                        ui,
                        "Compressor",
                        &mut config.compressor,
                        &mut config.bypass.compressor,
                        CompressorConfig::default,
                    )
                    .rect;
                    bounds[3] = stage_switch(
                        ui,
                        "Peak limiter",
                        &mut config.limiter,
                        &mut config.bypass.limiter,
                        LimiterConfig::default,
                    )
                    .rect;
                });
            },
        );
        bounds
    }

    fn click(ctx: &egui::Context, config: &mut DspConfig, position: egui::Pos2) {
        for pressed in [true, false] {
            draw(
                ctx,
                config,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }

    #[test]
    fn effect_switches_preserve_tuned_settings_through_bypass_and_reopen() {
        let context = egui::Context::default();
        let initial = DspConfig {
            high_pass: Some(HighPassConfig { cutoff_hz: 321.0 }),
            gate: Some(GateConfig {
                threshold_db: -42.0,
                hold_ms: 72.0,
                ..Default::default()
            }),
            compressor: Some(CompressorConfig {
                threshold_db: -27.0,
                ratio: 7.0,
                makeup_gain_db: 3.0,
                ..Default::default()
            }),
            limiter: Some(LimiterConfig {
                ceiling_db: -5.0,
                release_ms: 300.0,
            }),
            ..Default::default()
        };
        let mut config = initial.clone();
        let bounds = draw(&context, &mut config, vec![]);
        for rect in bounds {
            click(&context, &mut config, rect.center());
        }
        let mut expected = initial.clone();
        expected.bypass = relink_core::DspBypass {
            high_pass: true,
            gate: true,
            compressor: true,
            limiter: true,
        };
        assert_eq!(
            config, expected,
            "the actual checkbox gesture must retain every parameter"
        );
        config = serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        let reopened = egui::Context::default();
        let bounds = draw(&reopened, &mut config, vec![]);
        for rect in bounds {
            click(&reopened, &mut config, rect.center());
        }
        assert_eq!(config, initial);
    }

    #[test]
    fn enabling_an_unconfigured_stage_creates_defaults_only_on_explicit_click() {
        let context = egui::Context::default();
        let mut config = DspConfig::default();
        let bounds = draw(&context, &mut config, vec![]);
        assert_eq!(config, DspConfig::default());
        click(&context, &mut config, bounds[2].center());
        assert_eq!(config.compressor, Some(CompressorConfig::default()));
        assert!(!config.bypass.compressor);
        assert!(config.high_pass.is_none() && config.gate.is_none() && config.limiter.is_none());
    }
}
