//! ReLink's independently authored session and stereo render engine.
//!
//! Edit and persist [`Session`] on a control thread, then build a [`CompiledMixer`].
//! Compilation validates the entire graph and allocates its working memory. The
//! [`CompiledMixer::render`] callback allocates nothing and takes no locks. Graph
//! replacement and meter publication belong to the host; do not compile, destroy,
//! serialize, or edit sessions on an audio callback.
//!
//! Inputs and buses have stable UUIDs. Sends form a sparse input-to-bus matrix;
//! there is no product channel-count limit. [`ResourceBudget`] makes memory and
//! block-size limits explicit for each host. Every bus is an independent stereo
//! mix with its own master controls and DSP.

mod application;
mod controls;
mod dsp;
mod mixer;
mod session;
mod smoothing;
mod sound_check;

pub use application::{ApplicationSelector, ApplicationSelectorError};
pub use controls::{ControlError, PreparedControls, ResolvedControl};
pub use dsp::PreparedDsp;
pub use mixer::{
    BusHandle, BusReadError, CompiledMixer, MeterReading, RenderError, ResourceBudget, StereoInput,
    StereoOutput,
};
pub use session::{
    Bus, ChannelControls, CompressorConfig, DspBypass, DspConfig, GateConfig, HighPassConfig,
    Input, InputKind, LimiterConfig, Send, Session, SessionError, SESSION_VERSION,
};
pub use sound_check::{
    SoundCheck, SoundCheckCaptureProgress, SoundCheckClip, SoundCheckClipView, SoundCheckError,
    SoundCheckPlaybackProgress, SoundCheckSnapshot, SoundCheckState, SoundCheckToken,
    SOUND_CHECK_MAX_SECONDS,
};
pub use uuid::Uuid;
