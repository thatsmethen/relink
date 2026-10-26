//! Control-thread selection and dispatch for original native output owners.
use crate::{
    AudioError, OutputDevice, OutputRenderer, RunningConfiguration, RunningInput, RunningOutput,
    Uuid,
};
use std::{sync::Arc, time::Duration};

#[cfg(target_os = "macos")]
use crate::{native_output, native_output_timing};
#[cfg(target_os = "windows")]
use crate::{wasapi_output, wasapi_output_state};

enum NativeSelection {
    #[cfg(target_os = "macos")]
    CoreAudio(native_output::Selection),
    #[cfg(target_os = "windows")]
    Wasapi(wasapi_output::Selection),
}

pub(crate) struct Selection {
    native: NativeSelection,
    device: OutputDevice,
}

pub(crate) fn select(
    device: &cpal::Device,
    output: OutputDevice,
    rate: u32,
) -> Result<Selection, AudioError> {
    #[cfg(target_os = "macos")]
    let native = {
        // CPAL supplies identity/discovery only. Current stream ASBDs are read
        // independently; no nominal rate or virtual/physical format is changed.
        let _ = device;
        NativeSelection::CoreAudio(native_output::select(&output.id.0, rate)?)
    };
    #[cfg(target_os = "windows")]
    let native = {
        use cpal::traits::DeviceTrait;
        let config = crate::select_config(
            device.supported_output_configs().map_err(|error| {
                crate::backend("output format discovery", Some(output.name.clone()), error)
            })?,
            rate,
        )
        .ok_or_else(|| AudioError::UnsupportedConfiguration {
            device: output.name.clone(),
            sample_rate: rate,
        })?;
        // The original owner revalidates this exact shared-mode format with
        // WASAPI before opening its stream. Closest-format fallback is absent.
        NativeSelection::Wasapi(wasapi_output::select(&output, &config)?)
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Err(AudioError::UnsupportedPlatform);
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    Ok(Selection {
        native,
        device: output,
    })
}

impl Selection {
    pub(crate) fn channels(&self) -> u16 {
        match &self.native {
            #[cfg(target_os = "macos")]
            NativeSelection::CoreAudio(selection) => selection.channels(),
            #[cfg(target_os = "windows")]
            NativeSelection::Wasapi(selection) => selection.channels(),
        }
    }
    pub(crate) fn owner_bytes(&self) -> Result<usize, AudioError> {
        let native = match &self.native {
            #[cfg(target_os = "macos")]
            NativeSelection::CoreAudio(selection) => selection.owner_bytes()?,
            #[cfg(target_os = "windows")]
            NativeSelection::Wasapi(selection) => selection.owner_bytes()?,
        };
        native
            .checked_add(std::mem::size_of::<Self>())
            .and_then(|n| n.checked_add(crate::output_progress::STACK_BYTES))
            .and_then(|n| n.checked_add(std::mem::size_of::<Diagnostics>()))
            .and_then(|n| n.checked_add(std::mem::size_of::<crate::OutputPermit>()))
            .and_then(|n| n.checked_add(self.device.id.0.capacity()))
            .and_then(|n| n.checked_add(self.device.name.capacity()))
            .ok_or_else(|| AudioError::InvalidSession("output memory estimate overflow".into()))
    }
    pub(crate) fn configuration(
        &self,
        bus_id: Uuid,
        inputs: Vec<RunningInput>,
    ) -> RunningConfiguration {
        let (sample_rate, sample_format) = match &self.native {
            #[cfg(target_os = "macos")]
            NativeSelection::CoreAudio(selection) => (selection.rate(), selection.description()),
            #[cfg(target_os = "windows")]
            NativeSelection::Wasapi(selection) => (selection.rate(), selection.description()),
        };
        RunningConfiguration {
            outputs: vec![self.running_output(bus_id)],
            clock_output_index: 0,
            output: self.device.clone(),
            inputs,
            bus_id,
            sample_rate,
            device_channels: self.channels(),
            sample_format,
        }
    }
    pub(crate) fn running_output(&self, bus_id: Uuid) -> RunningOutput {
        let (sample_rate, sample_format) = match &self.native {
            #[cfg(target_os = "macos")]
            NativeSelection::CoreAudio(selection) => (selection.rate(), selection.description()),
            #[cfg(target_os = "windows")]
            NativeSelection::Wasapi(selection) => (selection.rate(), selection.description()),
        };
        RunningOutput {
            output: self.device.clone(),
            bus_id,
            sample_rate,
            device_channels: self.channels(),
            sample_format,
        }
    }
    pub(crate) fn diagnostics(&self) -> Diagnostics {
        match &self.native {
            #[cfg(target_os = "macos")]
            NativeSelection::CoreAudio(_) => {
                Diagnostics::CoreAudio(Arc::new(native_output_timing::NativeOutputTiming::new()))
            }
            #[cfg(target_os = "windows")]
            NativeSelection::Wasapi(_) => {
                Diagnostics::Wasapi(Arc::new(wasapi_output_state::WasapiOutputTiming::new()))
            }
        }
    }
    pub(crate) fn build(
        self,
        render: OutputRenderer,
        diagnostics: &Diagnostics,
    ) -> Result<Stream, AudioError> {
        match (self.native, diagnostics) {
            #[cfg(target_os = "macos")]
            (NativeSelection::CoreAudio(selection), Diagnostics::CoreAudio(timing)) => {
                native_output::NativeOutput::build(selection, render, timing.clone())
                    .map(Stream::CoreAudio)
            }
            #[cfg(target_os = "windows")]
            (NativeSelection::Wasapi(selection), Diagnostics::Wasapi(timing)) => {
                wasapi_output::WasapiOutput::build(selection, render, timing.clone())
                    .map(Stream::Wasapi)
            }
        }
    }
}

/// Native observations retained after a stream handle is cancelled or dropped.
/// CoreAudio handoff and WASAPI ReleaseBuffer are different software boundaries;
/// neither establishes that samples reached a speaker.
#[derive(Clone, Debug)]
pub enum OutputTimingSnapshot {
    #[cfg(target_os = "macos")]
    CoreAudio(native_output_timing::NativeOutputTimingSnapshot),
    #[cfg(target_os = "windows")]
    Wasapi(wasapi_output_state::WasapiOutputSnapshot),
}

#[derive(Clone)]
pub(crate) enum Diagnostics {
    #[cfg(target_os = "macos")]
    CoreAudio(Arc<native_output_timing::NativeOutputTiming>),
    #[cfg(target_os = "windows")]
    Wasapi(Arc<wasapi_output_state::WasapiOutputTiming>),
}
impl Diagnostics {
    pub(crate) fn progress_count(&self) -> u64 {
        match self {
            #[cfg(target_os = "macos")]
            Self::CoreAudio(timing) => timing.progress_count(),
            #[cfg(target_os = "windows")]
            Self::Wasapi(timing) => timing.progress_count(),
        }
    }
    pub(crate) fn snapshot(&self) -> OutputTimingSnapshot {
        match self {
            #[cfg(target_os = "macos")]
            Self::CoreAudio(timing) => OutputTimingSnapshot::CoreAudio(timing.snapshot()),
            #[cfg(target_os = "windows")]
            Self::Wasapi(timing) => OutputTimingSnapshot::Wasapi(timing.snapshot()),
        }
    }
}

pub(crate) enum Stream {
    #[cfg(target_os = "macos")]
    CoreAudio(native_output::NativeOutput),
    #[cfg(target_os = "windows")]
    Wasapi(wasapi_output::WasapiOutput),
}
impl crate::output_startup::OutputStart for Stream {
    fn start(&self) -> Result<(), AudioError> {
        match self {
            #[cfg(target_os = "macos")]
            Self::CoreAudio(stream) => stream.start(),
            #[cfg(target_os = "windows")]
            Self::Wasapi(stream) => stream.start(),
        }
    }
    fn wait_for_progress(&self, timeout: Duration) -> Result<(), AudioError> {
        match self {
            #[cfg(target_os = "macos")]
            Self::CoreAudio(stream) => stream.wait_for_progress(timeout),
            #[cfg(target_os = "windows")]
            Self::Wasapi(stream) => stream.wait_for_progress(timeout),
        }
    }
    fn activate(&self) -> Result<(), AudioError> {
        match self {
            #[cfg(target_os = "macos")]
            Self::CoreAudio(stream) => stream.activate(),
            #[cfg(target_os = "windows")]
            Self::Wasapi(stream) => stream.activate(),
        }
    }
    fn cancel(&self) {
        match self {
            #[cfg(target_os = "macos")]
            Self::CoreAudio(stream) => stream.cancel(),
            #[cfg(target_os = "windows")]
            Self::Wasapi(stream) => stream.cancel(),
        }
    }
}
