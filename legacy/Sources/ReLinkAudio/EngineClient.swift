import Foundation
import Combine
import Darwin
import RaveLinkCore

public struct EngineTimeouts: Sendable {
    public var discovery: TimeInterval
    public var start: TimeInterval
    public var stop: TimeInterval
    public var heartbeat: TimeInterval
    public init(discovery: TimeInterval = 10, start: TimeInterval = 12, stop: TimeInterval = 2, heartbeat: TimeInterval = 3) {
        self.discovery = max(0.05, min(discovery, 60)); self.start = max(0.05, min(start, 60)); self.stop = max(0.05, min(stop, 10))
        self.heartbeat = max(0.1, min(heartbeat, 30))
    }
}
private enum EngineEvent: Sendable { case launched(Int32), message(EngineResponse), failed(String), exited(Int32) }
private final class EngineClientBundleMarker: NSObject {}

/// GUI-facing supervisor. No HAL query, device callback, or blocking pipe write
/// occurs in this process's main actor. Failed helpers never resume playback.
@MainActor public final class EngineClient: ObservableObject {
    @Published public private(set) var devices: [AudioDeviceInfo] = []
    @Published public private(set) var applications: [AudioApplicationInfo] = []
    @Published public private(set) var running = false
    @Published public private(set) var starting = false
    @Published public private(set) var errorMessage: String?
    @Published public private(set) var channelMeters: [UUID: Float] = [:]
    @Published public private(set) var busMeters: [UUID: Float] = [:]
    @Published public private(set) var diagnostics = AudioDiagnostics()
    @Published public private(set) var soundCheck: SoundCheckSnapshot?
    @Published public private(set) var helperProcessID: Int32?
    private let helperURL: URL?
    private let timeouts: EngineTimeouts
    private var transport: EngineTransport?
    private var retiringTransport: EngineTransport?
    private var connection = UUID()
    private var generation: UInt64 = 0
    private var sessionRequest: UUID?
    private var sessionOperation: EngineOperation?
    private var discoveryRequest: UUID?
    private var updateTask: Task<Void, Never>?
    private var latestUpdate: MixerSession?
    private var updateIdentity: UUID?
    private var soundCheckChannelID: UUID?
    private var soundCheckExcludedApplications: [String] = []
    private var powerObserver: AudioPowerObserver?
    private var sleeping = false

    public init(helperURL: URL? = EngineClient.defaultHelperURL(), timeouts: EngineTimeouts = .init(), autoDiscover: Bool = true) {
        self.helperURL = helperURL; self.timeouts = timeouts
        powerObserver = AudioPowerObserver { [weak self] event in
            Task { @MainActor [weak self] in self?.handlePowerEvent(event) }
        }
        if autoDiscover { refreshDevices() }
    }
    deinit { updateTask?.cancel(); transport?.cancel() }
    nonisolated public static func defaultHelperURL(bundle: Bundle = .main) -> URL? {
        HelperResolution.resolve(.engine,
            locations: [HelperResolution.Location(bundle), HelperResolution.Location(Bundle(for: EngineClientBundleMarker.self))],
            command: CommandLine.arguments.first.map { URL(fileURLWithPath: $0) })
    }
    public func refreshDevices() {
        guard !sleeping, discoveryRequest == nil else { return }
        do {
            try ensureTransport()
            let request = EngineRequest(generation: generation, operation: .discover)
            discoveryRequest = request.id; transport?.enqueue(request, timeout: timeouts.discovery)
        } catch { fail(error.localizedDescription) }
    }
    public func start(session: MixerSession) throws {
        try start(session: session, soundCheckChannelID: nil)
    }
    public func startSoundCheck(session: MixerSession, channelID: UUID, excludedApplications: [String] = []) throws {
        try validateSoundCheckSession(session, channelID: channelID)
        try start(session: session, soundCheckChannelID: channelID, excludedApplications: validatedExcludedApplications(excludedApplications))
    }
    public func soundCheckCommand(_ command: SoundCheckCommand) throws {
        guard running, soundCheckChannelID != nil else {
            throw AudioRoutingError.configuration("Start Sound Check before controlling its clip.")
        }
        generation &+= 1
        let request = EngineRequest(generation: generation, operation: .soundCheckCommand, soundCheckCommand: command)
        sessionRequest = request.id; sessionOperation = .soundCheckCommand
        transport?.enqueue(request, timeout: timeouts.stop)
    }
    private func start(session: MixerSession, soundCheckChannelID: UUID?, excludedApplications: [String] = []) throws {
        guard !sleeping else { throw AudioRoutingError.configuration("Audio is paused while this Mac sleeps. Start audio again after it wakes.") }
        _ = try session.validated()
        let operation: EngineOperation = soundCheckChannelID == nil ? .start : .startSoundCheck
        // Verify wire size before replacing a working helper.
        _ = try EngineWire.encode(EngineRequest(generation: generation, operation: operation, session: session, soundCheckChannelID: soundCheckChannelID, excludedApplications: soundCheckChannelID == nil ? nil : excludedApplications))
        cancelUpdates()
        replaceTransport()
        generation &+= 1; starting = true; running = false; errorMessage = nil
        self.soundCheckChannelID = soundCheckChannelID; soundCheck = nil
        soundCheckExcludedApplications = excludedApplications
        channelMeters = [:]; busMeters = [:]; diagnostics = .init()
        do {
            try ensureTransport()
            let request = EngineRequest(generation: generation, operation: operation, session: session, soundCheckChannelID: soundCheckChannelID, excludedApplications: soundCheckChannelID == nil ? nil : excludedApplications)
            sessionRequest = request.id; sessionOperation = operation
            transport?.enqueue(request, timeout: timeouts.start)
        } catch { fail(error.localizedDescription); throw error }
    }
    public func update(session: MixerSession) throws {
        _ = try session.validated()
        guard running || starting else { return }
        // An edit during a pending start invalidates that process immediately.
        // A newly spawned helper receives the complete latest snapshot.
        if starting {
            if let soundCheckChannelID { try validateSoundCheckSession(session, channelID: soundCheckChannelID) }
            let operation: EngineOperation = soundCheckChannelID == nil ? .start : .startSoundCheck
            let request = EngineRequest(generation: generation &+ 1, operation: operation, session: session, soundCheckChannelID: soundCheckChannelID, excludedApplications: soundCheckChannelID == nil ? nil : soundCheckExcludedApplications)
            _ = try EngineWire.encode(request)
            if transport?.replaceQueuedStart(request, timeout: timeouts.start) == true {
                generation = request.generation; sessionRequest = request.id; sessionOperation = operation
            } else { try start(session: session, soundCheckChannelID: soundCheckChannelID, excludedApplications: soundCheckExcludedApplications) }
            return
        }
        latestUpdate = session
        guard updateTask == nil else { return }
        let identity = UUID(); updateIdentity = identity
        updateTask = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(25))
                guard !Task.isCancelled, let self, self.running, let latest = self.latestUpdate else { break }
                self.latestUpdate = nil
                self.generation &+= 1
                let request = EngineRequest(generation: self.generation, operation: .update, session: latest)
                self.sessionRequest = request.id; self.sessionOperation = .update
                self.transport?.enqueue(request, timeout: self.timeouts.start)
            }
            if let self, self.updateIdentity == identity { self.updateTask = nil; self.updateIdentity = nil }
        }
    }
    public func stop() {
        cancelUpdates()
        generation &+= 1; running = false; starting = false; channelMeters = [:]; busMeters = [:]; soundCheck = nil; soundCheckChannelID = nil
        guard let transport else { return }
        let request = EngineRequest(generation: generation, operation: .stop)
        sessionRequest = request.id; sessionOperation = .stop
        transport.enqueue(request, timeout: timeouts.stop)
    }
    /// Application termination and tests use this to close and reap the helper.
    public func shutdown() {
        cancelUpdates()
        generation &+= 1; running = false; starting = false; channelMeters = [:]; busMeters = [:]; soundCheck = nil; soundCheckChannelID = nil
        sessionRequest = nil; sessionOperation = nil; discoveryRequest = nil
        transport?.cancel(); retiringTransport = transport ?? retiringTransport; transport = nil
        connection = UUID(); helperProcessID = nil
    }
    private func replaceTransport() {
        transport?.cancel(); retiringTransport = transport ?? retiringTransport; transport = nil
        connection = UUID(); helperProcessID = nil; discoveryRequest = nil; sessionRequest = nil; sessionOperation = nil
    }
    private func ensureTransport() throws {
        guard transport == nil else { return }
        guard let helperURL, FileManager.default.isExecutableFile(atPath: helperURL.path) else {
            throw AudioRoutingError.configuration("The audio engine helper is missing. Reinstall RaveLink or build RaveLinkEngine beside the app.")
        }
        connection = UUID(); helperProcessID = nil
        let identity = connection
        let next = EngineTransport(executable: helperURL, heartbeatTimeout: timeouts.heartbeat) { [weak self] event in
            Task { @MainActor [weak self] in
                guard let self, self.connection == identity else { return }
                self.receive(event)
            }
        }
        let predecessor = retiringTransport; retiringTransport = nil
        transport = next; next.launch(after: predecessor)
    }
    private func receive(_ event: EngineEvent) {
        switch event {
        case .launched(let pid): helperProcessID = pid
        case .exited(let pid): if helperProcessID == pid { helperProcessID = nil }
        case .failed(let message): fail(message)
        case .message(let response): receive(response)
        }
    }
    // The transport still consumes every validated heartbeat. UI subscribers
    // only need values that changed; @Published emits even for equal assignments.
    func receive(_ response: EngineResponse) {
        if response.id == discoveryRequest { discoveryRequest = nil }
        guard response.generation == generation else { return }
        if devices != response.snapshot.devices { devices = response.snapshot.devices }
        if applications != response.snapshot.applications { applications = response.snapshot.applications }
        let isSessionReply = response.kind == .reply && response.id == sessionRequest
        if isSessionReply { sessionRequest = nil; sessionOperation = nil }
        if response.kind == .reply && !response.success {
            fail(userFacing(response.errorMessage ?? "The audio engine rejected this route.")); return
        }
        if sessionOperation != .start && sessionOperation != .startSoundCheck && sessionOperation != .stop {
            if running != response.snapshot.running { running = response.snapshot.running }
            if starting != response.snapshot.starting { starting = response.snapshot.starting }
        }
        if channelMeters != response.snapshot.channelMeters { channelMeters = response.snapshot.channelMeters }
        if busMeters != response.snapshot.busMeters { busMeters = response.snapshot.busMeters }
        if diagnostics != response.snapshot.diagnostics { diagnostics = response.snapshot.diagnostics }
        if sessionOperation != .stop {
            if response.snapshot.running, soundCheckChannelID != nil, response.snapshot.soundCheck == nil {
                fail("The audio engine stopped reporting its Sound Check clip and was stopped."); return
            }
            if let check = response.snapshot.soundCheck, check.channelID != soundCheckChannelID {
                fail("The audio engine returned Sound Check data for an unexpected input and was stopped."); return
            }
            if soundCheck != response.snapshot.soundCheck { soundCheck = response.snapshot.soundCheck }
        }
        if let message = response.errorMessage {
            // A local HAL watchdog indicates that this helper is unusable;
            // terminate it so the next manual start gets a fresh process.
            if !response.snapshot.running && !response.snapshot.starting { fail(userFacing(message)) }
            else if errorMessage != message { errorMessage = message }
        } else if isSessionReply, errorMessage != nil { errorMessage = nil }
    }
    private func fail(_ message: String) {
        running = false; starting = false; errorMessage = message
        channelMeters = [:]; busMeters = [:]; sessionRequest = nil; sessionOperation = nil; discoveryRequest = nil
        soundCheck = nil; soundCheckChannelID = nil
        cancelUpdates()
        transport?.cancel(); retiringTransport = transport ?? retiringTransport; transport = nil
        connection = UUID(); helperProcessID = nil
    }
    private func userFacing(_ message: String) -> String {
        if message.contains("Quit") || message.contains("reopen RaveLink") {
            return "A selected audio device stopped responding. The audio engine was stopped; select another device or start audio again to launch a fresh engine."
        }
        return message
    }
    private func cancelUpdates() {
        updateTask?.cancel(); updateTask = nil; latestUpdate = nil; updateIdentity = nil
    }
    func handlePowerEvent(_ event: AudioPowerEvent) {
        switch event {
        case .sleep:
            sleeping = true
            let wasActive = running || starting
            shutdown() // Close/terminate now; a blocked HAL stop cannot delay sleep.
            if wasActive { errorMessage = "Audio stopped because this Mac is sleeping. Start audio again after it wakes; temporary Sound Check audio was discarded." }
        case .wake:
            sleeping = false
            refreshDevices() // Discovery only. Session audio never resumes itself.
        }
    }
}

/// A single worker owns Process and nonblocking descriptors. Only the bounded
/// command mailbox/cancellation flag cross threads; audio callbacks never use it.
private final class EngineTransport: @unchecked Sendable {
    private struct Queued { let request: EngineRequest; let deadline: UInt64 }
    private let executable: URL
    private let heartbeatTimeout: UInt64
    private let event: @Sendable (EngineEvent) -> Void
    private let lock = NSLock()
    private var cancelled = false
    private var commands: [Queued] = []
    private var pid: Int32?
    private var launching = false
    private let finished = DispatchGroup()
    init(executable: URL, heartbeatTimeout: TimeInterval, event: @escaping @Sendable (EngineEvent) -> Void) {
        self.executable = executable; self.heartbeatTimeout = UInt64(heartbeatTimeout * 1_000_000_000); self.event = event; finished.enter()
    }
    func launch(after predecessor: EngineTransport?) {
        DispatchQueue.global(qos: .userInitiated).async {
            predecessor?.finished.wait()
            defer { self.finished.leave() }
            self.run()
        }
    }
    func enqueue(_ request: EngineRequest, timeout: TimeInterval) {
        lock.lock(); defer { lock.unlock() }
        guard !cancelled else { return }
        if request.operation == .update { commands.removeAll { $0.request.operation == .update } }
        if request.operation == .stop { commands.removeAll { $0.request.operation != .discover } }
        guard commands.count < 8 else { cancelled = true; event(.failed("Too many pending audio commands. The engine was stopped.")); return }
        commands.append(Queued(request: request, deadline: DispatchTime.now().uptimeNanoseconds + UInt64(timeout * 1_000_000_000)))
    }
    /// While a successor waits for its predecessor to exit, keep one latest
    /// snapshot. Once launching begins, changing a pending start needs a new
    /// process because the old request could already reach its native worker.
    func replaceQueuedStart(_ request: EngineRequest, timeout: TimeInterval) -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard !cancelled, !launching else { return false }
        commands.removeAll { $0.request.operation == .start || $0.request.operation == .startSoundCheck || $0.request.operation == .update }
        commands.append(Queued(request: request, deadline: DispatchTime.now().uptimeNanoseconds + UInt64(timeout * 1_000_000_000)))
        return true
    }
    func cancel() {
        lock.lock(); cancelled = true; commands.removeAll()
        if let pid { _ = kill(pid, SIGTERM) }
        lock.unlock()
    }
    private var isCancelled: Bool { lock.lock(); defer { lock.unlock() }; return cancelled }
    private func next() -> Queued? { lock.lock(); defer { lock.unlock() }; return commands.isEmpty ? nil : commands.removeFirst() }
    private func queuedTimeout(_ now: UInt64) -> EngineOperation? { lock.lock(); defer { lock.unlock() }; return commands.first { $0.deadline <= now }?.request.operation }
    private func run() {
        lock.lock()
        if cancelled { lock.unlock(); return }
        launching = true; lock.unlock()
        let process = Process(), input = Pipe(), output = Pipe(), errors = Pipe()
        process.executableURL = executable; process.standardInput = input; process.standardOutput = output; process.standardError = errors
        let exited = DispatchSemaphore(value: 0)
        process.terminationHandler = { _ in exited.signal() }
        do { try process.run() }
        catch { event(.failed("Could not launch the audio engine: \(error.localizedDescription)")); return }
        lock.lock(); pid = process.processIdentifier; let wasCancelled = cancelled; lock.unlock()
        event(.launched(process.processIdentifier))
        if wasCancelled { process.terminate() }
        try? input.fileHandleForReading.close(); try? output.fileHandleForWriting.close(); try? errors.fileHandleForWriting.close()
        let writeFD = input.fileHandleForWriting.fileDescriptor, readFD = output.fileHandleForReading.fileDescriptor, errorFD = errors.fileHandleForReading.fileDescriptor
        for fd in [writeFD, readFD, errorFD] { _ = fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK) }
        _ = fcntl(writeFD, F_SETNOSIGPIPE, 1)
        defer {
            try? input.fileHandleForWriting.close()
            if process.isRunning { process.terminate() }
            if exited.wait(timeout: .now() + .milliseconds(250)) == .timedOut {
                _ = kill(process.processIdentifier, SIGKILL)
                // Reaping stays off the UI thread. SIGKILL cannot be ignored by
                // a helper blocked in a HAL call or stuck in its own main loop.
                exited.wait()
            }
            process.waitUntilExit()
            lock.lock(); pid = nil; lock.unlock()
            try? output.fileHandleForReading.close(); try? errors.fileHandleForReading.close()
            event(.exited(process.processIdentifier))
        }
        var framing = EngineLineBuffer(), buffer = [UInt8](repeating: 0, count: 8192)
        var writing: Data?, writeOffset = 0
        var pending: [UUID: (deadline: UInt64, operation: EngineOperation, generation: UInt64)] = [:]
        var repliesThisSecond = 0, windowStart = DispatchTime.now().uptimeNanoseconds
        var lastRunningHeartbeat: UInt64?, latestResponseGeneration: UInt64 = 0, latestIssuedGeneration: UInt64 = 0
        do {
            while !isCancelled {
                if !process.isRunning {
                    throw AudioRoutingError.configuration("The audio engine exited unexpectedly (status \(process.terminationStatus)). Your session is saved; start audio again to launch a fresh engine.")
                }
                let now = DispatchTime.now().uptimeNanoseconds
                if let heartbeat = lastRunningHeartbeat, now - heartbeat >= heartbeatTimeout {
                    throw AudioRoutingError.configuration("The audio engine stopped reporting its running state and was stopped. Start audio again to launch a fresh engine.")
                }
                if now - windowStart >= 1_000_000_000 { repliesThisSecond = 0; windowStart = now }
                if let operation = pending.values.first(where: { $0.deadline <= now })?.operation ?? queuedTimeout(now) {
                    let action = operation == .discover ? "device discovery" : operation == .stop ? "stop" : "audio routing"
                    throw AudioRoutingError.configuration("The audio engine did not finish \(action) in time and was stopped. Select another device or start audio again to launch a fresh engine.")
                }
                if writing == nil, let command = next() {
                    writing = try EngineWire.encode(command.request); writeOffset = 0
                    latestIssuedGeneration = max(latestIssuedGeneration, command.request.generation)
                    pending[command.request.id] = (command.deadline, command.request.operation, command.request.generation)
                    guard pending.count <= 64 else { throw EngineProtocolError.malformed }
                }
                if let bytes = writing {
                    let count = bytes.withUnsafeBytes { Darwin.write(writeFD, $0.baseAddress!.advanced(by: writeOffset), bytes.count - writeOffset) }
                    if count > 0 { writeOffset += count; if writeOffset == bytes.count { writing = nil } }
                    else if count < 0 && errno != EAGAIN && errno != EINTR { throw AudioRoutingError.configuration("The audio engine closed its control channel and was stopped.") }
                }
                for _ in 0..<32 {
                    let count = Darwin.read(readFD, &buffer, buffer.count)
                    if count == 0 { throw AudioRoutingError.configuration("The audio engine closed its response channel and was stopped.") }
                    if count < 0 { if errno == EAGAIN || errno == EINTR { break }; throw EngineProtocolError.malformed }
                    for line in try framing.append(Data(buffer.prefix(count))) {
                        repliesThisSecond += 1
                        guard repliesThisSecond <= 1000 else { throw EngineProtocolError.malformed }
                        let response = try EngineWire.response(line)
                        guard response.generation <= latestIssuedGeneration else { throw EngineProtocolError.malformed }
                        if response.kind == .reply {
                            guard let id = response.id, let expected = pending.removeValue(forKey: id), expected.generation == response.generation else { throw EngineProtocolError.malformed }
                        }
                        // A stale generation must not mask a hung current graph.
                        if response.generation >= max(latestResponseGeneration, latestIssuedGeneration) {
                            latestResponseGeneration = response.generation
                            lastRunningHeartbeat = response.snapshot.running ? now : nil
                        }
                        event(.message(response))
                    }
                }
                // Discard bounded stderr chunks; arbitrary native/plugin logs are
                // never parsed as protocol messages or shown as trusted UI text.
                for _ in 0..<8 { if Darwin.read(errorFD, &buffer, buffer.count) <= 0 { break } }
                usleep(2000)
            }
        } catch { if !isCancelled { event(.failed(error.localizedDescription)) } }
    }
}
