//! Selection and private graph construction for Sound Check. This layer never
//! discovers, opens or starts devices; native ownership remains a separate gate.
use crate::{validate_session, AudioError, OutputId, OutputPlan};
use relink_core::{Bus, ChannelControls, DspConfig, Input, InputKind, Send, Session, Uuid};

/// A source identity and one explicitly chosen audition endpoint. Studio routes
/// are deliberately absent: neither graph builder can import a broadcast bus,
/// another input, bus effects or a second output from the user's studio.
#[derive(Clone, Debug)]
pub struct SoundCheckSelection {
    studio_id: Uuid,
    source_id: Uuid,
    source_kind: InputKind,
    sample_rate: u32,
    graph_id: Uuid,
    bus_id: Uuid,
    send_id: Uuid,
    output: OutputId,
}
impl SoundCheckSelection {
    /// Validate the selection without touching any native device. A later native
    /// host must still prove exclusive ownership, exact format and permissions.
    pub fn prepare(studio: &Session, source: Uuid, output: OutputId) -> Result<Self, AudioError> {
        studio.validate().map_err(invalid)?;
        let input = studio
            .inputs
            .iter()
            .find(|input| input.id == source)
            .ok_or_else(|| invalid("the Sound Check source no longer exists"))?;
        let result = Self {
            studio_id: studio.id,
            source_id: source,
            source_kind: input.kind.clone(),
            sample_rate: studio.sample_rate,
            graph_id: Uuid::new_v4(),
            bus_id: Uuid::new_v4(),
            send_id: Uuid::new_v4(),
            output,
        };
        let graph = result.recording_graph(studio)?;
        validate_session(&graph, result.bus_id)?;
        result.output_plan().validate(&graph).map_err(invalid)?;
        Ok(result)
    }
    pub fn source_id(&self) -> Uuid {
        self.source_id
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn bus_id(&self) -> Uuid {
        self.bus_id
    }
    pub fn output(&self) -> &OutputId {
        &self.output
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.output.0.capacity()
            + match &self.source_kind {
                InputKind::Hardware { device_id } => device_id.capacity(),
                InputKind::Application { application_id } => application_id.capacity(),
                InputKind::Virtual { endpoint_id } => endpoint_id.capacity(),
                InputKind::TestTone { .. } => 0,
            }
    }

    pub fn output_plan(&self) -> OutputPlan {
        OutputPlan::single(self.output.clone(), self.bus_id)
    }

    /// Selected source at unity, before ReLink effects. Native recording must
    /// append raw capture-group PCM and emit silence to the audition endpoint;
    /// this graph alone does not silence output or authorize recording. The
    /// neutral graph also provides the selected built-in tone when applicable.
    pub fn recording_graph(&self, studio: &Session) -> Result<Session, AudioError> {
        let mut input = self.current_source(studio)?.clone();
        input.controls = ChannelControls::default();
        input.dsp = DspConfig::default();
        self.graph(input)
    }

    /// One current source effect chain, with unity send/bus controls and no bus
    /// processing. The playback host must feed immutable clip samples for this
    /// source and must never call capture selection/build for this graph.
    pub fn playback_graph(&self, studio: &Session) -> Result<Session, AudioError> {
        self.graph(self.current_source(studio)?.clone())
    }

    fn current_source<'a>(&self, studio: &'a Session) -> Result<&'a Input, AudioError> {
        studio.validate().map_err(invalid)?;
        if studio.id != self.studio_id || studio.sample_rate != self.sample_rate {
            return Err(invalid("the Sound Check studio or sample rate changed"));
        }
        let input = studio
            .inputs
            .iter()
            .find(|input| input.id == self.source_id)
            .ok_or_else(|| invalid("the Sound Check source no longer exists"))?;
        if input.kind != self.source_kind {
            return Err(invalid(
                "the Sound Check source identity changed; clear the old clip",
            ));
        }
        Ok(input)
    }

    fn graph(&self, input: Input) -> Result<Session, AudioError> {
        let mut session = Session::new("Sound Check audition", self.sample_rate);
        session.id = self.graph_id;
        session.add_input(input).map_err(invalid)?;
        let mut bus = Bus::new("Sound Check output");
        bus.id = self.bus_id;
        // Bus::new normally installs a master limiter. Audition must apply only
        // the chosen source's chain, exactly once, including when over full scale.
        bus.dsp = DspConfig::default();
        session.add_bus(bus).map_err(invalid)?;
        let mut send = Send::new(self.source_id, self.bus_id);
        send.id = self.send_id;
        session.add_send(send).map_err(invalid)?;
        session.validate().map_err(invalid)?;
        Ok(session)
    }
}
fn invalid(error: impl std::fmt::Display) -> AudioError {
    AudioError::InvalidSession(error.to_string())
}

#[cfg(test)]
#[path = "sound_check_plan_tests.rs"]
mod tests;
