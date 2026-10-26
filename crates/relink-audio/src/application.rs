//! Saved application choices are stable identities, never process IDs.
//! Native discovery and activation resolve current process instances separately.
use crate::AudioError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationInfo {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ProcessInstance {
    pub(crate) pid: u32,
    pub(crate) started: u64,
}

#[cfg(target_os = "macos")]
pub(crate) use crate::application_macos::{select, ApplicationCapture, Selection};
#[cfg(target_os = "windows")]
pub(crate) use crate::application_windows::{select, ApplicationCapture, Selection};

pub(crate) fn enumerate() -> Result<Vec<ApplicationInfo>, AudioError> {
    #[cfg(target_os = "macos")]
    let result = crate::application_macos::enumerate();
    #[cfg(target_os = "windows")]
    let result = crate::application_windows::enumerate();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = Err(AudioError::InvalidSession(
        "application capture is available only on macOS and Windows".into(),
    ));
    result
}
