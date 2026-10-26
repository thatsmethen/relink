import Foundation
import Darwin
import RaveLinkAudio

/// Output is serialized off the main actor. Meter snapshots coalesce; replies are
/// bounded. A parent that stops draining the pipe cannot block engine controls.
private final class WireOutput: @unchecked Sendable {
    let descriptor: Int32
    private let lock = NSLock()
    private var replies: [EngineResponse] = []
    private var state: EngineResponse?
    private var scheduled = false
    private let queue = DispatchQueue(label: "au.textual.ravelink.engine-wire")
    init() {
        descriptor = dup(STDOUT_FILENO)
        _ = fcntl(descriptor, F_SETFL, fcntl(descriptor, F_GETFL) | O_NONBLOCK)
        _ = fcntl(descriptor, F_SETNOSIGPIPE, 1)
        _ = dup2(STDERR_FILENO, STDOUT_FILENO)
    }
    func send(_ response: EngineResponse) {
        lock.lock()
        if response.kind == .state { state = response }
        else {
            guard replies.count < 32 else { lock.unlock(); _exit(70) }
            replies.append(response)
        }
        let start = !scheduled; scheduled = true; lock.unlock()
        if start { queue.async { self.drain() } }
    }
    private func next() -> EngineResponse? {
        lock.lock(); defer { lock.unlock() }
        if !replies.isEmpty { return replies.removeFirst() }
        if let value = state { state = nil; return value }
        scheduled = false; return nil
    }
    private func drain() {
        while let response = next() {
            guard let bytes = try? EngineWire.encode(response) else { _exit(70) }
            var offset = 0
            while offset < bytes.count {
                let count = bytes.withUnsafeBytes { Darwin.write(descriptor, $0.baseAddress!.advanced(by: offset), bytes.count - offset) }
                if count > 0 { offset += count }
                else if count < 0 && (errno == EAGAIN || errno == EINTR) { usleep(1000) }
                else { _exit(0) }
            }
        }
    }
}

@MainActor private final class EngineServer {
    let audio = AudioCoordinator()
    let output: WireOutput
    private var generation: UInt64 = 0
    private var timer: Timer?
    init(output: WireOutput) {
        self.output = output
        timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
            Task { @MainActor in
                guard let self else { return }
                self.output.send(EngineResponse(generation: self.generation, kind: .state,
                    errorMessage: self.audio.errorMessage, snapshot: EngineSnapshot(self.audio)))
            }
        }
    }
    func receive(_ request: EngineRequest) {
        guard request.generation >= generation else { reply(request, error: "The request was superseded."); return }
        if request.operation != .discover { generation = request.generation }
        Task { @MainActor in
            do {
                switch request.operation {
                case .discover:
                    audio.refreshDevices()
                    await audio.finishPendingControlWork()
                    while !audio.inventoryReady { try await Task.sleep(for: .milliseconds(10)) }
                case .start, .startSoundCheck:
                    while !audio.inventoryReady { try await Task.sleep(for: .milliseconds(10)) }
                    guard request.generation == generation else { reply(request, error: "The request was superseded."); return }
                    if request.operation == .startSoundCheck {
                        try audio.startSoundCheck(session: request.session!, channelID: request.soundCheckChannelID!, excludedApplications: request.excludedApplications ?? [])
                    } else { try audio.start(session: request.session!) }
                    while audio.starting && request.generation == generation { try await Task.sleep(for: .milliseconds(10)) }
                    guard request.generation == generation else { reply(request, error: "The request was superseded."); return }
                    if !audio.running { reply(request, error: audio.errorMessage ?? "The audio route did not start."); return }
                case .update:
                    try audio.update(session: request.session!)
                    while audio.starting && request.generation == generation { try await Task.sleep(for: .milliseconds(10)) }
                    guard request.generation == generation else { reply(request, error: "The request was superseded."); return }
                    if let error = audio.errorMessage { reply(request, error: error); return }
                case .stop:
                    audio.stop()
                    await audio.finishPendingControlWork()
                case .soundCheckCommand:
                    try audio.soundCheckCommand(request.soundCheckCommand!)
                }
                reply(request)
            } catch { reply(request, error: error.localizedDescription) }
        }
    }
    private func reply(_ request: EngineRequest, error: String? = nil) {
        output.send(EngineResponse(id: request.id, generation: request.generation, kind: .reply,
            success: error == nil, errorMessage: error, snapshot: EngineSnapshot(audio)))
    }
}

@main struct EngineMain {
    @MainActor static func main() {
        let output = WireOutput()
        let server = EngineServer(output: output)
        let admission = DispatchSemaphore(value: 4)
        DispatchQueue.global(qos: .userInitiated).async {
            var framing = EngineLineBuffer()
            var buffer = [UInt8](repeating: 0, count: 4096)
            while true {
                let count = Darwin.read(STDIN_FILENO, &buffer, buffer.count)
                if count == 0 { _exit(0) } // Parent exited: HAL resources die with this process.
                if count < 0 { if errno == EINTR { continue }; _exit(70) }
                do {
                    for line in try framing.append(Data(buffer.prefix(count))) {
                        let request = try EngineWire.request(line)
                        admission.wait()
                        Task { @MainActor in server.receive(request); admission.signal() }
                    }
                } catch { _exit(70) }
            }
        }
        RunLoop.main.run()
    }
}
