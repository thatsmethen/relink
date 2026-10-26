import Foundation
import RaveLinkCore

public enum EngineWire {
    public static let version = 2
    public static let maximumLineBytes = 4 * 1024 * 1024
    public static func encode<T: Encodable>(_ value: T) throws -> Data {
        var data = try JSONEncoder().encode(value)
        guard data.count < maximumLineBytes else { throw EngineProtocolError.lineLimit }
        data.append(0x0a); return data
    }
    public static func request(_ line: Data) throws -> EngineRequest {
        guard line.count <= maximumLineBytes else { throw EngineProtocolError.lineLimit }
        let request = try JSONDecoder().decode(EngineRequest.self, from: line)
        guard request.version == version else { throw EngineProtocolError.version }
        switch request.operation {
        case .start, .update:
            guard let session = request.session, request.soundCheckChannelID == nil, request.soundCheckCommand == nil, request.excludedApplications == nil else { throw EngineProtocolError.malformed }
            _ = try session.validated()
        case .startSoundCheck:
            guard let session = request.session, let channelID = request.soundCheckChannelID, request.soundCheckCommand == nil else { throw EngineProtocolError.malformed }
            try validateSoundCheckSession(session, channelID: channelID)
            _ = try validatedExcludedApplications(request.excludedApplications ?? [])
        case .soundCheckCommand:
            guard request.session == nil, request.soundCheckChannelID == nil, request.soundCheckCommand != nil, request.excludedApplications == nil else { throw EngineProtocolError.malformed }
        case .discover, .stop:
            guard request.session == nil, request.soundCheckChannelID == nil, request.soundCheckCommand == nil, request.excludedApplications == nil else { throw EngineProtocolError.malformed }
        }
        return request
    }
    public static func response(_ line: Data) throws -> EngineResponse {
        guard line.count <= maximumLineBytes else { throw EngineProtocolError.lineLimit }
        let response = try JSONDecoder().decode(EngineResponse.self, from: line)
        guard response.version == version else { throw EngineProtocolError.version }
        guard response.kind != .reply || response.id != nil,
              response.kind != .state || response.id == nil,
              response.errorMessage?.utf8.count ?? 0 <= 8192,
              !(response.snapshot.running && response.snapshot.starting),
              response.snapshot.diagnostics.effectAlignmentLatencySeconds.isFinite,
              (0...2).contains(response.snapshot.diagnostics.effectAlignmentLatencySeconds),
              response.snapshot.devices.allSatisfy({ !$0.id.isEmpty && $0.id.utf8.count <= 4096 && $0.name.utf8.count <= 4096 && $0.sampleRate.isFinite && $0.sampleRate >= 0 && $0.inputChannels >= 0 && $0.outputChannels >= 0 }),
              response.snapshot.applications.allSatisfy({ !$0.id.isEmpty && $0.id.utf8.count <= 4096 && $0.name.utf8.count <= 4096 }),
              Set(response.snapshot.devices.map(\.id)).count == response.snapshot.devices.count,
              Set(response.snapshot.applications.map(\.id)).count == response.snapshot.applications.count,
              response.snapshot.channelMeters.values.allSatisfy({ $0.isFinite && $0 >= 0 }),
              response.snapshot.busMeters.values.allSatisfy({ $0.isFinite && $0 >= 0 }) else { throw EngineProtocolError.malformed }
        if let check = response.snapshot.soundCheck {
            guard check.sampleRate.isFinite, (8000...384000).contains(check.sampleRate),
                  check.maximumSeconds.isFinite, (0...30).contains(check.maximumSeconds), check.maximumSeconds > 0,
                  Double(check.recordedFrames) <= ceil(check.sampleRate * check.maximumSeconds),
                  check.peak.isFinite, check.peak >= 0,
                  response.snapshot.running || response.snapshot.starting,
                  check.mode != .empty || check.recordedFrames == 0,
                  (check.mode != .playing && check.mode != .ready) || check.recordedFrames > 0 else { throw EngineProtocolError.malformed }
        }
        return response
    }
}
public enum EngineProtocolError: Error, LocalizedError {
    case lineLimit, version, malformed
    public var errorDescription: String? {
        switch self {
        case .lineLimit: return "The audio engine message exceeds its 4 MiB limit. Reduce the routing graph."
        case .version: return "The audio engine version does not match RaveLink. Reinstall the application."
        case .malformed: return "The audio engine sent an invalid response and was stopped."
        }
    }
}
public enum EngineOperation: String, Codable, Sendable { case discover, start, update, stop, startSoundCheck, soundCheckCommand }
public struct EngineRequest: Codable, Sendable {
    public let version: Int
    public let id: UUID
    public let generation: UInt64
    public let operation: EngineOperation
    public let session: MixerSession?
    public let soundCheckChannelID: UUID?
    public let soundCheckCommand: SoundCheckCommand?
    public let excludedApplications: [String]?
    public init(id: UUID = UUID(), generation: UInt64, operation: EngineOperation, session: MixerSession? = nil,
                soundCheckChannelID: UUID? = nil, soundCheckCommand: SoundCheckCommand? = nil, excludedApplications: [String]? = nil) {
        version = EngineWire.version; self.id = id; self.generation = generation; self.operation = operation; self.session = session
        self.soundCheckChannelID = soundCheckChannelID; self.soundCheckCommand = soundCheckCommand
        self.excludedApplications = excludedApplications
    }
}
public struct EngineSnapshot: Codable, Sendable {
    public var devices: [AudioDeviceInfo] = []
    public var applications: [AudioApplicationInfo] = []
    public var running = false
    public var starting = false
    public var channelMeters: [UUID: Float] = [:]
    public var busMeters: [UUID: Float] = [:]
    public var diagnostics = AudioDiagnostics()
    public var soundCheck: SoundCheckSnapshot?
    public init() {}
    @MainActor public init(_ audio: AudioCoordinator) {
        devices = audio.devices; applications = audio.applications
        running = audio.running; starting = audio.starting
        channelMeters = audio.channelMeters; busMeters = audio.busMeters; diagnostics = audio.diagnostics
        soundCheck = audio.soundCheck
    }
}
public struct EngineResponse: Codable, Sendable {
    public enum Kind: String, Codable, Sendable { case reply, state }
    public let version: Int
    public let id: UUID?
    public let generation: UInt64
    public let kind: Kind
    public let success: Bool
    public let errorMessage: String?
    public let snapshot: EngineSnapshot
    public init(id: UUID? = nil, generation: UInt64, kind: Kind, success: Bool = true, errorMessage: String? = nil, snapshot: EngineSnapshot) {
        version = EngineWire.version; self.id = id; self.generation = generation; self.kind = kind; self.success = success
        self.errorMessage = errorMessage.map { String($0.prefix(2048)) }; self.snapshot = snapshot
    }
}

/// Bounded incremental framing. A partial line can never grow past the limit,
/// including when a malicious peer writes without newline delimiters.
public struct EngineLineBuffer {
    private var partial = Data()
    public init() {}
    public mutating func append(_ data: Data) throws -> [Data] {
        var lines: [Data] = [], start = data.startIndex
        while start < data.endIndex {
            let newline = data[start...].firstIndex(of: 0x0a)
            let end = newline ?? data.endIndex
            guard partial.count + data.distance(from: start, to: end) <= EngineWire.maximumLineBytes else { throw EngineProtocolError.lineLimit }
            partial.append(data[start..<end])
            guard let newline else { break }
            guard !partial.isEmpty else { throw EngineProtocolError.malformed }
            lines.append(partial); partial = Data()
            start = data.index(after: newline)
        }
        return lines
    }
    public var hasPartialLine: Bool { !partial.isEmpty }
}
