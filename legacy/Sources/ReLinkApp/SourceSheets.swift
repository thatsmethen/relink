import SwiftUI
import RaveLinkCore
import RaveLinkAudio

struct InputSheet: View {
    @ObservedObject var studio: StudioModel
    @ObservedObject var audio: EngineClient
    @Environment(\.dismiss) private var dismiss
    @State private var kind: SourceKind = .device
    @State private var name = ""
    @State private var deviceUID = ""
    @State private var bundleID = ""
    @State private var channelOffset = 0
    @State private var validationError: String?
    private var valid: Bool { (kind == .device && !deviceUID.isEmpty) || (kind == .application && !bundleID.isEmpty) || kind == .system || kind == .tone }
    var body: some View {
        VStack(alignment: .leading, spacing: 22) {
            HStack { Text(studio.sourceTarget == nil ? "Add an input" : (studio.sourceReplacement == nil ? "Add a source" : "Change source")).font(.title2.weight(.semibold)); Spacer(); Button { audio.refreshDevices() } label: { Label("Refresh", systemImage: "arrow.clockwise") } }
            Text("Bring audio into your studio, then choose which mixes can hear it.").foregroundStyle(.secondary)
            Picker("Source", selection: $kind) {
                Text("Device").tag(SourceKind.device)
                Text("Application").tag(SourceKind.application)
                Text("System").tag(SourceKind.system)
                Text("Test tone").tag(SourceKind.tone)
            }.pickerStyle(.segmented)
            Group {
                switch kind {
                case .device:
                    Picker("Audio device", selection: Binding(get: { deviceUID }, set: { deviceUID = $0; channelOffset = 0 })) {
                        Text("Choose an input…").tag("")
                        ForEach(audio.devices.filter { $0.inputChannels > 0 }) { Text("\($0.name) (\($0.inputChannels) ch)").tag($0.id) }
                    }
                    if let device = audio.devices.first(where: { $0.id == deviceUID }), device.inputChannels > 1 {
                        Picker("Input channels", selection: $channelOffset) {
                            ForEach(Array(stride(from: 0, to: device.inputChannels, by: 2)), id: \.self) { value in
                                Text(value + 1 < device.inputChannels ? "\(value + 1)–\(value + 2)" : "\(value + 1) (mono)").tag(value)
                            }
                        }
                    }
                    Text("Microphones, interfaces and virtual devices are supported. Use Input settings to select a different stereo pair.").font(.caption).foregroundStyle(.secondary)
                case .application:
                    Picker("Application", selection: $bundleID) {
                        Text("Choose an application…").tag("")
                        ForEach(audio.applications) { Text($0.name).tag($0.id) }
                    }
                    Text("Open an app and play audio if it is missing here. macOS will ask for audio capture permission when needed. While mixing, this app’s original playback is replaced by your routes.").font(.caption).foregroundStyle(.secondary)
                case .system:
                    Label("All system audio", systemImage: "desktopcomputer").font(.headline)
                    Text("Captures applications outside dedicated app inputs. RaveLink’s own output is excluded. Original playback is replaced by your routes while mixing.").font(.caption).foregroundStyle(.secondary)
                case .tone:
                    Label("440 Hz calibration tone", systemImage: "waveform").font(.headline)
                    Text("A quiet signal for checking your route. Starts only after you enable a mix and start audio.").font(.caption).foregroundStyle(.secondary)
                }
            }.frame(minHeight: 80, alignment: .top)
            if studio.sourceTarget == nil { TextField("Name (optional)", text: $name).textFieldStyle(.roundedBorder) }
            if let validationError { Text(validationError).font(.callout).foregroundStyle(.orange).fixedSize(horizontal: false, vertical: true) }
            HStack { Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction); Spacer(); Button(studio.sourceReplacement != nil ? "Use source" : (studio.sourceTarget == nil ? "Add input" : "Add source")) { add() }.buttonStyle(AccentButtonStyle()).disabled(!valid).keyboardShortcut(.defaultAction) }
        }.padding(30).frame(width: 560)
        .onAppear {
            if let target = studio.sourceTarget, let slot = studio.sourceReplacement,
               let channel = studio.session.channels.first(where: { $0.id == target }) {
                let sources = channel.allSources
                if sources.indices.contains(slot) {
                    let source = sources[slot]
                    kind = source.kind; deviceUID = source.deviceUID ?? ""; bundleID = source.bundleID ?? ""; channelOffset = source.channelOffset
                }
            }
        }
    }
    private func add() {
        let suggested: String
        switch kind {
        case .device: suggested = audio.devices.first { $0.id == deviceUID }?.name ?? "Input"
        case .application: suggested = audio.applications.first { $0.id == bundleID }?.name ?? "Application"
        case .system: suggested = "System audio"
        case .tone: suggested = "Test tone"
        }
        let source = InputSource(kind: kind, deviceUID: deviceUID.isEmpty ? nil : deviceUID, bundleID: bundleID.isEmpty ? nil : bundleID, channelOffset: channelOffset).canonicalized
        do {
            try studio.add(MixerChannel(name: name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? suggested : name, source: source))
            dismiss()
        } catch { validationError = error.localizedDescription }
    }
}
struct MixSheet: View {
    @ObservedObject var studio: StudioModel
    @ObservedObject var audio: EngineClient
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var deviceUID: String?
    @State private var validationError: String?
    var body: some View {
        VStack(alignment: .leading, spacing: 22) {
            Text("Add a mix").font(.title2.weight(.semibold))
            Text("Give a destination its own balance of your inputs.").foregroundStyle(.secondary)
            TextField("Mix name", text: $name).textFieldStyle(.roundedBorder)
            Picker("Output", selection: $deviceUID) {
                Text("Choose later").tag(nil as String?)
                ForEach(audio.devices.filter { $0.outputChannels > 1 }) { Text($0.name).tag(Optional($0.id)) }
            }
            Text("For a streaming or chat app, choose a virtual device such as RaveLink Stream or BlackHole. For listening, choose your headphones.").font(.caption).foregroundStyle(.secondary)
            if let validationError { Text(validationError).font(.callout).foregroundStyle(.orange).fixedSize(horizontal: false, vertical: true) }
            HStack { Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction); Spacer(); Button("Add mix") {
                do { try studio.add(MixBus(name: name, deviceUID: deviceUID)); dismiss() }
                catch { validationError = error.localizedDescription }
            }.buttonStyle(AccentButtonStyle()).disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty).keyboardShortcut(.defaultAction) }
        }.padding(30).frame(width: 500)
    }
}

struct MonitorOutputsSheet: View {
    @ObservedObject var studio: StudioModel
    @ObservedObject var audio: EngineClient
    @Environment(\.dismiss) private var dismiss
    @State private var selectedUID = ""
    @State private var offset = 0
    @State private var validationError: String?
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("Additional outputs").font(.title2.weight(.semibold))
            Text("Play the same mix on more than one device. All destinations follow this mix’s level and mute controls.").foregroundStyle(.secondary)
            if let bus = studio.session.buses.first(where: { $0.id == studio.outputTarget }) {
                Text(bus.name).font(.headline)
                let outputs = bus.additionalOutputs
                if outputs.isEmpty { Text("No additional outputs yet.").font(.callout).foregroundStyle(.secondary) }
                if !outputs.isEmpty {
                    ScrollView {
                        VStack(spacing: 12) {
                            ForEach(Array(outputs.enumerated()), id: \.offset) { _, output in
                                HStack {
                                    Image(systemName: "speaker.wave.2")
                                    Text(audio.devices.first(where: { $0.id == output.deviceUID })?.name ?? "Missing device")
                                    Text("\(output.channelOffset + 1)–\(output.channelOffset + 2)").foregroundStyle(.secondary)
                                    Spacer()
                                    Button("Remove") { studio.removeOutput(output, from: bus.id) }
                                }
                            }
                        }
                    }.frame(height: min(CGFloat(outputs.count) * 38, 228))
                }
                Divider()
                Picker("Device", selection: $selectedUID) {
                    Text("Choose output…").tag("")
                    ForEach(audio.devices.filter { $0.outputChannels > 1 }) { Text($0.name).tag($0.id) }
                }.onChange(of: selectedUID) { _, _ in offset = 0 }
                if let device = audio.devices.first(where: { $0.id == selectedUID }), device.outputChannels > 2 {
                    Picker("Channels", selection: $offset) {
                        ForEach(Array(stride(from: 0, to: device.outputChannels - 1, by: 2)), id: \.self) { value in Text("\(value + 1)–\(value + 2)").tag(value) }
                    }
                }
                let duplicate = bus.allOutputs.contains { $0.deviceUID == selectedUID && abs($0.channelOffset - offset) < 2 }
                if let validationError { Text(validationError).font(.callout).foregroundStyle(.orange).fixedSize(horizontal: false, vertical: true) }
                HStack {
                    Button("Add output") {
                        do { try studio.addOutput(MixDestination(deviceUID: selectedUID, channelOffset: offset), to: bus.id); selectedUID = ""; validationError = nil }
                        catch { validationError = error.localizedDescription }
                    }.disabled(selectedUID.isEmpty || duplicate)
                    Spacer()
                    Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
                }
            } else { Button("Done") { dismiss() } }
        }.padding(30).frame(width: 540)
    }
}
