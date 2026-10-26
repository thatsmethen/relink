import SwiftUI
import RaveLinkCore
import RaveLinkAudio

struct SoundCheckView: View {
    @ObservedObject var studio: StudioModel
    @ObservedObject var audio: EngineClient
    @State private var outputUID: String?
    @State private var outputPair = 0
    private var channel: MixerChannel? { studio.session.channels.first { $0.id == studio.soundCheckChannelID } }
    private var snapshot: SoundCheckSnapshot? { audio.soundCheck }
    private var active: Bool { audio.running && snapshot != nil }
    private var mode: SoundCheckMode { snapshot?.mode ?? .empty }
    private var busy: Bool { studio.preparingSoundCheck || audio.starting }
    private let accent = Color(red: 0.42, green: 0.87, blue: 0.75)

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Sound Check").font(.title2.weight(.semibold))
                    Text(channel?.name ?? "Input removed").foregroundStyle(.secondary)
                }
                Spacer()
                Button("Done") { studio.endSoundCheck(); studio.showingSoundCheck = false }
                    .keyboardShortcut(.cancelAction)
            }
            Text("Studio audio is paused. Record up to 30 seconds, then loop it while adjusting your effects. Only the listening output below receives playback.")
                .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            HStack(alignment: .top, spacing: 28) {
                VStack(alignment: .leading, spacing: 18) {
                    Text("Listen through").font(.headline)
                    Picker("Listening output", selection: $outputUID) {
                        Text("Choose output…").tag(nil as String?)
                        ForEach(audio.devices.filter { $0.outputChannels > 1 }) { device in
                            Text(device.name).tag(Optional(device.id))
                        }
                    }.labelsHidden().accessibilityLabel("Sound Check listening output")
                    if let device = audio.devices.first(where: { $0.id == outputUID }), device.outputChannels > 2 {
                        Picker("Output pair", selection: $outputPair) {
                            ForEach(Array(stride(from: 0, to: device.outputChannels - 1, by: 2)), id: \.self) { offset in
                                Text("\(offset + 1)–\(offset + 2)").tag(offset)
                            }
                        }
                    }
                    VStack(spacing: 14) {
                        Image(systemName: mode == .recording ? "record.circle" : (mode == .playing ? "repeat" : "waveform"))
                            .font(.system(size: 34)).foregroundStyle(mode == .recording ? .red : accent)
                        Text(status).font(.headline)
                        Text(duration).font(.system(size: 25, weight: .medium, design: .monospaced))
                            .accessibilityLabel("Recorded duration \(duration)")
                        ProgressView(value: min(snapshot?.duration ?? 0, 30), total: 30).tint(mode == .recording ? .red : accent)
                        LevelMeter(level: snapshot?.peak ?? 0, active: active && (mode == .recording || mode == .playing))
                            .accessibilityLabel("Sound Check input level")
                    }.padding(22).frame(maxWidth: .infinity).background(.white.opacity(0.04), in: RoundedRectangle(cornerRadius: 12))
                    HStack {
                        if busy {
                            Button("Cancel") { studio.endSoundCheck() }.buttonStyle(.bordered)
                        } else if active && (mode == .recording || mode == .playing) {
                            Button { studio.commandSoundCheck(.stop) } label: { Label("Stop", systemImage: "stop.fill") }
                                .buttonStyle(AccentButtonStyle())
                        } else {
                            Button {
                                studio.recordSoundCheck(output: .init(deviceUID: outputUID, channelOffset: outputPair))
                            } label: { Label(mode == .ready ? "Record again" : "Record sample", systemImage: "record.circle") }
                                .buttonStyle(AccentButtonStyle()).disabled(outputUID == nil || channel == nil)
                        }
                        Spacer()
                        Button { studio.commandSoundCheck(.play) } label: { Label("Play loop", systemImage: "play.fill") }
                            .buttonStyle(.bordered).disabled(!active || mode != .ready || busy)
                    }
                    HStack {
                        Text("Sample stays in memory until this window closes.").font(.caption).foregroundStyle(.secondary)
                        Spacer()
                        Button { studio.commandSoundCheck(.clear) } label: { Image(systemName: "trash") }
                            .buttonStyle(.plain).accessibilityLabel("Delete sample").disabled(!active || mode == .empty || busy)
                    }
                    if let error = studio.soundCheckNotice ?? (studio.soundCheckChannelID != nil ? audio.errorMessage : nil) {
                        Text(error).font(.callout).foregroundStyle(.orange).textSelection(.enabled)
                    }
                    Text("Changing the output or effect order clears the sample. Start studio audio again when you finish.")
                        .font(.caption).foregroundStyle(.secondary)
                }.frame(width: 315)
                Divider()
                ScrollView {
                    if let channel {
                        VStack(alignment: .leading, spacing: 22) {
                            Text("Tune your input").font(.headline)
                            effect("Input gain", channel: channel, path: \.gainDB, range: -60...24, suffix: "dB")
                            effect("High-pass", channel: channel, path: \.highPassHz, range: 0...1000, suffix: "Hz")
                            effect("Noise gate", channel: channel, path: \.gateThresholdDB, range: -100...0, suffix: "dB")
                            effect("Compressor", channel: channel, path: \.compressorThresholdDB, range: -60...0, suffix: "dB")
                            effect("Ratio", channel: channel, path: \.compressorRatio, range: 1...20, suffix: ":1")
                            Divider()
                            AudioUnitRackView(studio: studio, channelID: channel.id)
                        }.padding(.trailing, 8)
                    }
                }.frame(width: 285, height: 430)
            }
        }.padding(28).frame(width: 720).tint(accent)
        .onChange(of: outputUID) { _, _ in studio.endSoundCheck(); outputPair = 0 }
        .onChange(of: outputPair) { _, _ in studio.endSoundCheck() }
        .onDisappear { studio.endSoundCheck() }
    }
    private var status: String {
        if busy { return "Connecting…" }
        if !active { return "Ready to record" }
        switch mode {
        case .empty: return "Ready to record"
        case .recording: return "Recording your input"
        case .ready: return "Sample ready"
        case .playing: return "Playing in a loop"
        }
    }
    private var duration: String { String(format: "%.1f / 30 s", snapshot?.duration ?? 0) }
    private func effect(_ title: String, channel: MixerChannel, path: WritableKeyPath<ChannelEffects, Float>, range: ClosedRange<Float>, suffix: String) -> some View {
        let binding = Binding<Float>(get: {
            studio.session.channels.first(where: { $0.id == channel.id })?.effects[keyPath: path] ?? channel.effects[keyPath: path]
        }, set: { value in
            guard let index = studio.session.channels.firstIndex(where: { $0.id == channel.id }) else { return }
            studio.session.channels[index].effects[keyPath: path] = value
        })
        return VStack(alignment: .leading, spacing: 8) {
            HStack { Text(title).font(.callout); Spacer(); Text("\(Int(binding.wrappedValue)) \(suffix)").font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary) }
            Slider(value: binding, in: range).accessibilityLabel("Sound Check \(title)")
                .accessibilityValue(audioControlValue(binding.wrappedValue, unit: suffix))
        }
    }
}
