import Foundation

public enum SourceKind: String, Codable, CaseIterable, Sendable {
    case device, application, system, tone
}
public struct InputSource: Codable, Hashable, Sendable {
    public var kind: SourceKind
    public var deviceUID: String?
    public var bundleID: String?
    public var channelOffset: Int
    public init(kind: SourceKind = .device, deviceUID: String? = nil, bundleID: String? = nil, channelOffset: Int = 0) {
        self.kind = kind; self.deviceUID = deviceUID; self.bundleID = bundleID; self.channelOffset = channelOffset
    }
    /// Capture identity ignores fields that do not apply to the selected source kind.
    public var canonicalized: InputSource {
        switch kind {
        case .device: return .init(kind: kind, deviceUID: deviceUID, channelOffset: channelOffset)
        case .application: return .init(kind: kind, bundleID: bundleID)
        case .system, .tone: return .init(kind: kind)
        }
    }
}
public struct ChannelEffects: Codable, Equatable, Sendable {
    public var gainDB: Float = 0
    public var highPassHz: Float = 0
    public var gateThresholdDB: Float = -80
    public var compressorThresholdDB: Float = 0
    public var compressorRatio: Float = 1
    public init() {}
}
public struct MixerChannel: Identifiable, Codable, Equatable, Sendable {
    public var id: UUID
    public var name: String
    public var source: InputSource
    public var additionalSources: [InputSource]
    public var muted: Bool
    public var solo: Bool
    public var effects: ChannelEffects
    public var audioUnitEffects: [AudioUnitEffectSlot]
    public init(id: UUID = UUID(), name: String, source: InputSource = .init(), muted: Bool = false, solo: Bool = false, effects: ChannelEffects = .init(), additionalSources: [InputSource] = [], audioUnitEffects: [AudioUnitEffectSlot] = []) {
        self.id = id; self.name = name; self.source = source; self.muted = muted; self.solo = solo; self.effects = effects
        self.additionalSources = additionalSources; self.audioUnitEffects = audioUnitEffects
    }
    public var allSources: [InputSource] { [source] + additionalSources }
    private enum CodingKeys: String, CodingKey { case id, name, source, muted, solo, effects, additionalSources, audioUnitEffects }
    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(UUID.self, forKey: .id)
        name = try values.decode(String.self, forKey: .name)
        source = try values.decode(InputSource.self, forKey: .source)
        muted = try values.decode(Bool.self, forKey: .muted)
        solo = try values.decode(Bool.self, forKey: .solo)
        effects = try values.decode(ChannelEffects.self, forKey: .effects)
        additionalSources = try values.decodeIfPresent([InputSource].self, forKey: .additionalSources) ?? []
        audioUnitEffects = try values.decodeIfPresent([AudioUnitEffectSlot].self, forKey: .audioUnitEffects) ?? []
    }
}
public struct MixDestination: Codable, Equatable, Sendable {
    public var deviceUID: String?
    public var channelOffset: Int
    public init(deviceUID: String? = nil, channelOffset: Int = 0) {
        self.deviceUID = deviceUID; self.channelOffset = channelOffset
    }
}
public struct MixBus: Identifiable, Codable, Equatable, Sendable {
    public var id: UUID
    public var name: String
    public var deviceUID: String?
    public var channelOffset: Int
    public var gainDB: Float
    public var muted: Bool
    public var additionalOutputs: [MixDestination]
    public init(id: UUID = UUID(), name: String, deviceUID: String? = nil, channelOffset: Int = 0, gainDB: Float = 0, muted: Bool = false, additionalOutputs: [MixDestination] = []) {
        self.id = id; self.name = name; self.deviceUID = deviceUID; self.channelOffset = channelOffset; self.gainDB = gainDB; self.muted = muted
        self.additionalOutputs = additionalOutputs
    }
    public var allOutputs: [MixDestination] { [.init(deviceUID: deviceUID, channelOffset: channelOffset)] + additionalOutputs }
    private enum CodingKeys: String, CodingKey { case id, name, deviceUID, channelOffset, gainDB, muted, additionalOutputs }
    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(UUID.self, forKey: .id)
        name = try values.decode(String.self, forKey: .name)
        deviceUID = try values.decodeIfPresent(String.self, forKey: .deviceUID)
        channelOffset = try values.decode(Int.self, forKey: .channelOffset)
        gainDB = try values.decode(Float.self, forKey: .gainDB)
        muted = try values.decode(Bool.self, forKey: .muted)
        additionalOutputs = try values.decodeIfPresent([MixDestination].self, forKey: .additionalOutputs) ?? []
    }
}
public struct MixSend: Identifiable, Codable, Equatable, Sendable {
    public var id: String { channelID.uuidString + ":" + busID.uuidString }
    public var channelID: UUID
    public var busID: UUID
    public var gainDB: Float
    public var pan: Float
    public var muted: Bool
    public init(channelID: UUID, busID: UUID, gainDB: Float = 0, pan: Float = 0, muted: Bool = false) {
        self.channelID = channelID; self.busID = busID; self.gainDB = gainDB; self.pan = pan; self.muted = muted
    }
}
public struct MixerSession: Codable, Equatable, Sendable {
    public var schemaVersion: Int = 1
    public var name: String
    public var channels: [MixerChannel]
    public var buses: [MixBus]
    public var sends: [MixSend]
    public init(name: String = "My studio", channels: [MixerChannel] = [], buses: [MixBus] = [], sends: [MixSend] = []) {
        self.name = name; self.channels = channels; self.buses = buses; self.sends = sends
    }
    public static var initial: MixerSession { .init(buses: [.init(name: "Personal"), .init(name: "Stream"), .init(name: "Voice chat")]) }
    public mutating func addChannel(_ channel: MixerChannel) {
        channels.append(channel)
        sends += buses.map { MixSend(channelID: channel.id, busID: $0.id, muted: true) }
    }
    public mutating func addBus(_ bus: MixBus) {
        buses.append(bus)
        sends += channels.map { MixSend(channelID: $0.id, busID: bus.id, muted: true) }
    }
    /// Reorder by identity so stale UI actions never target a different row.
    @discardableResult public mutating func moveChannel(_ id: UUID, by distance: Int) -> Bool {
        guard let from = channels.firstIndex(where: { $0.id == id }) else { return false }
        let destination = from.addingReportingOverflow(distance)
        guard !destination.overflow, channels.indices.contains(destination.partialValue) else { return false }
        channels.insert(channels.remove(at: from), at: destination.partialValue)
        return true
    }
    @discardableResult public mutating func moveBus(_ id: UUID, by distance: Int) -> Bool {
        guard let from = buses.firstIndex(where: { $0.id == id }) else { return false }
        let destination = from.addingReportingOverflow(distance)
        guard !destination.overflow, buses.indices.contains(destination.partialValue) else { return false }
        buses.insert(buses.remove(at: from), at: destination.partialValue)
        return true
    }
    public mutating func removeChannel(_ id: UUID) { channels.removeAll { $0.id == id }; sends.removeAll { $0.channelID == id } }
    public mutating func removeBus(_ id: UUID) { buses.removeAll { $0.id == id }; sends.removeAll { $0.busID == id } }
}
