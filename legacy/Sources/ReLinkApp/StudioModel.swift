import AppKit
import Combine
import RaveLinkCore
import RaveLinkAudio

@MainActor final class StudioModel: ObservableObject {
    @Published var session: MixerSession { didSet {
        if !StudioIndex.sameStructure(oldValue, session) { index = StudioIndex(session) }
        scheduleSave(previous: oldValue)
    } }
    @Published var selectedChannel: UUID?
    @Published var notice: String?
    @Published var saveStatus = "Saved locally"
    @Published var showingInput = false
    @Published var showingMix = false
    @Published var sourceTarget: UUID?
    @Published var sourceReplacement: Int?
    @Published var outputTarget: UUID?
    @Published var showingOutputs = false
    @Published var showingSettings = false
    @Published var showingDiagnostics = false
    @Published var showingSoundCheck = false
    @Published var soundCheckChannelID: UUID?
    @Published private(set) var preparingSoundCheck = false
    @Published var soundCheckNotice: String?
    let audio: EngineClient
    private let store: SessionStore
    private var pending: Task<Void, Never>?
    private var allowsOverwrite = true
    private var audition: MixerSession?
    private var auditionTask: Task<Void, Never>?
    private var index = StudioIndex()
    init(store: SessionStore = .standard, audio: EngineClient? = nil) {
        self.store = store; self.audio = audio ?? EngineClient()
        if FileManager.default.fileExists(atPath: store.url.path) {
            do { session = try store.load() }
            catch {
                session = .initial
                let failure = error.localizedDescription
                let backup = store.url.deletingLastPathComponent().appendingPathComponent("unreadable-\(UUID().uuidString).json")
                do {
                    try FileManager.default.copyItem(at: store.url, to: backup)
                    notice = "Your saved studio could not be opened. A copy has been preserved at \(backup.path). \(failure)"
                } catch {
                    allowsOverwrite = false
                    saveStatus = "Recovery needed · use Export studio"
                    notice = "Your saved studio could not be opened or backed up. Automatic saving is disabled to preserve it. Use Export studio to save changes elsewhere. \(failure)"
                }
            }
        } else { session = .initial }
        index = StudioIndex(session)
    }
    func channel(_ id: UUID) -> MixerChannel? { index.channels[id].map { session.channels[$0] } }
    func bus(_ id: UUID) -> MixBus? { index.buses[id].map { session.buses[$0] } }
    func send(_ id: String) -> MixSend? { index.sends[id].map { session.sends[$0] } }
    func route(channelID: UUID, busID: UUID) -> MixSend? {
        index.routes[.init(channelID: channelID, busID: busID)].map { session.sends[$0] }
    }
    func setChannel<Value>(_ id: UUID, _ path: WritableKeyPath<MixerChannel, Value>, to value: Value) {
        guard let offset = index.channels[id] else { return }
        session.channels[offset][keyPath: path] = value
    }
    func setBus<Value>(_ id: UUID, _ path: WritableKeyPath<MixBus, Value>, to value: Value) {
        guard let offset = index.buses[id] else { return }
        session.buses[offset][keyPath: path] = value
    }
    func setSend<Value>(_ id: String, _ path: WritableKeyPath<MixSend, Value>, to value: Value) {
        guard let offset = index.sends[id] else { return }
        session.sends[offset][keyPath: path] = value
    }
    private func audioSnapshot(_ value: MixerSession) -> MixerSession {
        var snapshot = value
        snapshot.name = "Studio"
        for index in snapshot.channels.indices { snapshot.channels[index].name = "Input" }
        for index in snapshot.buses.indices { snapshot.buses[index].name = "Mix" }
        return snapshot
    }
    private func scheduleSave(previous: MixerSession) {
        pending?.cancel()
        if allowsOverwrite { saveStatus = "Saving…" }
        if (audio.running || audio.starting) && audioSnapshot(previous) != audioSnapshot(session) {
            do {
                if var audition {
                    if let id = soundCheckChannelID, let channel = session.channels.first(where: { $0.id == id }) {
                        let previousAudition = audition
                        var input = channel; input.muted = false; input.solo = false
                        audition.channels = [input]
                        if audioSnapshot(audition) != audioSnapshot(previousAudition) {
                            self.audition = audition
                            try audio.update(session: audioSnapshot(audition))
                        }
                    } else { endSoundCheck() }
                } else { try audio.update(session: audioSnapshot(session)) }
            }
            catch { notice = error.localizedDescription }
        }
        pending = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled, let self, allowsOverwrite else { return }
            do {
                _ = try session.validated()
                try store.save(session)
                saveStatus = "Saved locally"
            } catch { saveStatus = "Not saved: \(error.localizedDescription)" }
        }
    }
    func flush() { pending?.cancel(); guard allowsOverwrite else { return }; do { try store.save(session) } catch { notice = error.localizedDescription } }
    func toggleAudio() {
        if showingSoundCheck {
            endSoundCheck()
            soundCheckNotice = "Use Record sample to start Sound Check. Close this window before starting studio audio."
            return
        }
        if audition != nil { endSoundCheck(); return }
        if audio.running || audio.starting { audio.stop() }
        else { do { try audio.start(session: session) } catch { notice = error.localizedDescription } }
    }
    func openSoundCheck(_ id: UUID) {
        endSoundCheck(); audio.stop()
        soundCheckChannelID = id; soundCheckNotice = nil; showingSoundCheck = true
    }
    func recordSoundCheck(output: MixDestination) {
        guard let id = soundCheckChannelID, let channel = session.channels.first(where: { $0.id == id }) else { return }
        soundCheckNotice = nil
        if audio.soundCheck?.channelID == id && audio.running {
            commandSoundCheck(.record); return
        }
        auditionTask?.cancel()
        do {
            let check = try SoundCheckSession.make(channel: channel, output: output)
            audition = check; preparingSoundCheck = true
            let excluded = Array(Set(session.channels.flatMap(\.allSources).filter { $0.kind == .application }.compactMap(\.bundleID))).sorted()
            try audio.startSoundCheck(session: check, channelID: id, excludedApplications: excluded)
            auditionTask = Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    while audio.starting { try await Task.sleep(for: .milliseconds(25)) }
                    try Task.checkCancellation()
                    guard audio.running, audio.soundCheck?.channelID == id else {
                        throw SessionError.invalid(audio.errorMessage ?? "Sound Check could not connect to the selected devices.")
                    }
                    try audio.soundCheckCommand(.record)
                } catch { if !Task.isCancelled { soundCheckNotice = error.localizedDescription } }
                if !Task.isCancelled { preparingSoundCheck = false }
            }
        } catch { preparingSoundCheck = false; soundCheckNotice = error.localizedDescription }
    }
    func commandSoundCheck(_ command: SoundCheckCommand) {
        do { try audio.soundCheckCommand(command); soundCheckNotice = nil }
        catch { soundCheckNotice = error.localizedDescription }
    }
    func endSoundCheck() {
        auditionTask?.cancel(); auditionTask = nil; preparingSoundCheck = false
        if audition != nil { audio.stop() }
        audition = nil
    }
    func beginInput(target: UUID? = nil, replacing: Int? = nil) {
        sourceTarget = target; sourceReplacement = replacing; showingInput = true
    }
    /// Publish structural edits once, only after the entire candidate is valid.
    private func editValidated(_ edit: (inout MixerSession) throws -> Void) throws {
        var candidate = session
        try edit(&candidate)
        session = try candidate.validated()
    }
    func add(_ input: MixerChannel) throws {
        try editValidated { candidate in
            if let target = sourceTarget {
                guard let i = candidate.channels.firstIndex(where: { $0.id == target }) else {
                    throw SessionError.invalid("This input was removed. Close this sheet and choose another input.")
                }
                if let slot = sourceReplacement {
                    if slot == 0 { candidate.channels[i].source = input.source.canonicalized }
                    else if candidate.channels[i].additionalSources.indices.contains(slot - 1) {
                        candidate.channels[i].additionalSources[slot - 1] = input.source.canonicalized
                    } else { throw SessionError.invalid("This source was removed. Close this sheet and choose another source.") }
                } else { candidate.channels[i].additionalSources.append(input.source.canonicalized) }
            } else {
                var channel = input
                channel.source = channel.source.canonicalized
                channel.additionalSources = channel.additionalSources.map(\.canonicalized)
                candidate.addChannel(channel)
            }
        }
        selectedChannel = sourceTarget
    }
    func add(_ bus: MixBus) throws { try editValidated { $0.addBus(bus) } }
    func removeChannel(_ id: UUID) {
        if selectedChannel == id { selectedChannel = nil }
        var candidate = session; candidate.removeChannel(id); session = candidate
    }
    func moveChannel(_ id: UUID, by distance: Int) {
        var candidate = session
        if candidate.moveChannel(id, by: distance) { session = candidate }
    }
    func moveBus(_ id: UUID, by distance: Int) {
        var candidate = session
        if candidate.moveBus(id, by: distance) { session = candidate }
    }
    func removeBus(_ id: UUID) {
        var candidate = session; candidate.removeBus(id); session = candidate
    }
    func removeSource(_ source: InputSource, from channelID: UUID) {
        var candidate = session
        guard let i = candidate.channels.firstIndex(where: { $0.id == channelID }),
              candidate.channels[i].allSources.count > 1 else { return }
        if candidate.channels[i].source == source {
            candidate.channels[i].source = candidate.channels[i].additionalSources.removeFirst()
        } else if let slot = candidate.channels[i].additionalSources.firstIndex(of: source) {
            candidate.channels[i].additionalSources.remove(at: slot)
        } else { return }
        session = candidate
    }
    func setPrimaryOutput(_ uid: String?, on busID: UUID) throws {
        try editValidated { candidate in
            guard let i = candidate.buses.firstIndex(where: { $0.id == busID }) else { return }
            candidate.buses[i].deviceUID = uid
            candidate.buses[i].channelOffset = 0
        }
    }
    func setOutputPair(_ offset: Int, on busID: UUID) throws {
        try editValidated { candidate in
            guard let i = candidate.buses.firstIndex(where: { $0.id == busID }) else { return }
            candidate.buses[i].channelOffset = offset
        }
    }
    func addOutput(_ output: MixDestination, to busID: UUID) throws {
        try editValidated { candidate in
            guard let i = candidate.buses.firstIndex(where: { $0.id == busID }) else {
                throw SessionError.invalid("This mix was removed. Close this sheet and choose another mix.")
            }
            candidate.buses[i].additionalOutputs.append(output)
        }
    }
    func removeOutput(_ output: MixDestination, from busID: UUID) {
        var candidate = session
        guard let i = candidate.buses.firstIndex(where: { $0.id == busID }),
              let slot = candidate.buses[i].additionalOutputs.firstIndex(of: output) else { return }
        candidate.buses[i].additionalOutputs.remove(at: slot)
        session = candidate
    }
    func editAudioUnits(on channelID: UUID, _ edit: (inout [AudioUnitEffectSlot]) throws -> Void) throws {
        try editValidated { candidate in
            guard let channel = candidate.channels.firstIndex(where: { $0.id == channelID }) else {
                throw SessionError.invalid("This input was removed. Close the effect editor and choose another input.")
            }
            try edit(&candidate.channels[channel].audioUnitEffects)
        }
    }
    func exportSession() {
        let panel = NSSavePanel(); panel.nameFieldStringValue = session.name + ".ravelink.json"; panel.allowedContentTypes = [.json]
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do { try SessionStore(url: url).save(session) } catch { notice = error.localizedDescription }
    }
    func importSession() {
        let panel = NSOpenPanel(); panel.allowedContentTypes = [.json]; panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let imported = try SessionStore(url: url).load()
            endSoundCheck(); showingSoundCheck = false
            audio.stop(); session = imported; selectedChannel = nil
        } catch { notice = "Could not import studio. \(error.localizedDescription)" }
    }
}
