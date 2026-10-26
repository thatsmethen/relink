use serde::{Deserialize, Serialize};
use std::{collections::HashSet, error::Error, fmt};
use uuid::Uuid;

pub const SESSION_VERSION: u32 = 1;

/// Stereo channel controls. A linked gain uses `gain_db` for both channels.
/// Unlinking preserves the last linked value; pan is a stereo balance control
/// (center is unity, full left silences right, full right silences left).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelControls {
    pub gain_db: f32,
    pub right_gain_db: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub linked: bool,
}

impl Default for ChannelControls {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            right_gain_db: 0.0,
            pan: 0.0,
            muted: false,
            solo: false,
            linked: true,
        }
    }
}

impl ChannelControls {
    pub fn set_gain_db(&mut self, gain_db: f32) {
        self.gain_db = gain_db;
        if self.linked {
            self.right_gain_db = gain_db;
        }
    }

    pub fn set_right_gain_db(&mut self, gain_db: f32) {
        self.right_gain_db = gain_db;
        if self.linked {
            self.gain_db = gain_db;
        }
    }

    pub fn set_linked(&mut self, linked: bool) {
        if linked || self.linked {
            self.right_gain_db = self.gain_db;
        }
        self.linked = linked;
    }

    pub(crate) fn gains(&self) -> [f32; 2] {
        let right_db = if self.linked {
            self.gain_db
        } else {
            self.right_gain_db
        };
        [
            db_to_gain(self.gain_db) * (1.0 - self.pan.max(0.0)),
            db_to_gain(right_db) * (1.0 + self.pan.min(0.0)),
        ]
    }

    fn validate(&self, label: &str) -> Result<(), SessionError> {
        bounded(self.gain_db, -120.0, 24.0, &format!("{label}.gain_db"))?;
        bounded(
            self.right_gain_db,
            -120.0,
            24.0,
            &format!("{label}.right_gain_db"),
        )?;
        bounded(self.pan, -1.0, 1.0, &format!("{label}.pan"))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputKind {
    Hardware {
        device_id: String,
    },
    Application {
        application_id: String,
    },
    Virtual {
        endpoint_id: String,
    },
    TestTone {
        frequency_hz: f32,
        amplitude_db: f32,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HighPassConfig {
    pub cutoff_hz: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateConfig {
    pub threshold_db: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            threshold_db: -50.0,
            attack_ms: 2.0,
            hold_ms: 30.0,
            release_ms: 100.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompressorConfig {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_gain_db: f32,
}

impl Default for CompressorConfig {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 3.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_gain_db: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimiterConfig {
    pub ceiling_db: f32,
    pub release_ms: f32,
}

impl Default for LimiterConfig {
    fn default() -> Self {
        Self {
            ceiling_db: -0.3,
            release_ms: 50.0,
        }
    }
}

/// Processing order: high-pass, stereo-linked gate, stereo-linked compressor,
/// stereo-linked sample-peak limiter. `None` bypasses a stage. This limiter is
/// causal with immediate attenuation; it is not a look-ahead or true-peak limiter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DspConfig {
    pub high_pass: Option<HighPassConfig>,
    pub gate: Option<GateConfig>,
    pub compressor: Option<CompressorConfig>,
    pub limiter: Option<LimiterConfig>,
}

impl DspConfig {
    pub fn master_default() -> Self {
        Self {
            limiter: Some(LimiterConfig::default()),
            ..Self::default()
        }
    }

    fn validate(&self, sample_rate: u32, label: &str) -> Result<(), SessionError> {
        if let Some(p) = &self.high_pass {
            if !p.cutoff_hz.is_finite()
                || p.cutoff_hz <= 0.0
                || p.cutoff_hz >= sample_rate as f32 * 0.5
            {
                return Err(SessionError::Invalid(format!(
                    "{label}.high_pass.cutoff_hz must be positive and below Nyquist"
                )));
            }
        }
        if let Some(p) = &self.gate {
            bounded(
                p.threshold_db,
                -120.0,
                0.0,
                &format!("{label}.gate.threshold_db"),
            )?;
            bounded(
                p.attack_ms,
                0.01,
                5000.0,
                &format!("{label}.gate.attack_ms"),
            )?;
            bounded(p.hold_ms, 0.0, 5000.0, &format!("{label}.gate.hold_ms"))?;
            bounded(
                p.release_ms,
                0.01,
                10000.0,
                &format!("{label}.gate.release_ms"),
            )?;
        }
        if let Some(p) = &self.compressor {
            bounded(
                p.threshold_db,
                -120.0,
                0.0,
                &format!("{label}.compressor.threshold_db"),
            )?;
            bounded(p.ratio, 1.0, 100.0, &format!("{label}.compressor.ratio"))?;
            bounded(
                p.attack_ms,
                0.01,
                5000.0,
                &format!("{label}.compressor.attack_ms"),
            )?;
            bounded(
                p.release_ms,
                0.01,
                10000.0,
                &format!("{label}.compressor.release_ms"),
            )?;
            bounded(
                p.makeup_gain_db,
                -24.0,
                24.0,
                &format!("{label}.compressor.makeup_gain_db"),
            )?;
        }
        if let Some(p) = &self.limiter {
            bounded(
                p.ceiling_db,
                -60.0,
                0.0,
                &format!("{label}.limiter.ceiling_db"),
            )?;
            bounded(
                p.release_ms,
                0.01,
                10000.0,
                &format!("{label}.limiter.release_ms"),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub id: Uuid,
    pub name: String,
    pub kind: InputKind,
    pub controls: ChannelControls,
    pub dsp: DspConfig,
}

impl Input {
    pub fn new(name: impl Into<String>, kind: InputKind) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            kind,
            controls: ChannelControls::default(),
            dsp: DspConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bus {
    pub id: Uuid,
    pub name: String,
    pub controls: ChannelControls,
    pub dsp: DspConfig,
}

impl Bus {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            controls: ChannelControls::default(),
            dsp: DspConfig::master_default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
    pub id: Uuid,
    pub input_id: Uuid,
    pub bus_id: Uuid,
    pub controls: ChannelControls,
}

impl Send {
    pub fn new(input_id: Uuid, bus_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            input_id,
            bus_id,
            controls: ChannelControls::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub schema_version: u32,
    pub id: Uuid,
    pub name: String,
    pub sample_rate: u32,
    pub inputs: Vec<Input>,
    pub buses: Vec<Bus>,
    pub sends: Vec<Send>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    UnsupportedVersion {
        found: u32,
        supported: u32,
    },
    Invalid(String),
    DuplicateId(Uuid),
    MissingInput(Uuid),
    MissingBus(Uuid),
    MissingSend(Uuid),
    DuplicateRoute {
        input_id: Uuid,
        bus_id: Uuid,
    },
    Json(String),
    ResourceBudget {
        resource: &'static str,
        required: usize,
        allowed: usize,
    },
    AllocationFailed,
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion { found, supported } => write!(
                f,
                "session version {found} is unsupported; this engine supports {supported}"
            ),
            Self::Invalid(message) | Self::Json(message) => f.write_str(message),
            Self::DuplicateId(id) => write!(f, "duplicate object ID {id}"),
            Self::MissingInput(id) => write!(f, "input {id} does not exist"),
            Self::MissingBus(id) => write!(f, "bus {id} does not exist"),
            Self::MissingSend(id) => write!(f, "send {id} does not exist"),
            Self::DuplicateRoute { input_id, bus_id } => {
                write!(f, "duplicate send from input {input_id} to bus {bus_id}")
            }
            Self::ResourceBudget {
                resource,
                required,
                allowed,
            } => write!(
                f,
                "{resource} requires {required}, exceeding the configured budget of {allowed}"
            ),
            Self::AllocationFailed => f.write_str("unable to allocate mixer working memory"),
        }
    }
}

impl Error for SessionError {}

impl Session {
    pub fn new(name: impl Into<String>, sample_rate: u32) -> Self {
        Self {
            schema_version: SESSION_VERSION,
            id: Uuid::new_v4(),
            name: name.into(),
            sample_rate,
            inputs: Vec::new(),
            buses: Vec::new(),
            sends: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), SessionError> {
        if self.schema_version != SESSION_VERSION {
            return Err(SessionError::UnsupportedVersion {
                found: self.schema_version,
                supported: SESSION_VERSION,
            });
        }
        if !(8000..=384000).contains(&self.sample_rate) {
            return Err(SessionError::Invalid(
                "sample_rate must be between 8000 and 384000 Hz".into(),
            ));
        }
        nonempty(&self.name, "session.name")?;
        let mut ids = HashSet::new();
        unique_id(self.id, &mut ids)?;
        let mut input_ids = HashSet::new();
        let mut bus_ids = HashSet::new();
        for input in &self.inputs {
            unique_id(input.id, &mut ids)?;
            input_ids.insert(input.id);
            nonempty(&input.name, "input.name")?;
            input.controls.validate("input.controls")?;
            input.dsp.validate(self.sample_rate, "input.dsp")?;
            match &input.kind {
                InputKind::Hardware { device_id } => nonempty(device_id, "hardware.device_id")?,
                InputKind::Application { application_id } => {
                    nonempty(application_id, "application.application_id")?
                }
                InputKind::Virtual { endpoint_id } => nonempty(endpoint_id, "virtual.endpoint_id")?,
                InputKind::TestTone {
                    frequency_hz,
                    amplitude_db,
                } => {
                    if !frequency_hz.is_finite()
                        || *frequency_hz <= 0.0
                        || *frequency_hz >= self.sample_rate as f32 * 0.5
                    {
                        return Err(SessionError::Invalid(
                            "test tone frequency must be positive and below Nyquist".into(),
                        ));
                    }
                    bounded(*amplitude_db, -120.0, 0.0, "test_tone.amplitude_db")?;
                }
            }
        }
        for bus in &self.buses {
            unique_id(bus.id, &mut ids)?;
            bus_ids.insert(bus.id);
            nonempty(&bus.name, "bus.name")?;
            bus.controls.validate("bus.controls")?;
            bus.dsp.validate(self.sample_rate, "bus.dsp")?;
        }
        let mut routes = HashSet::new();
        for send in &self.sends {
            unique_id(send.id, &mut ids)?;
            if !input_ids.contains(&send.input_id) {
                return Err(SessionError::MissingInput(send.input_id));
            }
            if !bus_ids.contains(&send.bus_id) {
                return Err(SessionError::MissingBus(send.bus_id));
            }
            if !routes.insert((send.input_id, send.bus_id)) {
                return Err(SessionError::DuplicateRoute {
                    input_id: send.input_id,
                    bus_id: send.bus_id,
                });
            }
            send.controls.validate("send.controls")?;
        }
        Ok(())
    }

    /// Read a version before deserializing the remaining schema, so newer session
    /// versions produce an explicit version error even if their structure changed.
    pub fn from_json(json: &str) -> Result<Self, SessionError> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|e| SessionError::Json(e.to_string()))?;
        let version = value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                SessionError::Json("schema_version must be an unsigned integer".into())
            })?;
        if version != SESSION_VERSION as u64 {
            if let Ok(found) = u32::try_from(version) {
                return Err(SessionError::UnsupportedVersion {
                    found,
                    supported: SESSION_VERSION,
                });
            }
            return Err(SessionError::Json(
                "schema_version exceeds the supported integer range".into(),
            ));
        }
        let session: Self =
            serde_json::from_value(value).map_err(|e| SessionError::Json(e.to_string()))?;
        session.validate()?;
        Ok(session)
    }

    pub fn to_json(&self) -> Result<String, SessionError> {
        self.validate()?;
        serde_json::to_string_pretty(self).map_err(|e| SessionError::Json(e.to_string()))
    }

    /// Mutations roll back if they would make the session invalid.
    pub fn add_input(&mut self, input: Input) -> Result<Uuid, SessionError> {
        let id = input.id;
        self.inputs.push(input);
        if let Err(error) = self.validate() {
            self.inputs.pop();
            return Err(error);
        }
        Ok(id)
    }

    pub fn add_bus(&mut self, bus: Bus) -> Result<Uuid, SessionError> {
        let id = bus.id;
        self.buses.push(bus);
        if let Err(error) = self.validate() {
            self.buses.pop();
            return Err(error);
        }
        Ok(id)
    }

    pub fn add_send(&mut self, send: Send) -> Result<Uuid, SessionError> {
        let id = send.id;
        self.sends.push(send);
        if let Err(error) = self.validate() {
            self.sends.pop();
            return Err(error);
        }
        Ok(id)
    }

    pub fn remove_input(&mut self, id: Uuid) -> Result<Input, SessionError> {
        let index = self
            .inputs
            .iter()
            .position(|item| item.id == id)
            .ok_or(SessionError::MissingInput(id))?;
        self.sends.retain(|send| send.input_id != id);
        Ok(self.inputs.remove(index))
    }

    pub fn remove_bus(&mut self, id: Uuid) -> Result<Bus, SessionError> {
        let index = self
            .buses
            .iter()
            .position(|item| item.id == id)
            .ok_or(SessionError::MissingBus(id))?;
        self.sends.retain(|send| send.bus_id != id);
        Ok(self.buses.remove(index))
    }

    pub fn remove_send(&mut self, id: Uuid) -> Result<Send, SessionError> {
        let index = self
            .sends
            .iter()
            .position(|item| item.id == id)
            .ok_or(SessionError::MissingSend(id))?;
        Ok(self.sends.remove(index))
    }
}

fn unique_id(id: Uuid, ids: &mut HashSet<Uuid>) -> Result<(), SessionError> {
    if id.is_nil() {
        return Err(SessionError::Invalid("object IDs must not be nil".into()));
    }
    if !ids.insert(id) {
        return Err(SessionError::DuplicateId(id));
    }
    Ok(())
}

fn nonempty(value: &str, label: &str) -> Result<(), SessionError> {
    if value.trim().is_empty() {
        Err(SessionError::Invalid(format!("{label} must not be empty")))
    } else {
        Ok(())
    }
}

fn bounded(value: f32, min: f32, max: f32, label: &str) -> Result<(), SessionError> {
    if !value.is_finite() || value < min || value > max {
        Err(SessionError::Invalid(format!(
            "{label} must be finite and between {min} and {max}"
        )))
    } else {
        Ok(())
    }
}

pub(crate) fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}
