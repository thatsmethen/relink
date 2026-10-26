import SwiftUI
import AudioToolbox
import RaveLinkCore
import RaveLinkAudio

struct AudioUnitRackView: View {
    @ObservedObject var studio: StudioModel
    let channelID: UUID
    @State private var showingBrowser = false
    @State private var editing: AudioUnitEffectSlot?
    @State private var error: String?
    private var slots: [AudioUnitEffectSlot] {
        studio.session.channels.first(where: { $0.id == channelID })?.audioUnitEffects ?? []
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Audio Units").font(.headline)
            Text("Effects run in order after the built-in processing.").font(.caption).foregroundStyle(.secondary)
            ForEach(slots) { slot in
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Text(slot.descriptor.name).font(.callout.weight(.medium)).lineLimit(2)
                        Spacer(minLength: 2)
                        Menu {
                            Button("Edit parameters…") { editing = slot }
                            Button("Move earlier") { move(slot, by: -1) }.disabled(slots.first?.id == slot.id)
                            Button("Move later") { move(slot, by: 1) }.disabled(slots.last?.id == slot.id)
                            Divider()
                            Button("Remove effect", role: .destructive) { mutate { $0.removeAll { $0.id == slot.id } } }
                        } label: { Label("Options for effect \(slot.descriptor.name)", systemImage: "ellipsis").labelStyle(.iconOnly) }.menuStyle(.borderlessButton).frame(width: 20)
                    }
                    HStack {
                        Toggle("Enabled", isOn: Binding(get: { !(slots.first { $0.id == slot.id }?.isBypassed ?? true) }, set: { enabled in
                            mutate { effects in
                                guard let i = effects.firstIndex(where: { $0.id == slot.id }) else { return }
                                effects[i].isBypassed = !enabled
                            }
                        })).toggleStyle(.switch).controlSize(.small).accessibilityLabel("Enable \(slot.descriptor.name)")
                        Spacer()
                        Button("Edit") { editing = slot }.buttonStyle(.bordered).controlSize(.small).accessibilityLabel("Edit \(slot.descriptor.name)")
                    }
                }.padding(12).background(.white.opacity(0.045), in: RoundedRectangle(cornerRadius: 8))
            }
            if let error { Text(error).font(.caption).foregroundStyle(.orange) }
            Button { showingBrowser = true } label: { Label("Add Audio Unit", systemImage: "plus") }.buttonStyle(.bordered)
        }
        .sheet(isPresented: $showingBrowser) { AudioUnitBrowser(studio: studio, channelID: channelID) }
        .sheet(item: $editing) { AudioUnitParameterEditor(studio: studio, channelID: channelID, initialSlot: $0) }
    }
    private func mutate(_ edit: (inout [AudioUnitEffectSlot]) throws -> Void) {
        do { try studio.editAudioUnits(on: channelID, edit); error = nil }
        catch { self.error = error.localizedDescription }
    }
    private func move(_ slot: AudioUnitEffectSlot, by delta: Int) {
        mutate { effects in
            guard let index = effects.firstIndex(where: { $0.id == slot.id }), effects.indices.contains(index + delta) else { return }
            effects.swapAt(index, index + delta)
        }
    }
}

private struct AudioUnitBrowser: View {
    @ObservedObject var studio: StudioModel
    let channelID: UUID
    @Environment(\.dismiss) private var dismiss
    @State private var components: [AudioUnitDescriptor] = []
    @State private var selection: String?
    @State private var query = ""
    @State private var loading = true
    @State private var preparing = false
    @State private var error: String?
    @State private var preparation: Task<Void, Never>?
    private var visible: [AudioUnitDescriptor] { components.filter { query.isEmpty || $0.name.localizedCaseInsensitiveContains(query) } }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Add Audio Unit").font(.title2.weight(.semibold))
            Text("Choose an installed effect for this input. Its settings will be saved with your studio.").font(.callout).foregroundStyle(.secondary)
            TextField("Search effects", text: $query).textFieldStyle(.roundedBorder).disabled(preparing)
            if loading { ProgressView("Finding installed effects…").frame(maxWidth: .infinity, minHeight: 240) }
            else if visible.isEmpty {
                ContentUnavailableView(query.isEmpty ? "No Audio Units found" : "No matching effects", systemImage: "slider.horizontal.3")
                    .frame(maxWidth: .infinity, minHeight: 240)
            } else {
                List(visible, selection: $selection) { effect in
                    Text(effect.name).padding(.vertical, 5).tag(effect.id)
                }.frame(height: 280).disabled(preparing)
            }
            if let error { Text(error).font(.callout).foregroundStyle(.orange).textSelection(.enabled) }
            HStack {
                if preparing { ProgressView().controlSize(.small); Text("Preparing effect…").font(.caption).foregroundStyle(.secondary) }
                Spacer()
                Button("Cancel") { preparation?.cancel(); dismiss() }.keyboardShortcut(.cancelAction)
                Button("Add effect") { prepare() }.keyboardShortcut(.defaultAction).buttonStyle(AccentButtonStyle())
                    .disabled(preparing || selection == nil || loading)
            }
        }.padding(28).frame(width: 540)
        .task {
            do { components = try await PluginCatalog().discover() }
            catch { if !Task.isCancelled { self.error = error.localizedDescription } }
            loading = false
        }
        .onDisappear { preparation?.cancel() }
    }
    private func prepare() {
        guard let descriptor = components.first(where: { $0.id == selection }) else { return }
        preparing = true; error = nil
        preparation = Task { @MainActor in
            do {
                let probe = try await PluginCatalog().probe(descriptor)
                guard probe.configurationAccepted && probe.renderResourcesAllocated else {
                    throw SessionError.invalid(probe.configurationError ?? "This effect does not accept stereo audio at 48 kHz.")
                }
                let rendered = try await PluginCatalog().render(descriptor, samples: [Float](repeating: 0, count: 1024))
                try Task.checkCancellation()
                let writable = Set(probe.parameters.filter { $0.flags & AudioUnitParameterOptions.flag_IsWritable.rawValue != 0 }.map(\.address))
                let parameters = rendered.parameterValues.filter { writable.contains($0.address) }
                let slot = AudioUnitEffectSlot(descriptor: descriptor, state: rendered.state, parameters: parameters)
                try studio.editAudioUnits(on: channelID) { $0.append(slot) }
                dismiss()
            } catch { if !Task.isCancelled { self.error = error.localizedDescription } }
            preparing = false
        }
    }
}

private struct AudioUnitParameterEditor: View {
    @ObservedObject var studio: StudioModel
    let channelID: UUID
    let initialSlot: AudioUnitEffectSlot
    @Environment(\.dismiss) private var dismiss
    @State private var probe: AudioUnitProbeReport?
    @State private var error: String?
    private var slot: AudioUnitEffectSlot? {
        studio.session.channels.first(where: { $0.id == channelID })?.audioUnitEffects.first { $0.id == initialSlot.id }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text(initialSlot.descriptor.name).font(.title2.weight(.semibold))
            Text("Changes are saved with your studio.").font(.callout).foregroundStyle(.secondary)
            if let probe {
                if let latency = probe.latencySeconds { Text(String(format: "Reported default latency: %.2f ms at 48 kHz", latency * 1000)).font(.caption).foregroundStyle(.secondary) }
                ScrollView {
                    VStack(alignment: .leading, spacing: 22) {
                        ForEach(probe.parameters, id: \.address) { parameter in
                            if let lower = parameter.minimum, let upper = parameter.maximum, lower < upper {
                                VStack(alignment: .leading, spacing: 8) {
                                    HStack {
                                        Text(parameter.name).font(.callout)
                                        Spacer()
                                        Text(value(parameter), format: .number.precision(.fractionLength(0...5)))
                                            .font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary)
                                        if let unit = parameter.unitName, !unit.isEmpty { Text(unit).font(.caption).foregroundStyle(.secondary) }
                                    }
                                    Slider(value: Binding(get: { min(upper, max(lower, value(parameter))) }, set: { set(parameter, value: $0) }), in: lower...upper)
                                        .accessibilityLabel(parameter.name)
                                        .accessibilityValue("\(value(parameter).formatted(.number.precision(.fractionLength(0...5)))) \(parameter.unitName ?? "")")
                                        .disabled(parameter.flags & AudioUnitParameterOptions.flag_IsWritable.rawValue == 0 || slot == nil)
                                }
                            }
                        }
                        if probe.parameters.isEmpty { Text("This effect exposes no adjustable parameters.").foregroundStyle(.secondary) }
                    }.padding(.vertical, 6)
                }.frame(maxHeight: 360)
            } else if error == nil { ProgressView("Loading effect controls…").frame(maxWidth: .infinity, minHeight: 120) }
            if let error { Text(error).foregroundStyle(.orange).font(.callout).textSelection(.enabled) }
            HStack { Spacer(); Button("Done") { dismiss() }.keyboardShortcut(.defaultAction).buttonStyle(AccentButtonStyle()) }
        }.padding(28).frame(width: 560)
        .task {
            do { probe = try await PluginCatalog().probe(initialSlot.descriptor) }
            catch { if !Task.isCancelled { self.error = error.localizedDescription } }
        }
    }
    private func value(_ parameter: AudioUnitParameterReport) -> Float {
        slot?.parameters.first(where: { $0.address == parameter.address })?.value ?? parameter.initialValue ?? parameter.minimum ?? 0
    }
    private func set(_ parameter: AudioUnitParameterReport, value: Float) {
        do {
            try studio.editAudioUnits(on: channelID) { effects in
                guard let index = effects.firstIndex(where: { $0.id == initialSlot.id }) else { return }
                effects[index].parameters.removeAll { $0.address == parameter.address }
                effects[index].parameters.append(.init(address: parameter.address, value: value))
                effects[index].parameters.sort { $0.address < $1.address }
            }
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}
