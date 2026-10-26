use relink_core::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
struct CountingAllocator;
thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static OPERATIONS: Cell<usize> = const { Cell::new(0) };
}
fn count() {
    let _ = COUNTING.try_with(|enabled| {
        if enabled.get() {
            let _ = OPERATIONS.try_with(|n| n.set(n.get() + 1));
        }
    });
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count();
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn fixture(rate: u32) -> Session {
    let mut session = Session::new("Generated live DSP controls", rate);
    let input = session
        .add_input(Input::new(
            "Generated input",
            InputKind::Hardware {
                device_id: "in-memory-only".into(),
            },
        ))
        .unwrap();
    for name in ["One", "Two"] {
        let mut bus = Bus::new(name);
        bus.dsp = DspConfig::default();
        let bus = session.add_bus(bus).unwrap();
        session.add_send(Send::new(input, bus)).unwrap();
    }
    session
}
fn effects() -> DspConfig {
    DspConfig {
        high_pass: Some(HighPassConfig { cutoff_hz: 300.0 }),
        gate: Some(GateConfig {
            threshold_db: -30.0,
            ..Default::default()
        }),
        compressor: Some(CompressorConfig {
            ratio: 6.0,
            attack_ms: 1.0,
            makeup_gain_db: 4.0,
            ..Default::default()
        }),
        limiter: Some(LimiterConfig {
            ceiling_db: -4.0,
            ..Default::default()
        }),
        bypass: Default::default(),
    }
}
fn bank(session: &Session) -> PreparedControls {
    PreparedControls::compile(session, 1024 * 1024).unwrap()
}
fn mixer(session: &Session) -> CompiledMixer {
    CompiledMixer::compile(session, ResourceBudget::default()).unwrap()
}
fn render(mixer: &mut CompiledMixer, session: &Session, source: &[f32]) -> Vec<f32> {
    let mut output = vec![0.0; source.len()];
    let mut right = vec![0.0; source.len()];
    mixer
        .render(
            &[StereoInput {
                id: session.inputs[0].id,
                left: source,
                right: source,
            }],
            &mut [StereoOutput {
                id: session.buses[0].id,
                left: &mut output,
                right: &mut right,
            }],
            source.len(),
        )
        .unwrap();
    output
}

#[test]
fn simultaneous_input_and_bus_dsp_updates_are_bit_exact_across_callback_partitions() {
    for rate in [8000, 44100, 48000, 384000] {
        let mut session = fixture(rate);
        let initial = session.clone();
        let mut changes = vec![(0, bank(&session))];
        session.inputs[0].dsp = effects();
        session.buses[0].dsp = effects();
        changes.push((37, bank(&session)));
        session.inputs[0].dsp.high_pass.as_mut().unwrap().cutoff_hz = 1200.0;
        session.inputs[0].dsp.gate = None;
        session.buses[0]
            .dsp
            .compressor
            .as_mut()
            .unwrap()
            .makeup_gain_db = -3.0;
        changes.push((91, bank(&session)));
        session.inputs[0].dsp = DspConfig::default();
        session.buses[0].dsp = DspConfig::default();
        changes.push((200, bank(&session)));
        session.inputs[0].dsp = effects();
        session.buses[0].dsp.limiter = Some(LimiterConfig::default());
        changes.push((311, bank(&session)));
        let source: Vec<_> = (0..2048)
            .map(|n| (n as f32 * 0.071).sin() * 0.8 + (n as f32 * 0.17).cos() * 0.1)
            .collect();
        let run = |parts: &[usize]| {
            let mut mixer = mixer(&initial);
            let mut result = Vec::new();
            let mut position = 0;
            let mut part = 0;
            let mut current = 0;
            while position < source.len() {
                if current + 1 < changes.len() && position == changes[current + 1].0 {
                    current += 1;
                }
                mixer.apply_controls(&changes[current].1).unwrap(); // repeated complete banks
                let next_change = changes
                    .get(current + 1)
                    .map_or(source.len(), |(frame, _)| *frame);
                let length = parts[part % parts.len()].min(next_change - position);
                result.extend(render(
                    &mut mixer,
                    &initial,
                    &source[position..position + length],
                ));
                position += length;
                part += 1;
            }
            result
        };
        assert_eq!(run(&[2048]), run(&[1, 7, 43, 2, 127, 19, 3]));
    }
}

#[test]
fn late_invalid_gain_layout_or_rate_rejects_entire_bank_without_advancing_dsp() {
    let mut session = fixture(48000);
    session.inputs[0].dsp = effects();
    session.buses[0].dsp = effects();
    let mut actual = mixer(&session);
    let mut reference = mixer(&session);
    let source: Vec<_> = (0..500).map(|n| (n as f32 * 0.2).sin() * 0.5).collect();
    assert_eq!(
        render(&mut actual, &session, &source),
        render(&mut reference, &session, &source)
    );
    let mut changed = session.clone();
    changed.inputs[0].controls.muted = true;
    changed.inputs[0].dsp = DspConfig::default();
    changed.buses[0].dsp.high_pass.as_mut().unwrap().cutoff_hz = 8000.0;
    let complete = bank(&changed);
    let mut other_rate = session.clone();
    other_rate.sample_rate = 96000;
    let other_rate = bank(&other_rate);
    for problem in 0..6 {
        let mut invalid = complete.clone();
        let expected = match problem {
            0 => {
                invalid.buses[1].gains[0] = f32::NAN;
                ControlError::InvalidGain
            }
            1 => {
                invalid.bus_dsp.pop();
                ControlError::LayoutMismatch
            }
            2 => {
                invalid.inputs[0].id = Uuid::nil();
                ControlError::LayoutMismatch
            }
            3 => {
                invalid.sample_rate = 96000;
                ControlError::SampleRateMismatch
            }
            4 => {
                invalid.bus_dsp[1] = other_rate.bus_dsp[1];
                ControlError::SampleRateMismatch
            }
            _ => {
                invalid.input_dsp.clear();
                ControlError::LayoutMismatch
            }
        };
        assert_eq!(actual.apply_controls(&invalid), Err(expected));
        assert_eq!(
            render(&mut actual, &session, &source),
            render(&mut reference, &session, &source)
        );
    }
}

#[test]
fn input_and_bus_stages_apply_once_with_sends_unchanged() {
    let mut session = fixture(48000);
    let mut mixer = mixer(&session);
    let source = [0.4; 512];
    assert_eq!(render(&mut mixer, &session, &source), source);
    let gain = 10.0_f32.powf(-6.0 / 20.0);
    session.inputs[0].dsp.compressor = Some(CompressorConfig {
        ratio: 1.0,
        makeup_gain_db: -6.0,
        ..Default::default()
    });
    session.buses[0].dsp = session.inputs[0].dsp.clone();
    mixer.apply_controls(&bank(&session)).unwrap();
    let output = render(&mut mixer, &session, &source);
    assert!(output[..240].windows(2).all(|pair| pair[1] <= pair[0]));
    assert!(output[240..]
        .iter()
        .all(|sample| *sample == (0.4 * gain) * gain));
    let untouched = mixer
        .rendered_bus(mixer.resolve_bus(session.buses[1].id).unwrap())
        .unwrap();
    assert!(untouched[240..]
        .iter()
        .all(|sample| sample[0] == 0.4 * gain));
}

#[test]
fn dsp_control_bytes_are_charged_before_allocation_at_exact_boundary() {
    let session = fixture(48000);
    let bytes = std::mem::size_of::<PreparedControls>()
        + (session.inputs.len() + session.sends.len() + session.buses.len())
            * std::mem::size_of::<ResolvedControl>()
        + (session.inputs.len() + session.buses.len()) * std::mem::size_of::<PreparedDsp>();
    assert!(
        matches!(PreparedControls::compile(&session, bytes - 1), Err(SessionError::ResourceBudget { required, allowed, .. }) if required == bytes && allowed == bytes - 1)
    );
    assert!(PreparedControls::compile(&session, bytes).is_ok());
}

#[test]
fn decode_apply_retarget_and_render_allocate_and_deallocate_nothing() {
    let mut session = fixture(48000);
    let dry = bank(&session);
    session.inputs[0].dsp = effects();
    session.buses[0].dsp = effects();
    let wet = bank(&session);
    let mut invalid = wet.clone();
    invalid.sample_rate = 96000;
    let mut mixer = mixer(&session);
    let source = [0.7; 63];
    let mut left = [0.0; 63];
    let mut right = [0.0; 63];
    let words = wet.input_dsp[0].to_words();
    COUNTING.with(|enabled| enabled.set(true));
    for index in 0..1000 {
        let decoded = PreparedDsp::from_words(words).unwrap();
        assert_eq!(decoded, wet.input_dsp[0]);
        mixer
            .apply_controls(if index % 2 == 0 { &wet } else { &dry })
            .unwrap();
        assert_eq!(
            mixer.apply_controls(&invalid),
            Err(ControlError::SampleRateMismatch)
        );
        mixer
            .render(
                &[StereoInput {
                    id: session.inputs[0].id,
                    left: &source,
                    right: &source,
                }],
                &mut [StereoOutput {
                    id: session.buses[0].id,
                    left: &mut left,
                    right: &mut right,
                }],
                63,
            )
            .unwrap();
    }
    COUNTING.with(|enabled| enabled.set(false));
    OPERATIONS.with(|operations| assert_eq!(operations.get(), 0));
}
