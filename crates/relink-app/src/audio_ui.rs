use eframe::egui;
use relink_audio::{AudioError, AudioRuntime, AudioSnapshot, OutputPlan, StartCancellation};
use relink_core::Session;
use std::{
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::Duration,
};
use uuid::Uuid;

pub enum Request {
    Refresh,
    RefreshApplications,
    Start(Session, OutputPlan),
    Update(Session),
    Stop,
}
#[derive(Clone, Copy)]
enum Operation {
    Refresh,
    RefreshApplications,
    Start,
    Update,
    Stop,
}
impl Request {
    fn operation(&self) -> Operation {
        match self {
            Self::Refresh => Operation::Refresh,
            Self::RefreshApplications => Operation::RefreshApplications,
            Self::Start(..) => Operation::Start,
            Self::Update(_) => Operation::Update,
            Self::Stop => Operation::Stop,
        }
    }
}
struct Command {
    request: Request,
    cancellation: StartCancellation,
}
pub struct Device {
    pub id: String,
    pub name: String,
}
enum Event {
    Devices(Vec<Device>),
    Inputs(Vec<Device>),
    Applications(Vec<Device>),
    Snapshot(Box<AudioSnapshot>),
    Finished {
        operation: Operation,
        result: Result<Option<u64>, AudioError>,
    },
}
pub struct AudioUi {
    commands: SyncSender<Command>,
    events: Receiver<Event>,
    runtime: Option<Arc<AudioRuntime>>,
    pub devices: Vec<Device>,
    pub inputs: Vec<Device>,
    pub applications: Vec<Device>,
    pub applications_loaded: bool,
    pub applications_error: Option<String>,
    pub busy: bool,
    pub error: Option<String>,
    snapshot: Option<AudioSnapshot>,
    updating: bool,
    pending_update: Option<Session>,
    wanted_version: u64,
    cancelled: bool,
    starting: bool,
    pending_operations: usize,
    cancellation: StartCancellation,
}
impl AudioUi {
    pub fn new(ctx: egui::Context) -> Self {
        let (commands, requests) = mpsc::sync_channel::<Command>(4);
        let (sender, events) = mpsc::sync_channel(8);
        let (runtime, mut error) = match AudioRuntime::new() {
            Ok(r) => (Some(Arc::new(r)), None),
            Err(e) => (None, Some(e.to_string())),
        };
        if let Some(owner) = runtime.clone() {
            let spawned = thread::Builder::new()
                .name("relink-ui-audio".into())
                .spawn(move || loop {
                    match requests.recv_timeout(Duration::from_millis(50)) {
                        Ok(Command {
                            request,
                            cancellation,
                        }) => {
                            let operation = request.operation();
                            let result = match request {
                                Request::Refresh => {
                                    owner.enumerate_output_devices().and_then(|devices| {
                                        let _ = sender.send(Event::Devices(
                                            devices
                                                .into_iter()
                                                .map(|d| Device {
                                                    id: d.id.0,
                                                    name: d.name,
                                                })
                                                .collect(),
                                        ));
                                        owner.enumerate_input_devices().map(|devices| {
                                            let _ = sender.send(Event::Inputs(
                                                devices
                                                    .into_iter()
                                                    .map(|d| Device {
                                                        id: d.id.0,
                                                        name: d.name,
                                                    })
                                                    .collect(),
                                            ));
                                            None
                                        })
                                    })
                                }
                                Request::RefreshApplications => {
                                    owner.enumerate_applications().map(|applications| {
                                        let _ = sender.send(Event::Applications(
                                            applications
                                                .into_iter()
                                                .map(|application| Device {
                                                    id: application.id,
                                                    name: application.name,
                                                })
                                                .collect(),
                                        ));
                                        None
                                    })
                                }
                                Request::Start(session, plan) => owner
                                    .start_outputs_cancellable(session, plan, cancellation)
                                    .map(|_| None),
                                Request::Update(session) => {
                                    owner.update_controls(session).map(Some)
                                }
                                Request::Stop => owner.stop().map(|_| None),
                            };
                            if matches!(operation, Operation::Update) && result.is_err() {
                                owner.cancel_output();
                            }
                            let _ = sender.send(Event::Snapshot(Box::new(owner.snapshot())));
                            if sender.send(Event::Finished { operation, result }).is_err() {
                                break;
                            }
                            ctx.request_repaint();
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let snapshot = owner.snapshot();
                            if snapshot.running
                                || snapshot.error.is_some()
                                || snapshot.fault != relink_audio::AudioFault::None
                            {
                                let _ = sender.try_send(Event::Snapshot(Box::new(snapshot)));
                                ctx.request_repaint();
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            owner.cancel_output();
                            let _ = owner.stop();
                            break;
                        }
                    }
                });
            if let Err(e) = spawned {
                error = Some(format!("Cannot create audio control worker: {e}"));
            }
        }
        let mut result = Self {
            commands,
            events,
            runtime,
            devices: vec![],
            inputs: vec![],
            applications: vec![],
            applications_loaded: false,
            applications_error: None,
            busy: false,
            error,
            snapshot: None,
            updating: false,
            pending_update: None,
            wanted_version: 0,
            cancelled: false,
            starting: false,
            pending_operations: 0,
            cancellation: StartCancellation::default(),
        };
        if result.error.is_none() {
            result.send(Request::Refresh);
        }
        result
    }
    pub fn send(&mut self, request: Request) {
        let operation = request.operation();
        if matches!(operation, Operation::Stop) {
            // Cancel the queued start itself as well as active callback output.
            // Hardware-queued samples cannot be recalled.
            self.cancellation.cancel();
            if let Some(runtime) = &self.runtime {
                runtime.cancel_output();
            }
            self.cancelled = true;
            self.pending_update = None;
        }
        if matches!(operation, Operation::Start) {
            self.cancellation = StartCancellation::default();
            self.cancelled = false;
            self.wanted_version = 0;
        }
        match self.commands.try_send(Command {
            request,
            cancellation: self.cancellation.clone(),
        }) {
            Ok(()) => {
                if matches!(operation, Operation::Update) {
                    self.updating = true;
                } else {
                    self.pending_operations += 1;
                    self.busy = true;
                }
                if matches!(operation, Operation::Start) {
                    self.starting = true;
                }
                if matches!(operation, Operation::RefreshApplications) {
                    self.applications_error = None;
                } else {
                    self.error = None;
                }
            }
            Err(e) => {
                let error = Some(format!("Audio control unavailable: {e}"));
                if matches!(operation, Operation::RefreshApplications) {
                    self.applications_error = error;
                } else {
                    self.error = error;
                }
            }
        }
    }
    pub fn queue_controls(&mut self, session: Session) {
        if self.running() {
            self.pending_update = Some(session);
        }
    }
    pub fn poll(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Devices(devices) => self.devices = devices,
                Event::Inputs(devices) => self.inputs = devices,
                Event::Applications(applications) => {
                    self.applications = applications;
                    self.applications_loaded = true;
                }
                Event::Snapshot(snapshot) => {
                    if !snapshot.running
                        && (snapshot.error.is_some()
                            || snapshot.fault != relink_audio::AudioFault::None)
                    {
                        self.pending_update = None;
                    }
                    // Stop revokes playback immediately, but native teardown
                    // can fail later. Suppress only intentional cancellation.
                    if snapshot.fault != relink_audio::AudioFault::None {
                        self.error = Some(snapshot.fault.message().into());
                    }
                    if let Some(e) = &snapshot.error {
                        if !self.cancelled || !matches!(e, AudioError::Cancelled) {
                            self.error = Some(e.to_string());
                        }
                    }
                    self.snapshot = Some(*snapshot);
                }
                Event::Finished { operation, result } => {
                    if matches!(operation, Operation::Update) {
                        self.updating = false;
                    } else {
                        self.pending_operations = self.pending_operations.saturating_sub(1);
                        self.busy = self.pending_operations > 0;
                    }
                    if matches!(operation, Operation::Start) {
                        self.starting = false;
                    }
                    if matches!(operation, Operation::RefreshApplications) {
                        self.applications_loaded = true;
                        self.applications_error = result.err().map(|error| error.to_string());
                        if self.applications_error.is_some() {
                            self.applications.clear();
                        }
                        continue;
                    }
                    match result {
                        Ok(Some(version)) => self.wanted_version = version,
                        Ok(None) if matches!(operation, Operation::Start) => {
                            // This worker sends the start's snapshot before
                            // its completion on the same FIFO event channel.
                            // Retire an older queued error only if this start
                            // is still wanted and its snapshot is healthy.
                            if !self.cancelled
                                && self.snapshot.as_ref().is_some_and(|snapshot| {
                                    snapshot.running
                                        && snapshot.fault == relink_audio::AudioFault::None
                                        && snapshot.error.is_none()
                                })
                            {
                                self.error = None;
                            }
                        }
                        Err(e) => {
                            if !self.cancelled || !matches!(e, AudioError::Cancelled) {
                                self.error = Some(e.to_string());
                            }
                            if matches!(operation, Operation::Update) {
                                self.pending_update = None;
                                self.cancelled = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if !self.busy && !self.updating && self.running() {
            if let Some(session) = self.pending_update.take() {
                self.send(Request::Update(session));
            }
        }
    }
    pub fn running(&self) -> bool {
        !self.cancelled && self.snapshot.as_ref().is_some_and(|s| s.running)
    }
    pub fn can_stop(&self) -> bool {
        self.running() || (self.starting && !self.cancelled)
    }
    pub fn controls_pending(&self) -> bool {
        self.pending_update.is_some()
            || self.updating
            || (self.running()
                && self.snapshot.as_ref().is_some_and(|s| {
                    s.applied_control_version < self.wanted_version
                        || (self.wanted_version > 0
                            && s.outputs.iter().any(|output| {
                                output
                                    .submitted_control_version
                                    .is_none_or(|version| version < self.wanted_version)
                            }))
                }))
    }
    pub fn bus_peak(&self, bus: Option<Uuid>) -> [f32; 2] {
        if !self.running() {
            return [0.0; 2];
        }
        self.snapshot
            .as_ref()
            .and_then(|s| s.bus_meters.iter().find(|m| Some(m.id) == bus))
            .map(|m| m.peak)
            .unwrap_or([0.0; 2])
    }
    pub fn snapshot(&self) -> Option<&AudioSnapshot> {
        self.snapshot.as_ref()
    }
    pub fn wanted_version(&self) -> u64 {
        self.wanted_version
    }
    pub fn input_peak(&self, id: Uuid) -> [f32; 2] {
        if !self.running() {
            return [0.0; 2];
        }
        self.snapshot
            .as_ref()
            .and_then(|s| s.input_meters.iter().find(|m| m.id == id))
            .map(|m| m.peak)
            .unwrap_or([0.0; 2])
    }

    #[cfg(test)]
    pub(crate) fn offline_for_test() -> Self {
        tests::fixture().0
    }
}
impl Drop for AudioUi {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(r) = &self.runtime {
            r.cancel_output();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn fixture() -> (AudioUi, Receiver<Command>, SyncSender<Event>) {
        let (commands, requests) = mpsc::sync_channel(4);
        let (sender, events) = mpsc::sync_channel(8);
        (
            AudioUi {
                commands,
                events,
                runtime: None,
                devices: vec![],
                inputs: vec![],
                applications: vec![],
                applications_loaded: false,
                applications_error: None,
                busy: false,
                error: None,
                snapshot: None,
                updating: false,
                pending_update: None,
                wanted_version: 0,
                cancelled: false,
                starting: false,
                pending_operations: 0,
                cancellation: StartCancellation::default(),
            },
            requests,
            sender,
        )
    }

    #[test]
    fn stop_cancels_start_while_it_is_still_in_the_ui_queue() {
        let (mut ui, requests, events) = fixture();
        ui.send(Request::Start(
            Session::new("Queued", 48000),
            OutputPlan::single(relink_audio::OutputId("unused".into()), Uuid::new_v4()),
        ));
        assert!(ui.can_stop());
        assert!(ui.busy);
        ui.send(Request::Stop);
        let start = requests.recv().unwrap();
        assert!(matches!(start.request, Request::Start(..)));
        assert!(start.cancellation.is_cancelled());
        assert!(!ui.can_stop());
        assert!(matches!(requests.recv().unwrap().request, Request::Stop));
        events
            .send(Event::Finished {
                operation: Operation::Start,
                result: Err(AudioError::Cancelled),
            })
            .unwrap();
        ui.poll();
        assert!(ui.busy, "Stop is still waiting for resource cleanup");
        assert!(
            ui.error.is_none(),
            "Intentional cancellation is not a failure"
        );
        events
            .send(Event::Finished {
                operation: Operation::Stop,
                result: Ok(None),
            })
            .unwrap();
        ui.poll();
        assert!(!ui.busy);
        assert!(!ui.can_stop());
    }

    #[test]
    fn dropping_ui_cancels_an_undispatched_start() {
        let (mut ui, requests, _events) = fixture();
        ui.send(Request::Start(
            Session::new("Queued", 48000),
            OutputPlan::single(relink_audio::OutputId("unused".into()), Uuid::new_v4()),
        ));
        drop(ui);
        assert!(requests.recv().unwrap().cancellation.is_cancelled());
    }

    fn snapshot(running: bool, fault: relink_audio::AudioFault) -> AudioSnapshot {
        AudioSnapshot {
            outputs: vec![],
            running,
            configuration: None,
            rendered_frames: 0,
            callback_count: 0,
            applied_control_version: 0,
            captures: vec![],
            output_timing: None,
            input_meters: vec![],
            bus_meters: vec![],
            fault,
            first_fault: None,
            error: None,
        }
    }

    #[test]
    fn asynchronous_cleanup_fault_remains_visible_after_stop() {
        let (mut ui, _requests, events) = fixture();
        ui.send(Request::Stop);
        events
            .send(Event::Finished {
                operation: Operation::Stop,
                result: Ok(None),
            })
            .unwrap();
        ui.poll();
        assert!(ui.cancelled);
        assert!(ui.error.is_none());
        events
            .send(Event::Snapshot(Box::new(snapshot(
                false,
                relink_audio::AudioFault::StreamInvalidated,
            ))))
            .unwrap();
        ui.poll();
        assert!(ui.error.is_some());
        assert!(!ui.running());
    }

    #[test]
    fn healthy_restart_clears_queued_old_fault_but_faulted_start_does_not() {
        use relink_audio::AudioFault;
        for fault in [AudioFault::None, AudioFault::StreamInvalidated] {
            let (mut ui, _requests, events) = fixture();
            ui.send(Request::Start(
                Session::new("Restart", 48000),
                OutputPlan::single(relink_audio::OutputId("unused".into()), Uuid::new_v4()),
            ));
            events
                .send(Event::Snapshot(Box::new(snapshot(false, AudioFault::Xrun))))
                .unwrap();
            events
                .send(Event::Snapshot(Box::new(snapshot(true, fault))))
                .unwrap();
            events
                .send(Event::Finished {
                    operation: Operation::Start,
                    result: Ok(None),
                })
                .unwrap();
            ui.poll();
            assert_eq!(ui.error.is_none(), fault == AudioFault::None);
        }
    }

    #[test]
    fn stopping_start_does_not_suppress_non_cancellation_errors() {
        let (mut ui, _requests, events) = fixture();
        ui.send(Request::Stop);
        events
            .send(Event::Finished {
                operation: Operation::Start,
                result: Err(AudioError::RuntimeUnavailable),
            })
            .unwrap();
        ui.poll();
        assert!(ui.error.is_some());
    }

    #[test]
    fn application_catalog_failure_preserves_devices_and_does_not_start_or_cancel_audio() {
        let (mut ui, requests, events) = fixture();
        ui.inputs.push(Device {
            id: "input".into(),
            name: "Microphone".into(),
        });
        ui.devices.push(Device {
            id: "output".into(),
            name: "Speakers".into(),
        });
        ui.error = Some("Existing output error".into());
        ui.send(Request::RefreshApplications);
        assert!(matches!(
            requests.recv().unwrap().request,
            Request::RefreshApplications
        ));
        assert!(ui.busy);
        assert_eq!(ui.error.as_deref(), Some("Existing output error"));
        events
            .send(Event::Finished {
                operation: Operation::RefreshApplications,
                result: Err(AudioError::UnsupportedPlatform),
            })
            .unwrap();
        ui.poll();
        assert!(!ui.busy && !ui.starting && !ui.cancelled);
        assert!(ui.applications_loaded && ui.applications_error.is_some());
        assert_eq!(ui.inputs[0].name, "Microphone");
        assert_eq!(ui.devices[0].name, "Speakers");
        assert_eq!(ui.error.as_deref(), Some("Existing output error"));
        assert!(requests.try_recv().is_err());
        ui.send(Request::Refresh);
        assert!(matches!(requests.recv().unwrap().request, Request::Refresh));
    }

    #[test]
    fn application_catalog_retry_is_independent_and_keeps_stable_selector_ids() {
        let (mut ui, requests, events) = fixture();
        let id = relink_core::ApplicationSelector::MacBundle {
            bundle_id: "com.example.Player".into(),
            path: "/Applications/Player.app".into(),
        }
        .encode()
        .unwrap();
        ui.applications_error = Some("Previous catalog failure".into());
        ui.send(Request::RefreshApplications);
        assert!(matches!(
            requests.recv().unwrap().request,
            Request::RefreshApplications
        ));
        events
            .send(Event::Applications(vec![Device {
                id: id.clone(),
                name: "Player".into(),
            }]))
            .unwrap();
        events
            .send(Event::Finished {
                operation: Operation::RefreshApplications,
                result: Ok(None),
            })
            .unwrap();
        ui.poll();
        assert_eq!(ui.applications[0].id, id);
        assert!(ui.applications_loaded && ui.applications_error.is_none());
        assert!(!ui.busy && !ui.can_stop());
        assert!(requests.try_recv().is_err());
    }
}
