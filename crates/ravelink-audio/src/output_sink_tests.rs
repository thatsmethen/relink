use super::*;
use output_sink::InvalidOutputBuffer;
use ravelink_core::{Bus, Input, Send};

fn fixture() -> (RenderState, Session) {
    let mut session = Session::new("Callback sink controls", 48000);
    let input = session
        .add_input(Input::new(
            "Tone",
            InputKind::TestTone {
                frequency_hz: 997.0,
                amplitude_db: -12.0,
            },
        ))
        .unwrap();
    let bus = session.add_bus(Bus::new("Mix")).unwrap();
    session.add_send(Send::new(input, bus)).unwrap();
    (tests::render(&session, bus, 2), session)
}

struct ProbeSink {
    pcm: Vec<f32>,
    advertised_frames: usize,
    fail_at: Option<usize>,
    cancel_at: Option<usize>,
    wanted: Arc<AtomicBool>,
    writes: usize,
    clears: usize,
}
impl ProbeSink {
    fn new(frames: usize, wanted: Arc<AtomicBool>) -> Self {
        Self {
            pcm: vec![9.0; frames * 2],
            advertised_frames: frames,
            fail_at: None,
            cancel_at: None,
            wanted,
            writes: 0,
            clears: 0,
        }
    }
}
impl StereoSink for ProbeSink {
    fn frames(&self) -> Result<usize, InvalidOutputBuffer> {
        Ok(self.advertised_frames)
    }
    fn clear(&mut self) {
        self.pcm.fill(0.0);
        self.clears += 1;
    }
    fn write_stereo(
        &mut self,
        start: usize,
        left: &[f32],
        right: &[f32],
    ) -> Result<(), InvalidOutputBuffer> {
        self.writes += 1;
        if self.fail_at == Some(start) {
            return Err(InvalidOutputBuffer);
        }
        for (frame, (&left, &right)) in left.iter().zip(right).enumerate() {
            self.pcm[(start + frame) * 2] = left;
            self.pcm[(start + frame) * 2 + 1] = right;
        }
        if self.cancel_at == Some(start) {
            self.wanted.store(false, Ordering::Release);
        }
        Ok(())
    }
}

#[test]
fn callback_sink_preserves_one_source_sequence_and_reports_only_completed_render() {
    let (mut render, _) = fixture();
    let mut position = 0;
    for frames in [17, RENDER_CHUNK + 7, 0, 511] {
        let mut sink = ProbeSink::new(frames, render.wanted.clone());
        let outcome = render.write_to(&mut sink);
        assert_eq!(
            outcome,
            if frames == 0 {
                RenderOutcome::Silence
            } else {
                RenderOutcome::Rendered {
                    frames,
                    accepted_version: Some(0),
                }
            }
        );
        for frame in 0..frames {
            let expected = ((position + frame) as f64 * std::f64::consts::TAU * 997.0 / 48000.0)
                .sin() as f32
                * 10.0_f32.powf(-12.0 / 20.0);
            assert!((sink.pcm[frame * 2] - expected).abs() < 2.0e-6);
            assert_eq!(sink.pcm[frame * 2], sink.pcm[frame * 2 + 1]);
        }
        position += frames;
    }
    assert_eq!(
        render.telemetry.frames.load(Ordering::Relaxed),
        position as u64
    );
}

#[test]
fn sink_failure_after_a_chunk_clears_every_frame_and_withholds_acknowledgment() {
    let (mut render, session) = fixture();
    let mut next = session.clone();
    next.buses[0].controls.set_gain_db(-6.0);
    render
        .bank
        .publish(&prepare_live_update(&session, &next).unwrap())
        .unwrap();
    let mut sink = ProbeSink::new(RENDER_CHUNK + 17, render.wanted.clone());
    sink.fail_at = Some(RENDER_CHUNK);
    assert_eq!(render.write_to(&mut sink), RenderOutcome::Silence);
    assert_eq!(sink.writes, 2);
    assert_eq!(sink.clears, 2);
    assert!(sink.pcm.iter().all(|sample| *sample == 0.0));
    assert_eq!(
        render.telemetry.fault.load(Ordering::Acquire),
        AudioFault::InvalidBuffer as u32
    );
    assert_eq!(render.telemetry.frames.load(Ordering::Relaxed), 0);
    assert_eq!(
        render
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        0
    );
}

#[test]
fn cancellation_during_sink_copy_clears_the_complete_callback_without_acknowledging() {
    let (mut render, session) = fixture();
    let mut next = session.clone();
    next.buses[0].controls.set_gain_db(-6.0);
    render
        .bank
        .publish(&prepare_live_update(&session, &next).unwrap())
        .unwrap();
    let mut sink = ProbeSink::new(RENDER_CHUNK + 5, render.wanted.clone());
    sink.cancel_at = Some(0);
    assert_eq!(render.write_to(&mut sink), RenderOutcome::Silence);
    assert!(sink.pcm.iter().all(|sample| *sample == 0.0));
    assert_eq!(render.telemetry.frames.load(Ordering::Relaxed), 0);
    assert_eq!(
        render
            .telemetry
            .applied_control_version
            .load(Ordering::Acquire),
        0
    );
    assert_eq!(render.telemetry.fault.load(Ordering::Acquire), 0);
}

#[test]
fn first_sink_callback_and_rejected_extent_allocate_nothing() {
    let (mut render, _) = fixture();
    let mut sink = ProbeSink::new(257, render.wanted.clone());
    tests::OPERATIONS.with(|n| n.set(0));
    tests::COUNTING.with(|on| on.set(true));
    let outcome = render.write_to(&mut sink);
    sink.advertised_frames = MAX_CALLBACK_FRAMES + 1;
    let rejected = render.write_to(&mut sink);
    tests::COUNTING.with(|on| on.set(false));
    assert_eq!(tests::OPERATIONS.with(|n| n.get()), 0);
    assert_eq!(
        outcome,
        RenderOutcome::Rendered {
            frames: 257,
            accepted_version: Some(0)
        }
    );
    assert_eq!(rejected, RenderOutcome::Silence);
    assert!(sink.pcm.iter().all(|sample| *sample == 0.0));
    assert_eq!(sink.writes, 1);
    assert_eq!(render.telemetry.frames.load(Ordering::Relaxed), 257);
    assert_eq!(
        render.telemetry.fault.load(Ordering::Acquire),
        AudioFault::InvalidBuffer as u32
    );
}
