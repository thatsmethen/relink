#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod audio_ui;
mod effects;
mod routing;
mod store;

use audio_ui::{AudioUi, Request};
use eframe::egui::{self, Color32, RichText, Vec2};
use relink_core::{ApplicationSelector, Bus, ChannelControls, Input, InputKind, Send, Session};
use routing::{OutputDraft, RoutingDraft};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};
use uuid::Uuid;

const BACKGROUND: Color32 = Color32::from_rgb(17, 20, 26);
const PANEL: Color32 = Color32::from_rgb(24, 28, 36);
const CARD: Color32 = Color32::from_rgb(32, 37, 47);
const ACCENT: Color32 = Color32::from_rgb(112, 222, 199);
const MUTED: Color32 = Color32::from_rgb(149, 161, 179);
const STRIP_WIDTH: f32 = 184.0;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 600.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!(
                    "../../../packaging/branding/relink-256.png"
                ))
                .expect("bundled ReLink icon"),
            ),
        ..Default::default()
    };
    eframe::run_native(
        "ReLink",
        options,
        Box::new(|cc| Ok(Box::new(Studio::new(cc)))),
    )
}

fn starter() -> Session {
    let mut session = Session::new("My studio", 48_000);
    session.buses = ["Monitor", "Stream", "Voice chat"]
        .into_iter()
        .map(Bus::new)
        .collect();
    session
}

#[derive(Clone)]
enum Edit {
    AddInput,
    AddDevice(String, String),
    AddApplication(String, String),
    AddBus,
    RemoveInput(Uuid),
    RemoveBus(Uuid),
    RenameInput(Uuid, String),
    RenameBus(Uuid, String),
    InputControls(Uuid, ChannelControls),
    BusControls(Uuid, ChannelControls),
    Route(Uuid, Uuid, Option<ChannelControls>),
    InputDsp(Uuid, relink_core::DspConfig),
    BusDsp(Uuid, relink_core::DspConfig),
    Tone(Uuid, f32, f32),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SourceKind {
    #[default]
    Device,
    Application,
}

struct Studio {
    session: Session,
    path: Option<PathBuf>,
    selected_bus: Option<Uuid>,
    selected_input: Option<Uuid>,
    route_index: HashMap<(Uuid, Uuid), usize>,
    undo: Vec<store::StudioDocument>,
    redo: Vec<store::StudioDocument>,
    dirty_since: Option<Instant>,
    notice: String,
    save_error: Option<String>,
    preserve_original_before_save: bool,
    audio: AudioUi,
    search: String,
    output_routing: RoutingDraft,
    outputs_open: bool,
    source_picker_open: bool,
    source_device: Option<String>,
    source_kind: SourceKind,
    source_application: Option<String>,
}

impl Studio {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = PANEL;
        style.visuals.window_fill = PANEL;
        style.visuals.extreme_bg_color = BACKGROUND;
        style.visuals.selection.bg_fill = Color32::from_rgb(41, 88, 83);
        style.visuals.selection.stroke.color = ACCENT;
        style.spacing.item_spacing = Vec2::new(10.0, 10.0);
        style.spacing.button_padding = Vec2::new(12.0, 8.0);
        cc.egui_ctx.set_style(style);
        let (path, path_error) = match store::default_path() {
            Ok(path) => (Some(path), None),
            Err(error) => (None, Some(error)),
        };
        let mut notice = "Add sources, shape your mixes, then choose Outputs.".to_owned();
        let mut loaded = false;
        let mut preserve_original_before_save = false;
        let document = match path.as_ref().map(|p| store::load(p)) {
            Some(Ok(Some(s))) => {
                loaded = true;
                s
            }
            Some(Err(error)) => {
                notice = format!(
                    "{error} Saving this recovered studio will first preserve the original in a separate backup."
                );
                preserve_original_before_save = true;
                store::StudioDocument::new(starter(), Default::default())
            }
            _ => store::StudioDocument::new(starter(), Default::default()),
        };
        let mut app = Self {
            selected_bus: document.session.buses.first().map(|b| b.id),
            session: document.session,
            path,
            selected_input: None,
            route_index: HashMap::new(),
            undo: vec![],
            redo: vec![],
            dirty_since: (!loaded && !preserve_original_before_save).then(Instant::now),
            notice,
            save_error: path_error,
            preserve_original_before_save,
            audio: AudioUi::new(cc.egui_ctx.clone()),
            search: String::new(),
            output_routing: document.output_routing,
            outputs_open: false,
            source_picker_open: false,
            source_device: None,
            source_kind: SourceKind::Device,
            source_application: None,
        };
        app.reindex();
        app
    }

    fn reindex(&mut self) {
        self.route_index = self
            .session
            .sends
            .iter()
            .enumerate()
            .map(|(i, s)| ((s.input_id, s.bus_id), i))
            .collect();
        if !self
            .session
            .buses
            .iter()
            .any(|b| Some(b.id) == self.selected_bus)
        {
            self.selected_bus = self.session.buses.first().map(|b| b.id);
        }
        if !self
            .session
            .inputs
            .iter()
            .any(|i| Some(i.id) == self.selected_input)
        {
            self.selected_input = None;
        }
    }

    fn edit(&mut self, edit: Edit) {
        if self.audio.busy {
            self.notice = "Wait for the audio device operation to finish before editing.".into();
            return;
        }
        let live_edit = match &edit {
            Edit::RenameInput(..)
            | Edit::RenameBus(..)
            | Edit::InputControls(..)
            | Edit::BusControls(..) => true,
            Edit::Route(input, bus, Some(_)) => self.route_index.contains_key(&(*input, *bus)),
            _ => false,
        };
        if self.audio.running() && !live_edit {
            self.notice = "Stop audio before changing sources, effects or routing.".into();
            return;
        }
        let previous = self.session.clone();
        match edit {
            Edit::AddInput => {
                let input = Input::new(
                    format!("Test tone {}", self.session.inputs.len() + 1),
                    InputKind::TestTone {
                        frequency_hz: 440.0,
                        amplitude_db: -24.0,
                    },
                );
                self.selected_input = Some(input.id);
                self.session.inputs.push(input);
            }
            Edit::AddDevice(device_id, name) => {
                let input = Input::new(name, InputKind::Hardware { device_id });
                self.selected_input = Some(input.id);
                self.session.inputs.push(input);
            }
            Edit::AddApplication(application_id, name) => {
                if let Err(error) = ApplicationSelector::decode(&application_id) {
                    self.notice = error.to_string();
                    return;
                }
                let input = Input::new(name, InputKind::Application { application_id });
                self.selected_input = Some(input.id);
                self.session.inputs.push(input);
            }
            Edit::AddBus => {
                let bus = Bus::new(format!("Mix {}", self.session.buses.len() + 1));
                self.selected_bus = Some(bus.id);
                self.session.buses.push(bus);
            }
            Edit::RemoveInput(id) => {
                let _ = self.session.remove_input(id);
            }
            Edit::RemoveBus(id) => {
                let _ = self.session.remove_bus(id);
            }
            Edit::RenameInput(id, name) => {
                if let Some(i) = self.session.inputs.iter_mut().find(|i| i.id == id) {
                    i.name = name;
                }
            }
            Edit::RenameBus(id, name) => {
                if let Some(b) = self.session.buses.iter_mut().find(|b| b.id == id) {
                    b.name = name;
                }
            }
            Edit::InputControls(id, controls) => {
                if let Some(i) = self.session.inputs.iter_mut().find(|i| i.id == id) {
                    i.controls = controls;
                }
            }
            Edit::BusControls(id, controls) => {
                if let Some(b) = self.session.buses.iter_mut().find(|b| b.id == id) {
                    b.controls = controls;
                }
            }
            Edit::InputDsp(id, dsp) => {
                if let Some(i) = self.session.inputs.iter_mut().find(|i| i.id == id) {
                    i.dsp = dsp;
                }
            }
            Edit::BusDsp(id, dsp) => {
                if let Some(bus) = self.session.buses.iter_mut().find(|bus| bus.id == id) {
                    bus.dsp = dsp;
                }
            }
            Edit::Tone(id, frequency_hz, amplitude_db) => {
                if let Some(i) = self.session.inputs.iter_mut().find(|i| i.id == id) {
                    i.kind = InputKind::TestTone {
                        frequency_hz,
                        amplitude_db,
                    };
                }
            }
            Edit::Route(input_id, bus_id, controls) => {
                if let Some(index) = self
                    .session
                    .sends
                    .iter()
                    .position(|s| s.input_id == input_id && s.bus_id == bus_id)
                {
                    if let Some(controls) = controls {
                        self.session.sends[index].controls = controls;
                    } else {
                        self.session.sends.remove(index);
                    }
                } else if let Some(controls) = controls {
                    let mut send = Send::new(input_id, bus_id);
                    send.controls = controls;
                    self.session.sends.push(send);
                }
            }
        }
        if let Err(error) = self.session.validate() {
            self.session = previous;
            self.notice = error.to_string();
            return;
        }
        if previous == self.session {
            return;
        }
        self.undo.push(store::StudioDocument::new(
            previous,
            self.output_routing.clone(),
        ));
        if self.undo.len() > 32 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty_since = Some(Instant::now());
        self.reindex();
        if self.audio.running() {
            self.audio.queue_controls(self.session.clone());
        }
    }

    fn save(&mut self) {
        match self
            .path
            .as_ref()
            .ok_or_else(|| "Cannot locate a studio folder.".to_string())
            .and_then(|p| {
                if self.preserve_original_before_save {
                    store::save_recovering(p, &self.document()).map(Some)
                } else {
                    store::save(p, &self.document()).map(|()| None)
                }
            }) {
            Ok(backup) => {
                if let Some(backup) = backup {
                    self.preserve_original_before_save = false;
                    self.notice = format!(
                        "Recovered studio saved. Original preserved at {}",
                        backup.display()
                    );
                }
                self.dirty_since = None;
                self.save_error = None;
            }
            Err(error) => {
                self.save_error = Some(error);
            }
        }
    }

    fn document(&self) -> store::StudioDocument {
        store::StudioDocument::new(self.session.clone(), self.output_routing.clone())
    }

    fn history(&mut self, redo: bool) {
        let stack = if redo { &mut self.redo } else { &mut self.undo };
        if let Some(document) = stack.pop() {
            let previous = self.document();
            self.session = document.session;
            self.output_routing = document.output_routing;
            if redo {
                self.undo.push(previous);
            } else {
                self.redo.push(previous);
            }
            self.dirty_since = Some(Instant::now());
            self.reindex();
        }
    }

    fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar")
            .exact_height(76.0)
            .show(ctx, |ui| {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(RichText::new("RE / LINK").strong().size(21.0).color(ACCENT));
                    ui.separator();
                    ui.label(&self.session.name);
                    ui.label(
                        RichText::new(if self.path.is_none() {
                            "No save location"
                        } else if self.save_error.is_some() {
                            "Save needs attention"
                        } else if self.preserve_original_before_save {
                            "Recovery not saved"
                        } else if self.dirty_since.is_some() {
                            "Unsaved changes"
                        } else {
                            "Saved locally"
                        })
                        .size(11.0)
                        .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(12.0);
                        let running = self.audio.running();
                        let can_stop = self.audio.can_stop();
                        if ui
                            .add_enabled(
                                can_stop || !self.audio.busy,
                                egui::Button::new(if can_stop {
                                    "Stop audio"
                                } else {
                                    "Start audio"
                                })
                                .fill(if can_stop {
                                    Color32::from_rgb(110, 42, 54)
                                } else {
                                    Color32::from_rgb(36, 96, 83)
                                }),
                            )
                            .clicked()
                        {
                            if can_stop {
                                self.audio.send(Request::Stop);
                            } else {
                                match self.output_routing.plan(&self.session) {
                                    Ok(plan) => {
                                        self.audio.send(Request::Start(self.session.clone(), plan))
                                    }
                                    Err(error) => {
                                        self.notice = error;
                                        self.outputs_open = true;
                                    }
                                }
                            }
                        }
                        if ui
                            .button(format!("Outputs ({})", self.output_routing.outputs.len()))
                            .clicked()
                        {
                            self.outputs_open = true;
                        }
                        if ui.button("Save").clicked() {
                            self.save();
                        }
                        ui.add_enabled_ui(!running && !self.audio.busy, |ui| {
                            if ui
                                .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                                .clicked()
                            {
                                self.history(true);
                            }
                            if ui
                                .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                                .clicked()
                            {
                                self.history(false);
                            }
                            if ui.button("+ Source").clicked() {
                                self.source_picker_open = true;
                            }
                        });
                    });
                });
            });
    }

    fn output_picker(&mut self, ctx: &egui::Context) {
        if !self.outputs_open {
            return;
        }
        let mut open = self.outputs_open;
        let before = self.output_routing.clone();
        let locked = self.audio.running() || self.audio.busy;
        egui::Window::new("Outputs")
            .open(&mut open)
            .default_width(620.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label("Send each mix to one or more devices. Choose a clock device; the other outputs follow it.");
                if locked {
                    ui.label("Stop audio to change output routing.");
                }
                let mut remove = None;
                let selected_devices: HashSet<_> = self.output_routing.outputs.iter()
                    .filter_map(|route| route.device_id.clone()).collect();
                let row_height = if self.audio.running() { 260.0 } else { 180.0 };
                egui::ScrollArea::vertical().max_height(440.0).show_rows(ui, row_height, self.output_routing.outputs.len(), |ui, rows| {
                    for index in rows {
                        let route = &mut self.output_routing.outputs[index];
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_height), egui::Sense::hover());
                        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                        ui.set_clip_rect(ui.clip_rect().intersect(rect));
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.strong(format!("Output {}", index + 1));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add_enabled(!locked, egui::Button::new("Remove").small()).clicked() {
                                        remove = Some(index);
                                    }
                                });
                            });
                            ui.add_enabled_ui(!locked, |ui| {
                                let old_device = route.device_id.clone();
                                let device_label = route.device_id.as_ref().map(|id| {
                                    self.audio.devices.iter().find(|device| &device.id == id)
                                        .map_or_else(|| format!("Unavailable: {id}"), |device| device.name.clone())
                                }).unwrap_or_else(|| "Select a device…".into());
                                ui.label("Device");
                                egui::ComboBox::from_id_salt(("output-device", index))
                                    .selected_text(device_label).width(480.0).show_ui(ui, |ui| {
                                        ui.selectable_value(&mut route.device_id, None, "Select a device…");
                                        for device in &self.audio.devices {
                                            let already_used = selected_devices.contains(&device.id)
                                                && route.device_id.as_ref() != Some(&device.id);
                                            ui.add_enabled_ui(!already_used, |ui| {
                                                ui.selectable_value(&mut route.device_id, Some(device.id.clone()), &device.name);
                                            });
                                        }
                                    });
                                if old_device != route.device_id && old_device.is_some()
                                    && self.output_routing.clock_output == old_device {
                                    self.output_routing.clock_output = None;
                                }
                                ui.horizontal(|ui| {
                                    ui.label("Mix");
                                    let mix_label = route.bus_id.map(|id| {
                                        self.session.buses.iter().find(|bus| bus.id == id)
                                            .map_or_else(|| "Mix removed — choose another".into(), |bus| bus.name.clone())
                                    }).unwrap_or_else(|| "Select a mix…".into());
                                    egui::ComboBox::from_id_salt(("output-mix", index))
                                        .selected_text(mix_label).width(230.0).show_ui(ui, |ui| {
                                            for bus in &self.session.buses {
                                                ui.selectable_value(&mut route.bus_id, Some(bus.id), &bus.name);
                                            }
                                        });
                                    if ui.add_enabled(route.device_id.is_some(), egui::RadioButton::new(
                                        route.device_id.is_some() && route.device_id == self.output_routing.clock_output,
                                        "Clock device",
                                    )).clicked() {
                                        self.output_routing.clock_output = route.device_id.clone();
                                    }
                                });
                            });
                            if self.audio.running() {
                                if let Some(state) = self.audio.snapshot().and_then(|snapshot| snapshot.outputs.iter().find(|state| {
                                    Some(&state.output.0) == route.device_id.as_ref() && Some(state.bus_id) == route.bus_id
                                })) {
                                    let status = if state.ready { "Active" } else { "Waiting for audio…" };
                                    ui.label(RichText::new(status).color(if state.ready { ACCENT } else { MUTED }));
                                    let submitted = state.submitted_control_version.map_or_else(|| "Waiting".into(), |v| format!("v{v}"));
                                    let rendered = state.rendered_control_version.map_or_else(|| "Waiting".into(), |v| format!("v{v}"));
                                    ui.label(format!("Controls prepared: {rendered} · submitted: {submitted}"));
                                    if state.submitted_control_version.is_none_or(|v| v < self.audio.wanted_version()) {
                                        ui.label("Waiting for this output to receive the latest controls.");
                                    }
                                }
                            }
                        });
                        });
                    }
                });
                if let Some(index) = remove {
                    self.output_routing.remove(index);
                }
                ui.add_enabled_ui(!locked, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("+ Add output").clicked() {
                            self.output_routing.outputs.push(OutputDraft {
                                device_id: None,
                                bus_id: self.selected_bus.or_else(|| self.session.buses.first().map(|bus| bus.id)),
                            });
                        }
                        if ui.button("Refresh devices").clicked() {
                            self.audio.send(Request::Refresh);
                        }
                    });
                    if self.audio.devices.is_empty() {
                        ui.label("No output devices found. Connect a device, then refresh.");
                    }
                    if let Some(error) = self.output_routing.issue(&self.session) {
                        ui.colored_label(Color32::LIGHT_RED, error);
                    } else {
                        for route in &self.output_routing.outputs {
                            if route.device_id.as_ref().is_some_and(|id| !self.audio.devices.iter().any(|device| &device.id == id)) {
                                ui.colored_label(Color32::LIGHT_RED, "A selected device is unavailable. Reconnect it or choose another device.");
                                break;
                            }
                        }
                    }
                });
                if let Some(error) = &self.audio.error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }
            });
        self.outputs_open = open;
        if before != self.output_routing {
            self.undo
                .push(store::StudioDocument::new(self.session.clone(), before));
            if self.undo.len() > 32 {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.dirty_since = Some(Instant::now());
        }
    }

    fn source_picker(&mut self, ctx: &egui::Context, edits: &mut Vec<Edit>) {
        if !self.source_picker_open {
            return;
        }
        let mut open = self.source_picker_open;
        let mut added = false;
        let locked = self.audio.running() || self.audio.busy;
        egui::Window::new("Add a source").open(&mut open).collapsible(false).resizable(false).default_width(420.0).show(ctx, |ui| {
            if locked {
                ui.label(if self.audio.running() { "Stop audio before changing sources." } else { "Refreshing sources…" });
            }
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.source_kind, SourceKind::Device, "Audio device");
                    if ui.selectable_value(&mut self.source_kind, SourceKind::Application, "Application").clicked()
                        && !self.audio.applications_loaded {
                        self.audio.send(Request::RefreshApplications);
                    }
                });
                ui.separator();
                match self.source_kind {
                    SourceKind::Device => {
                        ui.label("Choose a microphone, interface or virtual audio device.");
                        let selected = self.audio.inputs.iter().find(|d| Some(&d.id) == self.source_device.as_ref());
                        egui::ComboBox::from_id_salt("source-device").selected_text(selected.map_or("Select an input…", |d| d.name.as_str())).width(380.0).show_ui(ui, |ui| {
                            for device in &self.audio.inputs {
                                ui.selectable_value(&mut self.source_device, Some(device.id.clone()), &device.name);
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Refresh inputs").clicked() { self.audio.send(Request::Refresh); }
                            let selected = self.audio.inputs.iter().find(|d| Some(&d.id) == self.source_device.as_ref());
                            if ui.add_enabled(selected.is_some() && !self.audio.busy, egui::Button::new("Add device")).clicked() {
                                if let Some(device) = selected { edits.push(Edit::AddDevice(device.id.clone(), device.name.clone())); added = true; }
                            }
                        });
                        ui.label("Mono inputs use both stereo channels; interfaces use channels 1–2.");
                        if let Some(error) = &self.audio.error { ui.colored_label(Color32::LIGHT_RED, error); }
                    }
                    SourceKind::Application => {
                        ui.label(RichText::new("Application capture · early preview").color(MUTED));
                        ui.label("Choose a running application to add to your studio.");
                        let selected = self.audio.applications.iter().find(|app| Some(&app.id) == self.source_application.as_ref());
                        egui::ComboBox::from_id_salt("source-application").selected_text(selected.map_or("Select an application…", |app| app.name.as_str())).width(380.0).show_ui(ui, |ui| {
                            for application in &self.audio.applications {
                                ui.selectable_value(&mut self.source_application, Some(application.id.clone()), &application.name).on_hover_ui(|ui| {
                                    if let Ok(selector) = ApplicationSelector::decode(&application.id) {
                                        let path = match selector { ApplicationSelector::MacBundle { path, .. } | ApplicationSelector::WindowsExecutable { path } => path };
                                        ui.label(path);
                                    }
                                });
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!self.audio.busy, egui::Button::new("Refresh applications")).clicked() { self.audio.send(Request::RefreshApplications); }
                            let selected = self.audio.applications.iter().find(|app| Some(&app.id) == self.source_application.as_ref());
                            if ui.add_enabled(selected.is_some() && !self.audio.busy && self.audio.applications_error.is_none(), egui::Button::new("Add application")).clicked() {
                                if let Some(application) = selected { edits.push(Edit::AddApplication(application.id.clone(), application.name.clone())); added = true; }
                            }
                        });
                        if let Some(error) = &self.audio.applications_error {
                            ui.colored_label(Color32::LIGHT_RED, error);
                        } else if self.audio.applications_loaded && self.audio.applications.is_empty() {
                            ui.label("No applications found. Open an app, start its audio, then refresh.");
                        }
                        ui.label("Start audio in the app before starting capture in ReLink.");
                        if cfg!(target_os = "macos") {
                            ui.label("Starting capture may ask for macOS audio-capture permission.");
                        }
                        ui.label("If the app restarts or adds processes, Stop and Start ReLink audio again.");
                    }
                }
                ui.label("Capture begins when you start audio.");
                ui.separator();
                if ui.add_enabled(!self.audio.busy, egui::Button::new("Add a test tone")).clicked() { edits.push(Edit::AddInput); added = true; }
            });
        });
        self.source_picker_open = open && !added;
    }

    fn sidebar(&mut self, ctx: &egui::Context, edits: &mut Vec<Edit>) {
        egui::SidePanel::left("mixes")
            .exact_width(184.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(18.0);
                ui.label(RichText::new("YOUR MIXES").size(11.0).color(MUTED));
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .max_height((ui.available_height() - 180.0).max(100.0))
                    .show_rows(ui, 44.0, self.session.buses.len(), |ui, rows| {
                        for index in rows {
                            let bus = &self.session.buses[index];
                            if ui
                                .add_sized(
                                    [160.0, 38.0],
                                    egui::Button::new(&bus.name)
                                        .selected(self.selected_bus == Some(bus.id)),
                                )
                                .clicked()
                            {
                                self.selected_bus = Some(bus.id);
                            }
                        }
                    });
                ui.add_enabled_ui(!self.audio.running() && !self.audio.busy, |ui| {
                    if ui.button("+ Create mix").clicked() {
                        edits.push(Edit::AddBus);
                    }
                });
                ui.add_space(24.0);
                ui.separator();
                ui.label(RichText::new("STUDIO").size(11.0).color(MUTED));
                ui.label(format!("{} sources", self.session.inputs.len()));
                ui.label(format!("{} mixes", self.session.buses.len()));
                ui.label(format!("{} active routes", self.session.sends.len()));
                ui.add_space(16.0);
                ui.label(
                    RichText::new("Rust development build\nWindows + macOS")
                        .size(11.0)
                        .color(MUTED),
                );
            });
    }

    fn inspector(&mut self, ctx: &egui::Context, edits: &mut Vec<Edit>) {
        egui::SidePanel::right("inspector")
            .exact_width(254.0)
            .resizable(false)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(18.0);
                    ui.heading("Mix output");
                    if let Some(bus) = self
                        .session
                        .buses
                        .iter()
                        .find(|b| Some(b.id) == self.selected_bus)
                    {
                        ui.add_enabled_ui(!self.audio.busy, |ui| {
                            let mut name = bus.name.clone();
                            if ui.text_edit_singleline(&mut name).changed()
                                && !name.trim().is_empty()
                                && name != bus.name
                            {
                                edits.push(Edit::RenameBus(bus.id, name));
                            }
                            let mut controls = bus.controls.clone();
                            gain_controls(ui, &mut controls);
                            if controls != bus.controls {
                                edits.push(Edit::BusControls(bus.id, controls));
                            }
                        });
                        egui::CollapsingHeader::new("Mix effects")
                            .id_salt(("mix-effects", bus.id))
                            .show(ui, |ui| {
                                if self.audio.running() {
                                    ui.label("Stop audio to edit effects.");
                                }
                                ui.add_enabled_ui(
                                    !self.audio.running() && !self.audio.busy,
                                    |ui| {
                                        let mut dsp = bus.dsp.clone();
                                        effects::editor(ui, &mut dsp, self.session.sample_rate);
                                        if dsp != bus.dsp {
                                            edits.push(Edit::BusDsp(bus.id, dsp));
                                        }
                                    },
                                );
                            });
                        ui.add_enabled_ui(!self.audio.running() && !self.audio.busy, |ui| {
                            ui.add_space(8.0);
                            ui.label("Devices receiving this mix");
                            let mut assigned = false;
                            for route in &self.output_routing.outputs {
                                if route.bus_id == Some(bus.id) {
                                    assigned = true;
                                    let name = route
                                        .device_id
                                        .as_ref()
                                        .map(|id| {
                                            self.audio
                                                .devices
                                                .iter()
                                                .find(|d| &d.id == id)
                                                .map_or_else(
                                                    || format!("Unavailable: {id}"),
                                                    |d| d.name.clone(),
                                                )
                                        })
                                        .unwrap_or_else(|| "Choose a device in Outputs".into());
                                    ui.label(name);
                                }
                            }
                            if !assigned {
                                ui.label("No output assigned");
                            }
                            if ui.button("Configure outputs…").clicked() {
                                self.outputs_open = true;
                            }
                            if ui.small_button("Remove mix").clicked() {
                                edits.push(Edit::RemoveBus(bus.id));
                            }
                        });
                    }
                    ui.add_space(12.0);
                    meter(ui, "MIX LEVEL", self.audio.bus_peak(self.selected_bus));
                    ui.separator();
                    if let Some(input) = self
                        .session
                        .inputs
                        .iter()
                        .find(|i| Some(i.id) == self.selected_input)
                    {
                        ui.heading("Source settings");
                        ui.label(&input.name);
                        ui.add_enabled_ui(!self.audio.busy, |ui| {
                            let mut name = input.name.clone();
                            if ui.text_edit_singleline(&mut name).changed()
                                && !name.trim().is_empty()
                                && name != input.name
                            {
                                edits.push(Edit::RenameInput(input.id, name));
                            }
                            let mut controls = input.controls.clone();
                            gain_controls(ui, &mut controls);
                            if controls != input.controls {
                                edits.push(Edit::InputControls(input.id, controls));
                            }
                        });
                        ui.add_enabled_ui(!self.audio.running() && !self.audio.busy, |ui| {
                            if let InputKind::TestTone {
                                frequency_hz,
                                amplitude_db,
                            } = input.kind
                            {
                                let (mut freq, mut amp) = (frequency_hz, amplitude_db);
                                ui.add(
                                    egui::Slider::new(&mut freq, 20.0..=20_000.0)
                                        .logarithmic(true)
                                        .text("Hz"),
                                );
                                ui.add(egui::Slider::new(&mut amp, -60.0..=-6.0).text("dB tone"));
                                if freq != frequency_hz || amp != amplitude_db {
                                    edits.push(Edit::Tone(input.id, freq, amp));
                                }
                            }
                            ui.separator();
                            ui.label(RichText::new("EFFECTS · ALL MIXES").size(11.0).color(MUTED));
                            let mut dsp = input.dsp.clone();
                            ui.push_id(("input-effects", input.id), |ui| {
                                effects::editor(ui, &mut dsp, self.session.sample_rate);
                            });
                            if dsp != input.dsp {
                                edits.push(Edit::InputDsp(input.id, dsp));
                            }
                            ui.add_space(10.0);
                            if ui.button("Remove source").clicked() {
                                edits.push(Edit::RemoveInput(input.id));
                            }
                        });
                    } else {
                        ui.label(
                            RichText::new("Select a source to adjust its input level and effects.")
                                .color(MUTED),
                        );
                    }
                });
            });
    }

    fn mixer(&mut self, ctx: &egui::Context, edits: &mut Vec<Edit>) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(22.0))
            .show(ctx, |ui| {
                let bus_id = match self.selected_bus {
                    Some(id) => id,
                    None => {
                        ui.heading("Create your first mix");
                        ui.label("Each mix has independent source levels and an output.");
                        return;
                    }
                };
                let name = self
                    .session
                    .buses
                    .iter()
                    .find(|b| b.id == bus_id)
                    .map(|b| b.name.as_str())
                    .unwrap_or("Mix");
                ui.horizontal(|ui| {
                    ui.heading(RichText::new(name).size(29.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .hint_text("Find a source")
                                .desired_width(170.0),
                        );
                    });
                });
                ui.label(
                    RichText::new("Shape this mix with independent source levels.").color(MUTED),
                );
                ui.add_space(24.0);
                if self.session.inputs.is_empty() {
                    ui.add_space(70.0);
                    ui.heading("Build your first mix");
                    ui.label("Add a source, include it in this mix, then choose where to listen.");
                    if ui
                        .add_enabled(
                            !self.audio.running() && !self.audio.busy,
                            egui::Button::new("+ Add a source"),
                        )
                        .clicked()
                    {
                        self.source_picker_open = true;
                    }
                    return;
                }
                let needle = self.search.to_lowercase();
                let inputs: Vec<_> = self
                    .session
                    .inputs
                    .iter()
                    .filter(|i| needle.is_empty() || i.name.to_lowercase().contains(&needle))
                    .collect();
                let count = inputs.len();
                let height = (ui.available_height() - 30.0).max(340.0);
                egui::ScrollArea::horizontal()
                    .id_salt("source-strips")
                    .show_viewport(ui, |ui, viewport| {
                        let width = count as f32 * (STRIP_WIDTH + 12.0);
                        let origin = ui.min_rect().min;
                        ui.set_min_size(Vec2::new(width, height));
                        let start =
                            (viewport.left() / (STRIP_WIDTH + 12.0)).floor().max(0.0) as usize;
                        let end = ((viewport.right() / (STRIP_WIDTH + 12.0)).ceil() as usize + 1)
                            .min(count);
                        for (index, input) in inputs.iter().enumerate().take(end).skip(start) {
                            let rect = egui::Rect::from_min_size(
                                origin + Vec2::new(index as f32 * (STRIP_WIDTH + 12.0), 0.0),
                                Vec2::new(STRIP_WIDTH, height),
                            );
                            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                                egui::Frame::new()
                                    .fill(CARD)
                                    .corner_radius(12.0)
                                    .inner_margin(14.0)
                                    .show(ui, |ui| {
                                        ui.set_min_size(Vec2::new(
                                            STRIP_WIDTH - 28.0,
                                            height - 28.0,
                                        ));
                                        ui.set_max_width(STRIP_WIDTH - 28.0);
                                        ui.spacing_mut().slider_width = 88.0;
                                        let selected = self.selected_input == Some(input.id);
                                        if ui
                                            .add_sized(
                                                [STRIP_WIDTH - 28.0, 40.0],
                                                egui::Button::new(
                                                    RichText::new(&input.name).strong(),
                                                )
                                                .selected(selected),
                                            )
                                            .clicked()
                                        {
                                            self.selected_input = Some(input.id);
                                        }
                                        ui.label(
                                            RichText::new(match input.kind {
                                                InputKind::TestTone { .. } => "TEST TONE",
                                                InputKind::Hardware { .. } => "DEVICE",
                                                InputKind::Application { .. } => "APPLICATION",
                                                InputKind::Virtual { .. } => "VIRTUAL DEVICE",
                                            })
                                            .size(10.0)
                                            .color(MUTED),
                                        );
                                        ui.add_space(16.0);
                                        ui.add_enabled_ui(!self.audio.busy, |ui| {
                                            let route = self
                                                .route_index
                                                .get(&(input.id, bus_id))
                                                .map(|&idx| &self.session.sends[idx]);
                                            if let Some(route) = route {
                                                let mut controls = route.controls.clone();
                                                ui.horizontal(|ui| {
                                                    if ui
                                                        .selectable_label(controls.muted, "Mute")
                                                        .on_hover_text(
                                                            "Mute this source in this mix",
                                                        )
                                                        .clicked()
                                                    {
                                                        controls.muted = !controls.muted;
                                                    }
                                                    if ui
                                                        .selectable_label(controls.solo, "Solo")
                                                        .clicked()
                                                    {
                                                        controls.solo = !controls.solo;
                                                    }
                                                });

                                                ui.add_space(18.0);
                                                ui.horizontal(|ui| {
                                                    ui.add_space(48.0);
                                                    ui.add(
                                                        egui::Slider::new(
                                                            &mut controls.gain_db,
                                                            -60.0..=12.0,
                                                        )
                                                        .vertical()
                                                        .text("dB")
                                                        .show_value(true),
                                                    );
                                                });
                                                if controls.linked {
                                                    controls.right_gain_db = controls.gain_db;
                                                }
                                                ui.add_space(20.0);
                                                let balance_label = ui.label("Balance");
                                                ui.add(egui::Slider::new(
                                                    &mut controls.pan,
                                                    -1.0..=1.0,
                                                ))
                                                .labelled_by(balance_label.id);
                                                if controls != route.controls {
                                                    edits.push(Edit::Route(
                                                        input.id,
                                                        bus_id,
                                                        Some(controls),
                                                    ));
                                                }
                                                if ui
                                                    .add_enabled(
                                                        !self.audio.running(),
                                                        egui::Button::new("Remove from mix")
                                                            .small(),
                                                    )
                                                    .clicked()
                                                {
                                                    edits.push(Edit::Route(input.id, bus_id, None));
                                                }
                                            } else {
                                                ui.add_space(50.0);
                                                ui.label(
                                                    RichText::new("Not in this mix").color(MUTED),
                                                );
                                                if ui
                                                    .add_enabled(
                                                        !self.audio.running(),
                                                        egui::Button::new("+ Add to mix"),
                                                    )
                                                    .clicked()
                                                {
                                                    edits.push(Edit::Route(
                                                        input.id,
                                                        bus_id,
                                                        Some(ChannelControls::default()),
                                                    ));
                                                }
                                            }
                                        });
                                        ui.add_space(12.0);
                                        meter(ui, "SOURCE LEVEL", self.audio.input_peak(input.id));
                                    });
                            });
                        }
                    });
            });
    }
}

fn gain_controls(ui: &mut egui::Ui, controls: &mut ChannelControls) {
    let mut linked = controls.linked;
    if ui.checkbox(&mut linked, "Link stereo levels").changed() {
        controls.set_linked(linked);
    }
    let mut gain = controls.gain_db;
    if ui
        .add(egui::Slider::new(&mut gain, -60.0..=12.0).text(if linked { "dB" } else { "Left dB" }))
        .changed()
    {
        controls.set_gain_db(gain);
    }
    if !linked {
        ui.add(egui::Slider::new(&mut controls.right_gain_db, -60.0..=12.0).text("Right dB"));
    }
    ui.horizontal(|ui| {
        ui.checkbox(&mut controls.muted, "Mute");
        ui.checkbox(&mut controls.solo, "Solo");
    });
}

fn meter(ui: &mut egui::Ui, label: &str, peak: [f32; 2]) {
    ui.label(RichText::new(label).size(10.0).color(MUTED));
    for (index, level) in peak.into_iter().enumerate() {
        let db = 20.0 * level.max(0.000001).log10();
        let fraction = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        ui.add(
            egui::ProgressBar::new(fraction)
                .corner_radius(0)
                .desired_width(ui.available_width())
                .text(format!(
                    "{}  {}",
                    if index == 0 { "L" } else { "R" },
                    if db < -90.0 {
                        "−∞ dBFS".into()
                    } else {
                        format!("{db:.1} dBFS")
                    }
                ))
                .fill(if db > -1.0 {
                    Color32::from_rgb(244, 166, 93)
                } else {
                    ACCENT
                }),
        );
    }
}

impl eframe::App for Studio {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.audio.poll();
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
            self.save();
        }
        if !self.audio.running() && !self.audio.busy && !text_editor_focused(ctx) {
            if let Some(redo) = history_shortcut(ctx) {
                self.history(redo);
            }
        }
        let mut edits = Vec::new();
        self.toolbar(ctx);
        egui::TopBottomPanel::bottom("status")
            .exact_height(44.0)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(
                        RichText::new(if self.audio.running() {
                            "● AUDIO RUNNING"
                        } else {
                            "○ AUDIO STOPPED"
                        })
                        .color(if self.audio.running() {
                            ACCENT
                        } else {
                            MUTED
                        }),
                    );
                    ui.separator();
                    if self.audio.controls_pending() {
                        ui.label("Applying controls…");
                    }
                    ui.label(
                        self.save_error
                            .as_deref()
                            .or(self.audio.error.as_deref())
                            .unwrap_or(&self.notice),
                    );
                });
            });
        self.sidebar(ctx, &mut edits);
        self.inspector(ctx, &mut edits);
        self.mixer(ctx, &mut edits);
        self.source_picker(ctx, &mut edits);
        self.output_picker(ctx);
        for edit in edits {
            self.edit(edit);
        }
        if self
            .dirty_since
            .is_some_and(|t| t.elapsed() > Duration::from_millis(600))
            && self.save_error.is_none()
        {
            self.save();
            ctx.request_repaint();
        }
        if self.dirty_since.is_some()
            || self.audio.busy
            || self.audio.running()
            || self.audio.controls_pending()
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
    fn on_exit(&mut self) {
        if self.dirty_since.is_some() {
            self.save();
        }
        self.audio.send(Request::Stop);
    }
}

fn text_editor_focused(ctx: &egui::Context) -> bool {
    ctx.memory(|memory| memory.focused())
        .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some())
}

fn history_shortcut(ctx: &egui::Context) -> Option<bool> {
    ctx.input_mut(|input| {
        // egui permits extra Shift modifiers in a less-specific match. Consume
        // redo first so Command-Shift-Z cannot be mistaken for Command-Z.
        let redo = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };
        if input.consume_key(redo, egui::Key::Z)
            || input.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
        {
            Some(true)
        } else if input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z) {
            Some(false)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    fn offline_studio() -> Studio {
        let session = starter();
        Studio {
            selected_bus: session.buses.first().map(|bus| bus.id),
            session,
            path: None,
            selected_input: None,
            route_index: HashMap::new(),
            undo: vec![],
            redo: vec![],
            dirty_since: None,
            notice: String::new(),
            save_error: None,
            preserve_original_before_save: false,
            audio: AudioUi::offline_for_test(),
            search: String::new(),
            output_routing: RoutingDraft::default(),
            outputs_open: false,
            source_picker_open: false,
            source_device: None,
            source_kind: SourceKind::Device,
            source_application: None,
        }
    }

    #[test]
    fn application_source_add_route_save_and_undo_preserve_stable_identity_without_starting_audio()
    {
        let mut studio = offline_studio();
        let selector = ApplicationSelector::WindowsExecutable {
            path: r"C:\Program Files\Player\player.exe".into(),
        };
        let id = selector.encode().unwrap();
        studio.edit(Edit::AddApplication(id.clone(), "Player".into()));
        assert_eq!(studio.session.inputs.len(), 1);
        let input_id = studio.session.inputs[0].id;
        assert_eq!(studio.selected_input, Some(input_id));
        let bus_id = studio.session.buses[0].id;
        studio.edit(Edit::Route(
            input_id,
            bus_id,
            Some(ChannelControls::default()),
        ));
        let persisted = serde_json::to_string(&studio.document()).unwrap();
        let restored: store::StudioDocument = serde_json::from_str(&persisted).unwrap();
        assert_eq!(restored.session.sends[0].input_id, input_id);
        let InputKind::Application { application_id } = &restored.session.inputs[0].kind else {
            panic!("application selection lost")
        };
        assert_eq!(
            ApplicationSelector::decode(application_id).unwrap(),
            selector
        );
        assert_eq!(application_id, &id);
        studio.history(false);
        assert!(studio.session.sends.is_empty());
        studio.history(false);
        assert!(studio.session.inputs.is_empty());
        studio.history(true);
        assert_eq!(studio.session.inputs[0].id, input_id);
        assert!(!studio.audio.running() && !studio.audio.busy && !studio.audio.can_stop());
    }

    #[test]
    fn application_source_rejects_transient_identity_and_busy_edits_without_changing_studio() {
        let mut studio = offline_studio();
        let before = studio.document();
        studio.edit(Edit::AddApplication("pid:1234".into(), "Player".into()));
        assert_eq!(studio.document(), before);
        assert!(studio.undo.is_empty());
        let id = ApplicationSelector::MacBundle {
            bundle_id: "com.example.Player".into(),
            path: "/Applications/Player.app".into(),
        }
        .encode()
        .unwrap();
        studio.audio.busy = true;
        studio.edit(Edit::AddApplication(id, "Player".into()));
        assert_eq!(studio.document(), before);
        assert!(studio.undo.is_empty());
    }

    #[test]
    fn only_text_focus_reserves_text_history_shortcuts() {
        let context = egui::Context::default();
        let mut value = String::from("Studio");
        let _ = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let button = ui.button("Save");
                button.request_focus();
                assert!(!text_editor_focused(ctx));
                let editor = ui.text_edit_singleline(&mut value);
                editor.request_focus();
                assert!(text_editor_focused(ctx));
                button.request_focus();
                assert!(!text_editor_focused(ctx));
            });
        });
    }

    #[test]
    fn redo_shortcuts_never_dispatch_undo_or_dispatch_twice() {
        for (key, shift, expected) in [
            (egui::Key::Z, true, true),
            (egui::Key::Y, false, true),
            (egui::Key::Z, false, false),
        ] {
            let modifiers = egui::Modifiers {
                shift,
                ..egui::Modifiers::COMMAND
            };
            let context = egui::Context::default();
            let _ = context.run(
                egui::RawInput {
                    modifiers,
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    assert_eq!(history_shortcut(ctx), Some(expected));
                    assert_eq!(history_shortcut(ctx), None);
                },
            );
        }
    }
}
