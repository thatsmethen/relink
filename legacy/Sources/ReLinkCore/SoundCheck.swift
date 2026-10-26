import Foundation

public enum SoundCheckCommand: String, Codable, Sendable { case record, stop, play, clear }
public enum SoundCheckMode: String, Codable, Sendable { case empty, recording, ready, playing }
public struct SoundCheckSnapshot: Codable, Equatable, Sendable {
    public var channelID: UUID
    public var mode: SoundCheckMode
    public var recordedFrames: UInt64
    public var sampleRate: Double
    public var maximumSeconds: Double
    public var peak: Float
    public init(channelID: UUID, mode: SoundCheckMode = .empty, recordedFrames: UInt64 = 0,
                sampleRate: Double = 48_000, maximumSeconds: Double = 30, peak: Float = 0) {
        self.channelID = channelID; self.mode = mode; self.recordedFrames = recordedFrames
        self.sampleRate = sampleRate; self.maximumSeconds = maximumSeconds; self.peak = peak
    }
    public var duration: Double { sampleRate > 0 ? Double(recordedFrames) / sampleRate : 0 }
}

/// An audition has exactly one explicit destination. No studio send or mute
/// setting can redirect the clip into a broadcast mix.
public enum SoundCheckSession {
    public static func make(channel: MixerChannel, output: MixDestination) throws -> MixerSession {
        guard let uid = output.deviceUID, !uid.isEmpty else {
            throw SessionError.invalid("Choose where you want to listen before starting Sound Check.")
        }
        var input = channel
        input.muted = false; input.solo = false
        let bus = MixBus(name: "Sound Check", deviceUID: uid, channelOffset: output.channelOffset)
        return try MixerSession(name: "Sound Check", channels: [input], buses: [bus],
            sends: [.init(channelID: input.id, busID: bus.id)]).validated()
    }
}
