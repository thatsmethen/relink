use eframe::egui;
use relink_core::{CompressorConfig, DspConfig, GateConfig, HighPassConfig, LimiterConfig};

/// Shared source/master editor. The caller owns the stopped-audio guard and
/// submits one validated, undoable session edit after comparing the result.
pub fn editor(ui: &mut egui::Ui, dsp: &mut DspConfig, sample_rate: u32) {
    ui.scope(|ui| {
        ui.spacing_mut().slider_width = 110.0;
        let mut enabled = dsp.high_pass.is_some();
        if ui.checkbox(&mut enabled, "High-pass filter").changed() {
            dsp.high_pass = enabled.then_some(HighPassConfig { cutoff_hz: 80.0 });
        }
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

        let mut enabled = dsp.gate.is_some();
        if ui.checkbox(&mut enabled, "Noise gate").changed() {
            dsp.gate = enabled.then(GateConfig::default);
        }
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

        let mut enabled = dsp.compressor.is_some();
        if ui.checkbox(&mut enabled, "Compressor").changed() {
            dsp.compressor = enabled.then(CompressorConfig::default);
        }
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

        let mut enabled = dsp.limiter.is_some();
        if ui.checkbox(&mut enabled, "Peak limiter").changed() {
            dsp.limiter = enabled.then(LimiterConfig::default);
        }
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
