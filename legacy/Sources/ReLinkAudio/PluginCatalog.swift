import Foundation
import RaveLinkCore
import Darwin

public typealias AudioUnitComponentID = RaveLinkCore.AudioUnitComponentID
public typealias AudioUnitDescriptor = RaveLinkCore.AudioUnitDescriptor
public struct AudioUnitBusReport: Codable, Equatable, Sendable {
    public let index: Int
    public let name: String?
    public let sampleRate: Double
    public let channels: UInt32
    public let interleaved: Bool
    public let commonFormat: UInt
    public let channelLayoutTag: UInt32?
    public let enabled: Bool
}
public struct AudioUnitParameterReport: Codable, Equatable, Sendable {
    public let address: UInt64
    public let identifier: String
    public let name: String
    public let minimum: Float?
    public let maximum: Float?
    public let initialValue: Float?
    public let declaredDefaultValue: Float?
    public let unit: UInt32
    public let unitName: String?
    public let flags: UInt32
}
public struct AudioUnitProbeReport: Codable, Equatable, Sendable {
    public let descriptor: AudioUnitDescriptor
    public let inputBuses: [AudioUnitBusReport]
    public let outputBuses: [AudioUnitBusReport]
    public let channelCapabilities: [Int]?
    public let parameters: [AudioUnitParameterReport]
    public let requestedSampleRate: Double
    public let requestedChannels: UInt32
    public let configurationAccepted: Bool
    public let renderResourcesAllocated: Bool
    public let configurationError: String?
    public let latencySeconds: Double?
    public let tailSeconds: Double?
    public let limitations: [String]
}
public enum PluginCatalogError: Error, LocalizedError, Equatable {
    case helperUnavailable
    case launchFailed(String)
    case invalidRequest
    case timeout
    case cancelled
    case outputLimit
    case crashed(Int32)
    case failed(Int32)
    case malformedResponse
    case unsupportedProtocol(Int)
    case operationMismatch
    case helperError(String, String)
    public var errorDescription: String? {
        switch self {
        case .helperUnavailable: return "The Audio Unit helper is missing. Reinstall RaveLink or build RaveLinkPluginHost beside the app."
        case .launchFailed(let message): return "Could not start the Audio Unit helper: \(message)"
        case .invalidRequest: return "The Audio Unit request is invalid."
        case .timeout: return "The Audio Unit helper timed out and was stopped."
        case .cancelled: return "Audio Unit inspection was cancelled."
        case .outputLimit: return "The Audio Unit helper exceeded its response limit and was stopped."
        case .crashed(let signal): return "The Audio Unit helper crashed (signal \(signal)). RaveLink is still running."
        case .failed(let status): return "The Audio Unit helper exited with status \(status)."
        case .malformedResponse: return "The Audio Unit helper returned an invalid response."
        case .unsupportedProtocol(let version): return "Unsupported Audio Unit helper protocol: \(version)."
        case .operationMismatch: return "The Audio Unit helper replied to a different request."
        case .helperError(_, let message): return message
        }
    }
}

private struct PluginRequest: Encodable {
    let protocolVersion = 1
    let operation: String
    var component: AudioUnitComponentID?
    var expectedVersion: UInt32?
    var sampleRate: Double?
    var channels: UInt32?
    var samples: [Float]?
    var parameters: [AudioUnitParameterSetting]?
    var state: AudioUnitState?
    var bypassed: Bool?
    var blockFrames: UInt32?
}
struct PluginResponse: Decodable {
    struct Failure: Decodable { let code: String; let message: String }
    let protocolVersion: Int
    let operation: String
    let status: String
    let components: [AudioUnitDescriptor]?
    let probe: AudioUnitProbeReport?
    let render: AudioUnitRenderReport?
    let error: Failure?
    let limitations: [String]?

    static func parse(_ data: Data, operation: String) throws -> PluginResponse {
        guard data.count <= PluginCatalog.maximumResponseBytes,
              let response = try? JSONDecoder().decode(Self.self, from: data) else { throw PluginCatalogError.malformedResponse }
        guard response.protocolVersion == 1 else { throw PluginCatalogError.unsupportedProtocol(response.protocolVersion) }
        guard response.operation == operation else { throw PluginCatalogError.operationMismatch }
        if response.status == "error", let failure = response.error {
            throw PluginCatalogError.helperError(failure.code, String(failure.message.prefix(2048)))
        }
        guard response.status == "ok", response.error == nil else { throw PluginCatalogError.malformedResponse }
        switch operation {
        case "list":
            guard let entries = response.components, response.probe == nil, response.render == nil,
                  entries.count <= 4096, Set(entries.map(\.id)).count == entries.count,
                  entries.allSatisfy({ isEffect($0.component) && !$0.name.isEmpty && $0.name.count <= 512 }) else { throw PluginCatalogError.malformedResponse }
        case "probe":
            guard let probe = response.probe, response.components == nil, response.render == nil,
                  isEffect(probe.descriptor.component), probe.parameters.count <= 4096,
                  probe.inputBuses.count <= 64, probe.outputBuses.count <= 64,
                  probe.requestedSampleRate.isFinite, (8_000...384_000).contains(probe.requestedSampleRate),
                  (1...2).contains(probe.requestedChannels),
                  !probe.renderResourcesAllocated || probe.configurationAccepted,
                  probe.parameters.allSatisfy({ [$0.minimum, $0.maximum, $0.initialValue, $0.declaredDefaultValue].allSatisfy { $0?.isFinite ?? true } }),
                  (probe.inputBuses + probe.outputBuses).allSatisfy({ $0.sampleRate.isFinite && $0.sampleRate > 0 }),
                  probe.latencySeconds?.isFinite ?? true,
                  probe.tailSeconds?.isFinite ?? true else { throw PluginCatalogError.malformedResponse }
        case "render":
            guard let rendered = response.render, response.components == nil, response.probe == nil,
                  isEffect(rendered.descriptor.component), (1...2).contains(rendered.channels),
                  rendered.sampleRate.isFinite, (8_000...192_000).contains(rendered.sampleRate),
                  (1...16_384).contains(rendered.frameCount), (1...512).contains(rendered.blockFrames),
                  rendered.samples.count == rendered.frameCount * Int(rendered.channels),
                  rendered.samples.allSatisfy({ $0.isFinite && abs($0) <= 1_000_000 }),
                  rendered.state?.isValid(for: rendered.descriptor) ?? true,
                  rendered.parameterValues.count <= 4096,
                  Set(rendered.parameterValues.map(\.address)).count == rendered.parameterValues.count,
                  rendered.parameterValues.allSatisfy({ $0.value.isFinite }),
                  rendered.latencySeconds?.isFinite ?? true else { throw PluginCatalogError.malformedResponse }
        default: throw PluginCatalogError.operationMismatch
        }
        return response
    }
}
private func isEffect(_ id: AudioUnitComponentID) -> Bool {
    // Apple's aufx / aumf identifiers. Metadata only; no AU is loaded in this module.
    (id.type == 0x61756678 || id.type == 0x61756d66) && id.subtype != 0 && id.manufacturer != 0
}

private final class PluginCatalogBundleMarker: NSObject {}

public struct PluginCatalog: Sendable {
    public static let maximumResponseBytes = 4 * 1024 * 1024
    public static let maximumRequestBytes = 2 * 1024 * 1024
    public let helperURL: URL?
    public init(helperURL: URL? = PluginCatalog.defaultHelperURL()) { self.helperURL = helperURL }

    public static func defaultHelperURL(bundle: Bundle = .main) -> URL? {
        HelperResolution.resolve(.pluginHost,
            locations: [HelperResolution.Location(bundle), HelperResolution.Location(Bundle(for: PluginCatalogBundleMarker.self))],
            command: CommandLine.arguments.first.map { URL(fileURLWithPath: $0) })
    }
    public func discover(timeout: TimeInterval = 10) async throws -> [AudioUnitDescriptor] {
        let response = try await request(PluginRequest(operation: "list"), timeout: timeout)
        return response.components!.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }
    public func probe(_ descriptor: AudioUnitDescriptor, sampleRate: Double = 48_000, channels: UInt32 = 2, timeout: TimeInterval = 10) async throws -> AudioUnitProbeReport {
        guard isEffect(descriptor.component), sampleRate.isFinite, (8_000...384_000).contains(sampleRate), (1...2).contains(channels) else { throw PluginCatalogError.invalidRequest }
        let request = PluginRequest(operation: "probe", component: descriptor.component, expectedVersion: descriptor.version, sampleRate: sampleRate, channels: channels)
        let response = try await self.request(request, timeout: timeout)
        guard let probe = response.probe, probe.descriptor.component == descriptor.component,
              probe.requestedSampleRate == sampleRate, probe.requestedChannels == channels,
              descriptor.version == nil || probe.descriptor.version == descriptor.version else { throw PluginCatalogError.malformedResponse }
        return probe
    }
    public func render(_ descriptor: AudioUnitDescriptor, samples: [Float], sampleRate: Double = 48_000,
                       channels: UInt32 = 2, parameters: [AudioUnitParameterSetting] = [], state: AudioUnitState? = nil,
                       bypassed: Bool = false, blockFrames: UInt32 = 512, timeout: TimeInterval = 10) async throws -> AudioUnitRenderReport {
        guard isEffect(descriptor.component), sampleRate.isFinite, (8_000...192_000).contains(sampleRate),
              (1...2).contains(channels), !samples.isEmpty, samples.count % Int(channels) == 0,
              samples.count / Int(channels) <= 16_384, samples.allSatisfy({ $0.isFinite && abs($0) <= 16 }),
              (1...512).contains(blockFrames), parameters.count <= 256,
              parameters.allSatisfy({ $0.value.isFinite }), Set(parameters.map(\.address)).count == parameters.count,
              state?.isValid(for: descriptor) ?? true else { throw PluginCatalogError.invalidRequest }
        let query = PluginRequest(operation: "render", component: descriptor.component, expectedVersion: descriptor.version,
                                  sampleRate: sampleRate, channels: channels, samples: samples, parameters: parameters,
                                  state: state, bypassed: bypassed, blockFrames: blockFrames)
        let response = try await request(query, timeout: timeout)
        guard let report = response.render, report.descriptor.component == descriptor.component,
              descriptor.version == nil || report.descriptor.version == descriptor.version,
              report.sampleRate == sampleRate, report.channels == channels, report.samples.count == samples.count,
              report.blockFrames == blockFrames, report.bypassed == bypassed,
              !bypassed || report.samples == samples else { throw PluginCatalogError.malformedResponse }
        return report
    }
    private func request(_ request: PluginRequest, timeout: TimeInterval) async throws -> PluginResponse {
        guard timeout.isFinite, (0.05...60).contains(timeout) else { throw PluginCatalogError.invalidRequest }
        guard let helperURL, FileManager.default.isExecutableFile(atPath: helperURL.path) else { throw PluginCatalogError.helperUnavailable }
        let bytes = try JSONEncoder().encode(request)
        let run = PluginHelperProcess()
        let result = try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<PluginHelperResult, Error>) in
                DispatchQueue.global(qos: .userInitiated).async {
                    do { continuation.resume(returning: try run.execute(helperURL, request: bytes, timeout: timeout)) }
                    catch { continuation.resume(throwing: error) }
                }
            }
        } onCancel: { run.cancel() }
        if result.reason == .uncaughtSignal { throw PluginCatalogError.crashed(result.status) }
        if result.status != 0 {
            // Typed helper failures remain useful; arbitrary crash text stays out of the UI.
            if let response = try? JSONDecoder().decode(PluginResponse.self, from: result.stdout), response.status == "error" {
                _ = try PluginResponse.parse(result.stdout, operation: request.operation)
            }
            throw PluginCatalogError.failed(result.status)
        }
        return try PluginResponse.parse(result.stdout, operation: request.operation)
    }
}

struct PluginHelperResult { let stdout: Data; let status: Int32; let reason: Process.TerminationReason }

// One background worker owns the Process and both nonblocking pipe reads.
// No UI thread waits, pipe-reader tasks, or unbounded output buffers survive a run.
final class PluginHelperProcess: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false
    func cancel() { lock.lock(); cancelled = true; lock.unlock() }
    private var isCancelled: Bool { lock.lock(); defer { lock.unlock() }; return cancelled }

    func execute(_ executable: URL, request: Data, timeout: TimeInterval) throws -> PluginHelperResult {
        if isCancelled { throw PluginCatalogError.cancelled }
        guard request.count <= PluginCatalog.maximumRequestBytes else { throw PluginCatalogError.invalidRequest }
        // A private, immediately unlinked file keeps larger bounded audio/state
        // requests off pipes, so early helper exits cannot SIGPIPE the parent.
        var template = Array((NSTemporaryDirectory() + "ravelink-au-XXXXXX").utf8CString)
        let inputFD = mkstemp(&template)
        guard inputFD >= 0 else { throw PluginCatalogError.launchFailed("Could not create the private helper input.") }
        _ = template.withUnsafeBufferPointer { unlink($0.baseAddress!) }
        let input = FileHandle(fileDescriptor: inputFD, closeOnDealloc: true)
        defer { try? input.close() }
        try input.write(contentsOf: request)
        try input.seek(toOffset: 0)
        if isCancelled { throw PluginCatalogError.cancelled }
        let process = Process(), output = Pipe(), errors = Pipe()
        process.executableURL = executable
        process.standardInput = input; process.standardOutput = output; process.standardError = errors
        let exited = DispatchSemaphore(value: 0)
        process.terminationHandler = { _ in exited.signal() }
        do { try process.run() } catch { throw PluginCatalogError.launchFailed(error.localizedDescription) }
        try? input.close()
        try? output.fileHandleForWriting.close(); try? errors.fileHandleForWriting.close()
        defer {
            try? output.fileHandleForReading.close(); try? errors.fileHandleForReading.close()
        }
        let outFD = output.fileHandleForReading.fileDescriptor, errFD = errors.fileHandleForReading.fileDescriptor
        _ = fcntl(outFD, F_SETFL, fcntl(outFD, F_GETFL) | O_NONBLOCK)
        _ = fcntl(errFD, F_SETFL, fcntl(errFD, F_GETFL) | O_NONBLOCK)
        let started = DispatchTime.now().uptimeNanoseconds
        let budget = UInt64(timeout * 1_000_000_000)
        var stdout = Data(), stderrCount = 0, outEOF = false, errEOF = false
        var buffer = [UInt8](repeating: 0, count: 16_384)

        func drain(_ fd: Int32, retain: Bool) throws -> Bool {
            while true {
                let count = read(fd, &buffer, buffer.count)
                if count > 0 {
                    if retain {
                        guard stdout.count + count <= PluginCatalog.maximumResponseBytes else { throw PluginCatalogError.outputLimit }
                        stdout.append(contentsOf: buffer.prefix(count))
                    } else {
                        stderrCount += count
                        guard stderrCount <= 256 * 1024 else { throw PluginCatalogError.outputLimit }
                    }
                } else if count == 0 { return true }
                else if errno == EAGAIN || errno == EWOULDBLOCK { return false }
                else if errno != EINTR { throw PluginCatalogError.malformedResponse }
            }
        }
        do {
            while true {
                if isCancelled { throw PluginCatalogError.cancelled }
                if DispatchTime.now().uptimeNanoseconds - started >= budget { throw PluginCatalogError.timeout }
                if !outEOF { outEOF = try drain(outFD, retain: true) }
                if !errEOF { errEOF = try drain(errFD, retain: false) }
                if !process.isRunning {
                    if !outEOF { _ = try drain(outFD, retain: true) }
                    if !errEOF { _ = try drain(errFD, retain: false) }
                    return .init(stdout: stdout, status: process.terminationStatus, reason: process.terminationReason)
                }
                var events = [pollfd(fd: outEOF ? -1 : outFD, events: Int16(POLLIN), revents: 0), pollfd(fd: errEOF ? -1 : errFD, events: Int16(POLLIN), revents: 0)]
                _ = poll(&events, nfds_t(events.count), 20)
            }
        } catch { stop(process, exited: exited); throw error }
    }
    private func stop(_ process: Process, exited: DispatchSemaphore) {
        guard process.isRunning else { return }
        process.terminate()
        if exited.wait(timeout: .now() + 0.2) == .timedOut, process.isRunning {
            _ = kill(process.processIdentifier, SIGKILL)
            _ = exited.wait(timeout: .now() + 1)
        }
    }
}
