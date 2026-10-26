import Foundation

public enum SessionError: LocalizedError {
    case invalid(String)
    public var errorDescription: String? { if case let .invalid(message) = self { return message }; return nil }
}
public extension MixerSession {
    func validated() throws -> MixerSession {
        func require(_ condition: Bool, _ message: String) throws { if !condition { throw SessionError.invalid(message) } }
        try require(schemaVersion == 1, "This session was saved by an unsupported version of RaveLink.")
        try require(!name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, "Give the session a name.")
        try require(Set(channels.map(\.id)).count == channels.count, "Duplicate input identifiers.")
        try require(Set(buses.map(\.id)).count == buses.count, "Duplicate mix identifiers.")
        try require(Set(sends.map(\.id)).count == sends.count, "Duplicate routes.")
        let channelIDs = Set(channels.map(\.id)), busIDs = Set(buses.map(\.id))
        let routeCount = channels.count.multipliedReportingOverflow(by: buses.count)
        try require(!routeCount.overflow && sends.count == routeCount.partialValue, "The routing matrix is incomplete. Every input must have a route to every mix, including muted routes.")
        for channel in channels {
            try require(!channel.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, "Input names cannot be empty.")
            let sources = channel.allSources
            try require(Set(sources.map(\.canonicalized)).count == sources.count, "An input group contains the same source more than once.")
            for source in sources {
                try require(source.channelOffset >= 0 && source.channelOffset < 4096, "Input channel offset is invalid.")
            }
            try require(Set(channel.audioUnitEffects.map(\.id)).count == channel.audioUnitEffects.count, "An input contains duplicate Audio Unit slot identifiers.")
            for slot in channel.audioUnitEffects {
                let descriptor = slot.descriptor, component = descriptor.component
                try require([UInt32(0x61756678), UInt32(0x61756d66)].contains(component.type) && component.subtype != 0 && component.manufacturer != 0,
                            "An Audio Unit effect identifier is invalid.")
                try require(!descriptor.name.isEmpty && descriptor.name.count <= 512, "An Audio Unit effect name is invalid.")
                try require(slot.state?.isValid(for: descriptor) ?? true, "An Audio Unit state exceeds 256 KiB or does not match its component/version.")
                try require(slot.parameters.count <= 256 && Set(slot.parameters.map(\.address)).count == slot.parameters.count && slot.parameters.allSatisfy({ $0.value.isFinite }),
                            "Audio Unit parameters must be unique finite values, with at most 256 assignments per effect.")
            }
            let e = channel.effects
            try require(e.gainDB.isFinite && (-60...24).contains(e.gainDB), "Input gain must be between −60 and +24 dB.")
            try require(e.highPassHz.isFinite && (0...2000).contains(e.highPassHz), "High-pass frequency is invalid.")
            try require(e.gateThresholdDB.isFinite && (-100...0).contains(e.gateThresholdDB), "Gate threshold is invalid.")
            try require(e.compressorThresholdDB.isFinite && (-60...0).contains(e.compressorThresholdDB), "Compressor threshold is invalid.")
            try require(e.compressorRatio.isFinite && (1...20).contains(e.compressorRatio), "Compressor ratio is invalid.")
        }
        for bus in buses {
            try require(!bus.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, "Mix names cannot be empty.")
            var occupiedChannels: [String: Set<Int>] = [:]
            for output in bus.allOutputs {
                try require(output.channelOffset >= 0 && output.channelOffset < 4096, "Output channel offset is invalid.")
                guard let uid = output.deviceUID else { continue }
                let pair: Set<Int> = [output.channelOffset, output.channelOffset + 1]
                try require(occupiedChannels[uid, default: []].isDisjoint(with: pair), "A mix cannot send to overlapping channel pairs on the same output device.")
                occupiedChannels[uid, default: []].formUnion(pair)
            }
            try require(bus.gainDB.isFinite && (-60...12).contains(bus.gainDB), "Mix gain is invalid.")
        }
        for send in sends {
            try require(channelIDs.contains(send.channelID) && busIDs.contains(send.busID), "A route refers to a missing input or mix.")
            try require(send.gainDB.isFinite && (-60...12).contains(send.gainDB), "Route gain is invalid.")
            try require(send.pan.isFinite && (-1...1).contains(send.pan), "Pan must be between −1 and +1.")
        }
        var normalized = self
        for index in normalized.channels.indices {
            normalized.channels[index].source = normalized.channels[index].source.canonicalized
            normalized.channels[index].additionalSources = normalized.channels[index].additionalSources.map(\.canonicalized)
        }
        return normalized
    }
}

public struct SessionStore {
    public static let maximumDataBytes = 32 * 1024 * 1024
    public let url: URL
    public init(url: URL) { self.url = url }
    public static var standard: SessionStore {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return .init(url: base.appendingPathComponent("RaveLink/session.json"))
    }
    public func load() throws -> MixerSession {
        let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
        if let size = attributes[.size] as? NSNumber, size.uint64Value > UInt64(Self.maximumDataBytes) {
            throw Self.sizeLimitError
        }
        // Bound the actual read too: a file can grow between the size check and read.
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var data = Data()
        while let chunk = try handle.read(upToCount: min(65_536, Self.maximumDataBytes + 1 - data.count)), !chunk.isEmpty {
            data.append(chunk)
            guard data.count <= Self.maximumDataBytes else { throw Self.sizeLimitError }
        }
        return try JSONDecoder().decode(MixerSession.self, from: data).validated()
    }
    public func save(_ session: MixerSession) throws {
        let encoder = JSONEncoder(); encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = try encoder.encode(session.validated())
        guard data.count <= Self.maximumDataBytes else { throw Self.sizeLimitError }
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url, options: .atomic)
    }
    private static var sizeLimitError: SessionError { .invalid("Session exceeds the 32 MB safety limit.") }
}
