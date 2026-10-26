import SwiftUI
import RaveLinkCore
import RaveLinkAudio

private let canvas = Color(red: 0.065, green: 0.075, blue: 0.095)
private let panel = Color(red: 0.105, green: 0.12, blue: 0.15)
private let accent = Color(red: 0.42, green: 0.87, blue: 0.75)

struct StudioView: View {
    @ObservedObject var studio: StudioModel
    @ObservedObject var audio: EngineClient
    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            HStack(spacing: 0) {
                VStack(alignment: .leading, spacing: 0) {
                    HStack {
                        VStack(alignment: .leading, spacing: 6) {
                            Text("YOUR STUDIO").font(.system(size: 10, weight: .semibold, design: .monospaced)).foregroundStyle(accent)
                            Text("Mixes & routing").font(.system(size: 22, weight: .semibold))
                        }
                        Spacer()
                        Button { studio.beginInput() } label: { Label("Add input", systemImage: "plus") }.buttonStyle(AccentButtonStyle())
                    }.padding(24)
                    matrix
                }
                if let id = studio.selectedChannel, let channel = studio.channel(id) {
                    Divider()
                    effects(channel).frame(width: 265)
                }
            }
            Divider()
            footer
        }
        .background(canvas).tint(accent)
        .sheet(isPresented: $studio.showingInput, onDismiss: { studio.sourceTarget = nil; studio.sourceReplacement = nil }) { InputSheet(studio: studio, audio: audio) }
        .sheet(isPresented: $studio.showingOutputs) { MonitorOutputsSheet(studio: studio, audio: audio) }
        .sheet(isPresented: $studio.showingMix) { MixSheet(studio: studio, audio: audio) }
        .sheet(isPresented: $studio.showingSettings) { setupInfo }
        .sheet(isPresented: $studio.showingDiagnostics) { EngineDiagnosticsView(audio: audio) }
        .sheet(isPresented: $studio.showingSoundCheck) { SoundCheckView(studio: studio, audio: audio) }
        .alert("Studio needs attention", isPresented: Binding(get: { studio.notice != nil }, set: { if !$0 { studio.notice = nil } })) {
            Button("OK") { studio.notice = nil }
        } message: { Text(studio.notice ?? "") }
    }
    private var header: some View {
        HStack(spacing: 12) {
            Image(systemName: "waveform.path").font(.system(size: 25, weight: .medium)).foregroundStyle(accent)
            Text("ravelink").font(.system(size: 22, weight: .semibold, design: .rounded))
            Rectangle().fill(.white.opacity(0.12)).frame(width: 1, height: 22).padding(.horizontal, 8)
            TextField("Studio name", text: $studio.session.name).textFieldStyle(.plain).frame(maxWidth: 220)
            Spacer()
            Menu {
                Button("Import studio…") { studio.importSession() }
                Button("Export studio…") { studio.exportSession() }
                Divider()
                Button("Audio setup…") { studio.showingSettings = true }
                Button("Engine diagnostics…") { studio.showingDiagnostics = true }
                Button("Refresh devices") { audio.refreshDevices() }
            } label: { Label("Studio options", systemImage: "ellipsis.circle").labelStyle(.iconOnly).font(.title3) }.menuStyle(.borderlessButton).frame(width: 34).help("Studio options")
            Button { studio.toggleAudio() } label: {
                Label(audio.starting ? "Cancel start" : (audio.running ? "Stop audio" : "Start audio"), systemImage: (audio.running || audio.starting) ? "stop.fill" : "play.fill").frame(width: 105)
            }.buttonStyle(AccentButtonStyle(color: audio.running ? Color(red: 0.7, green: 0.16, blue: 0.2) : accent, foreground: audio.running ? .white : .black))
        }.padding(.horizontal, 24).padding(.top, 30).padding(.bottom, 18)
    }
    private var matrix: some View {
        ScrollView([.horizontal, .vertical]) {
            LazyVStack(alignment: .leading, spacing: 1) {
                HStack(alignment: .top, spacing: 1) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("INPUTS").font(.system(size: 11, weight: .semibold)).foregroundStyle(.secondary)
                        Text("Choose where each input goes").font(.caption).foregroundStyle(.secondary)
                    }.frame(width: 260, height: 148, alignment: .leading).padding(.horizontal, 18)
                    ForEach(studio.session.buses) { bus in busHeader(bus) }
                    Button { studio.showingMix = true } label: { Image(systemName: "plus").frame(width: 42, height: 42) }.buttonStyle(.bordered).help("Add mix").accessibilityLabel("Add mix").padding(16)
                }
                if studio.session.channels.isEmpty {
                    VStack(spacing: 14) {
                        Image(systemName: "waveform.badge.plus").font(.system(size: 38)).foregroundStyle(accent)
                        Text("Build your first mix").font(.title2.weight(.semibold))
                        Text("Add a microphone, an app, or system audio.\nThen turn on the mixes that should hear it.").foregroundStyle(.secondary).multilineTextAlignment(.center)
                        Button("Add your first input") { studio.beginInput() }.buttonStyle(AccentButtonStyle())
                        Button("Set up virtual outputs") { studio.showingSettings = true }.buttonStyle(.plain).foregroundStyle(.secondary)
                    }.frame(width: max(820, CGFloat(studio.session.buses.count * 196 + 296)), height: 330)
                } else {
                    ForEach(studio.session.channels) { channel in channelRow(channel) }
                    Button { studio.beginInput() } label: { Label("Add input", systemImage: "plus").frame(width: 260, height: 50, alignment: .leading) }.buttonStyle(.plain).foregroundStyle(accent).padding(.horizontal, 18)
                }
            }.padding(.horizontal, 24).padding(.bottom, 24)
        }.defaultScrollAnchor(.topLeading, for: .alignment)
    }
    private func busHeader(_ bus: MixBus) -> some View {
        let mute = busBinding(bus, \.muted)
        return VStack(alignment: .leading, spacing: 10) {
            HStack {
                Image(systemName: bus.id == studio.session.buses.first?.id ? "headphones" : "antenna.radiowaves.left.and.right").foregroundStyle(accent)
                TextField("Mix name", text: busBinding(bus, \.name)).textFieldStyle(.plain).font(.headline)
                Menu {
                    Button(bus.muted ? "Unmute mix" : "Mute mix") { mute.wrappedValue.toggle() }
                    Button("Additional outputs…") { studio.outputTarget = bus.id; studio.showingOutputs = true }
                    Divider()
                    Button("Move mix left") { studio.moveBus(bus.id, by: -1) }.disabled(studio.session.buses.first?.id == bus.id)
                    Button("Move mix right") { studio.moveBus(bus.id, by: 1) }.disabled(studio.session.buses.last?.id == bus.id)
                    Divider()
                    Button("Remove mix", role: .destructive) { studio.removeBus(bus.id) }
                } label: { Label("Options for mix \(bus.name)", systemImage: "ellipsis").labelStyle(.iconOnly) }.menuStyle(.borderlessButton).frame(width: 18)
            }
            Picker("Output", selection: Binding(get: { busBinding(bus, \.deviceUID).wrappedValue }, set: { uid in
                do { try studio.setPrimaryOutput(uid, on: bus.id) } catch { studio.notice = error.localizedDescription }
            })) {
                Text("Choose output…").tag(nil as String?)
                ForEach(audio.devices.filter { $0.outputChannels > 1 }) { Text($0.name).tag(Optional($0.id)) }
                if let uid = bus.deviceUID, !audio.devices.contains(where: { $0.id == uid }) { Text("Missing device").tag(Optional(uid)) }
            }.labelsHidden().frame(maxWidth: .infinity).accessibilityLabel("\(bus.name) output")
            HStack(spacing: 6) {
                Button { mute.wrappedValue.toggle() } label: { Image(systemName: bus.muted ? "speaker.slash.fill" : "speaker.wave.2.fill").foregroundStyle(bus.muted ? .orange : .secondary) }.buttonStyle(.plain)
                    .accessibilityLabel(bus.muted ? "Unmute mix \(bus.name)" : "Mute mix \(bus.name)")
                    .accessibilityValue(bus.muted ? "Muted" : "Unmuted")
                Slider(value: busBinding(bus, \.gainDB), in: -60...12).accessibilityLabel("\(bus.name) master level")
                    .accessibilityValue(audioControlValue(bus.gainDB, unit: "dB"))
                Text(db(bus.gainDB)).font(.system(size: 10, design: .monospaced)).frame(width: 38)
            }
            LevelMeter(level: audio.busMeters[bus.id] ?? 0, active: audio.running && !bus.muted,
                       label: "\(bus.name) mix level", inactiveStatus: audio.running ? "Muted" : "Stopped")
            if let d = audio.devices.first(where: { $0.id == bus.deviceUID }), d.outputChannels > 2 {
                Picker("Pair", selection: Binding(get: { busBinding(bus, \.channelOffset).wrappedValue }, set: { offset in
                    do { try studio.setOutputPair(offset, on: bus.id) } catch { studio.notice = error.localizedDescription }
                })) {
                    ForEach(Array(stride(from: 0, to: d.outputChannels - 1, by: 2)), id: \.self) { offset in Text("\(offset + 1)–\(offset + 2)").tag(offset) }
                }.font(.caption).accessibilityLabel("\(bus.name) output channel pair")
            }
        }.padding(16).frame(width: 196, height: 148, alignment: .top).background(panel)
    }
    private func channelRow(_ channel: MixerChannel) -> some View {
        let mute = channelBinding(channel, \.muted)
        let solo = channelBinding(channel, \.solo)
        return HStack(spacing: 1) {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 12) {
                    Image(systemName: sourceIcon(channel.source.kind)).font(.title3).foregroundStyle(accent).frame(width: 24)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(channel.name).font(.headline).lineLimit(1)
                        Text(sourceDescription(channel)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Spacer(minLength: 0)
                    Menu {
                        Button("Input settings") { studio.selectedChannel = channel.id }
                        Button("Sound Check…") { studio.openSoundCheck(channel.id) }
                        Divider()
                        Button("Move input up") { studio.moveChannel(channel.id, by: -1) }.disabled(studio.session.channels.first?.id == channel.id)
                        Button("Move input down") { studio.moveChannel(channel.id, by: 1) }.disabled(studio.session.channels.last?.id == channel.id)
                        Divider()
                        Button("Remove input", role: .destructive) { studio.removeChannel(channel.id) }
                    } label: { Label("Options for input \(channel.name)", systemImage: "ellipsis").labelStyle(.iconOnly) }.menuStyle(.borderlessButton).frame(width: 18)
                }
                LevelMeter(level: audio.channelMeters[channel.id] ?? 0, active: audio.running && !channel.muted,
                           label: "\(channel.name) input level", inactiveStatus: audio.running ? "Muted" : "Stopped")
                HStack(spacing: 8) {
                    Button(channel.muted ? "Unmute" : "Mute") { mute.wrappedValue.toggle() }.tint(channel.muted ? .orange : .gray)
                        .accessibilityLabel(channel.muted ? "Unmute input \(channel.name)" : "Mute input \(channel.name)")
                        .accessibilityValue(channel.muted ? "Muted" : "Unmuted")
                    Button("Solo") { solo.wrappedValue.toggle() }.tint(channel.solo ? accent : .gray)
                        .accessibilityLabel("Solo input \(channel.name)").accessibilityValue(channel.solo ? "On" : "Off")
                    Spacer()
                    Button { studio.selectedChannel = studio.selectedChannel == channel.id ? nil : channel.id } label: { Label("Effects", systemImage: "slider.horizontal.3") }
                        .accessibilityLabel("Effects for \(channel.name)")
                        .accessibilityValue(studio.selectedChannel == channel.id ? "Expanded" : "Collapsed")
                }.buttonStyle(.bordered).controlSize(.small)
            }.padding(18).frame(width: 296, height: 140).background(studio.selectedChannel == channel.id ? panel.opacity(1.5) : panel)
            ForEach(studio.session.buses) { bus in
                if let route = studio.route(channelID: channel.id, busID: bus.id) { routeCell(route, channel: channel, bus: bus) }
                else { Color.clear.frame(width: 196, height: 140) }
            }
        }
    }
    private func routeCell(_ send: MixSend, channel: MixerChannel, bus: MixBus) -> some View {
        let mute = sendBinding(send, \.muted)
        return VStack(spacing: 12) {
            HStack {
                Toggle(isOn: Binding(get: { !mute.wrappedValue }, set: { mute.wrappedValue = !$0 })) { Text(send.muted ? "Off" : "On").font(.caption.weight(.medium)) }.toggleStyle(.switch).controlSize(.small)
                    .accessibilityLabel("Route \(channel.name) to \(bus.name)")
                Spacer()
                Text(db(send.gainDB)).font(.system(size: 12, design: .monospaced)).foregroundStyle(send.muted ? .secondary : accent)
            }
            Slider(value: sendBinding(send, \.gainDB), in: -60...12).disabled(send.muted).accessibilityLabel("\(channel.name) level in \(bus.name)")
                .accessibilityValue(audioControlValue(send.gainDB, unit: "dB"))
            HStack {
                Text("L").font(.system(size: 9, weight: .medium))
                Slider(value: sendBinding(send, \.pan), in: -1...1).disabled(send.muted).accessibilityLabel("\(channel.name) pan in \(bus.name)")
                    .accessibilityValue(send.pan == 0 ? "Center" : "\(send.pan < 0 ? "Left" : "Right") \(Int(abs(send.pan) * 100)) percent")
                Text("R").font(.system(size: 9, weight: .medium))
            }.foregroundStyle(.secondary).opacity(0.7)
        }.padding(18).frame(width: 196, height: 140).background(send.muted ? panel.opacity(0.4) : panel)
    }
    private func effects(_ channel: MixerChannel) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                HStack {
                    Text("INPUT SETTINGS").font(.system(size: 11, weight: .semibold)).foregroundStyle(accent)
                    Spacer()
                    Button { studio.selectedChannel = nil } label: { Image(systemName: "xmark") }.buttonStyle(.plain).help("Close input settings").accessibilityLabel("Close input settings")
                }
                TextField("Input name", text: channelBinding(channel, \.name)).font(.title2).textFieldStyle(.plain)
                sourcesSection(channel)
                Divider()
                Text("Applied before this input reaches your mixes.").font(.caption).foregroundStyle(.secondary)
                Button { studio.openSoundCheck(channel.id) } label: { Label("Sound Check", systemImage: "waveform") }
                    .buttonStyle(.bordered)
                effectSlider("Input gain", value: channelBinding(channel, \.effects.gainDB), range: -60...24, suffix: "dB")
                effectSlider("High-pass filter", value: channelBinding(channel, \.effects.highPassHz), range: 0...1000, suffix: "Hz")
                effectSlider("Noise gate", value: channelBinding(channel, \.effects.gateThresholdDB), range: -100...0, suffix: "dB")
                effectSlider("Compressor threshold", value: channelBinding(channel, \.effects.compressorThresholdDB), range: -60...0, suffix: "dB")
                effectSlider("Compressor ratio", value: channelBinding(channel, \.effects.compressorRatio), range: 1...20, suffix: ":1")
                Button("Reset effects") { channelBinding(channel, \.effects).wrappedValue = .init() }.buttonStyle(.bordered)
                Divider()
                AudioUnitRackView(studio: studio, channelID: channel.id)
            }.padding(22)
        }.background(panel.opacity(0.6))
    }
    private func sourcesSection(_ channel: MixerChannel) -> some View {
        let sources = channel.allSources
        return VStack(alignment: .leading, spacing: 10) {
            Text("Sources").font(.headline)
            ForEach(Array(sources.enumerated()), id: \.element) { slot, source in
                HStack {
                    Image(systemName: sourceIcon(source.kind)).foregroundStyle(accent)
                    Text(sourceName(source)).font(.caption).lineLimit(2)
                    Spacer()
                    Menu {
                        Button("Change source…") { studio.beginInput(target: channel.id, replacing: slot) }
                        if sources.count > 1 {
                            Button("Remove source", role: .destructive) {
                                studio.removeSource(source, from: channel.id)
                            }
                        }
                    } label: { Label("Options for source \(slot + 1), \(sourceName(source)), in \(channel.name)", systemImage: "ellipsis").labelStyle(.iconOnly) }.menuStyle(.borderlessButton).frame(width: 18)
                }
            }
            Button { studio.beginInput(target: channel.id) } label: { Label("Add source", systemImage: "plus") }.buttonStyle(.bordered)
        }
    }
    // SwiftUI may read a disappearing control's binding after the collection changes.
    // Resolve stable identities on every access; removed controls retain their last value.
    private func busBinding<Value>(_ bus: MixBus, _ path: WritableKeyPath<MixBus, Value>) -> Binding<Value> {
        Binding(get: { studio.bus(bus.id)?[keyPath: path] ?? bus[keyPath: path] },
                set: { studio.setBus(bus.id, path, to: $0) })
    }
    private func channelBinding<Value>(_ channel: MixerChannel, _ path: WritableKeyPath<MixerChannel, Value>) -> Binding<Value> {
        Binding(get: { studio.channel(channel.id)?[keyPath: path] ?? channel[keyPath: path] },
                set: { studio.setChannel(channel.id, path, to: $0) })
    }
    private func sendBinding<Value>(_ send: MixSend, _ path: WritableKeyPath<MixSend, Value>) -> Binding<Value> {
        let id = send.id
        return Binding(get: { studio.send(id)?[keyPath: path] ?? send[keyPath: path] },
                       set: { studio.setSend(id, path, to: $0) })
    }
    private func sourceName(_ source: InputSource) -> String {
        switch source.kind {
        case .device: return audio.devices.first(where: { $0.id == source.deviceUID })?.name ?? "Device unavailable"
        case .application: return audio.applications.first(where: { $0.id == source.bundleID })?.name ?? source.bundleID ?? "Application"
        case .system: return "System audio"
        case .tone: return "Test tone"
        }
    }
    private func effectSlider(_ title: String, value: Binding<Float>, range: ClosedRange<Float>, suffix: String) -> some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack { Text(title).font(.callout); Spacer(); Text("\(Int(value.wrappedValue)) \(suffix)").font(.system(size: 11, design: .monospaced)).foregroundStyle(accent) }
            Slider(value: value, in: range).accessibilityLabel(title).accessibilityValue(audioControlValue(value.wrappedValue, unit: suffix))
        }
    }
    private var footer: some View {
        HStack(spacing: 8) {
            Circle().fill(audio.running ? accent : .gray).frame(width: 6, height: 6)
            Text(audio.starting ? "Connecting audio…" : (audio.running ? "Audio running" : "Audio stopped")).font(.caption.weight(.medium))
            if let error = audio.errorMessage { Text(error).font(.caption).foregroundStyle(.orange).lineLimit(2) }
            Spacer()
            Text(studio.saveStatus).font(.caption).foregroundStyle(studio.saveStatus.hasPrefix("Not saved") ? .orange : .secondary).lineLimit(1)
            Text("\(studio.session.channels.count) inputs  ·  \(studio.session.buses.count) mixes").font(.system(size: 11, design: .monospaced)).foregroundStyle(.secondary)
            Button { studio.showingSettings = true } label: { Image(systemName: "questionmark.circle") }.buttonStyle(.plain).help("Audio setup").accessibilityLabel("Audio setup")
        }.padding(.horizontal, 24).padding(.vertical, 14)
    }
    private var setupInfo: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("Connect your studio").font(.title2.weight(.semibold))
            Text("Listen locally").font(.headline)
            Text("Select your headphones or audio interface as the Personal mix output. Add an input, enable its Personal route, then start audio.")
            Text("Send a mix to another app").font(.headline)
            Text("Select a RaveLink virtual device or BlackHole as the Stream output. In OBS, Discord or your recording app, select that same device as the microphone. Each independent mix needs its own virtual device or channel pair.")
            Text("Application audio").font(.headline)
            Text("macOS may request permission to capture system audio. If a source is missing, open it and play audio, then refresh devices. A missing or denied source appears as an error.")
            Link("Virtual device build and installation guide", destination: URL(string: "https://github.com/thatsmethen/ravelink/blob/main/docs/virtual-devices.md")!)
            HStack { Spacer(); Button("Done") { studio.showingSettings = false }.keyboardShortcut(.defaultAction) }
        }.font(.callout).frame(width: 520).padding(30)
    }
    private func sourceDescription(_ channel: MixerChannel) -> String {
        if !channel.additionalSources.isEmpty { return "\(channel.additionalSources.count + 1) sources" }
        switch channel.source.kind {
        case .device: return audio.devices.first(where: { $0.id == channel.source.deviceUID })?.name ?? "Device unavailable"
        case .application: return channel.source.bundleID ?? "Application"
        case .system: return "System audio"
        case .tone: return "Test tone · 440 Hz"
        }
    }
}
func sourceIcon(_ kind: SourceKind) -> String {
    switch kind { case .device: return "mic.fill"; case .application: return "app.dashed"; case .system: return "desktopcomputer"; case .tone: return "waveform" }
}
func db(_ value: Float) -> String { String(format: "%+.0f dB", value) }
func audioControlValue(_ value: Float, unit: String) -> String {
    let number = value.formatted(.number.precision(.fractionLength(0...1)))
    switch unit {
    case "dB": return "\(number) decibels"
    case "Hz": return value == 0 ? "Off" : "\(number) hertz"
    case ":1": return "\(number) to 1"
    default: return "\(number) \(unit)"
    }
}
struct LevelMeter: View {
    let level: Float
    let active: Bool
    var label = "Audio level"
    var inactiveStatus = "Stopped"
    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(.white.opacity(0.08))
                Capsule().fill(level > 0.95 ? Color.orange : accent).frame(width: geo.size.width * CGFloat(active && level.isFinite ? min(1, max(0, (20 * log10(max(level, 0.001)) + 60) / 60)) : 0))
            }
        }.frame(height: 4).accessibilityLabel(label).accessibilityValue(active ? "\(Int(20 * log10(max(level.isFinite ? level : 0, 0.001)))) decibels" : inactiveStatus)
    }
}

struct AccentButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var enabled
    var color = Color(red: 0.42, green: 0.87, blue: 0.75)
    var foreground = Color.black
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.font(.system(size: 13, weight: .semibold))
            .foregroundStyle(foreground)
            .padding(.horizontal, 14).padding(.vertical, 8)
            .background(color.opacity(enabled ? (configuration.isPressed ? 0.75 : 1) : 0.35), in: RoundedRectangle(cornerRadius: 7))
            .contentShape(RoundedRectangle(cornerRadius: 7))
    }
}
