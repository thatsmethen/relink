import Foundation
import RaveLinkCore

public typealias AudioUnitParameterSetting = RaveLinkCore.AudioUnitParameterSetting
public typealias AudioUnitState = RaveLinkCore.AudioUnitState

public struct AudioUnitRenderReport: Codable, Equatable, Sendable {
    public let descriptor: AudioUnitDescriptor
    public let sampleRate: Double
    public let channels: UInt32
    public let frameCount: Int
    public let blockFrames: UInt32
    public let bypassed: Bool
    /// Interleaved Float32 samples, with exactly frameCount * channels elements.
    public let samples: [Float]
    public let state: AudioUnitState?
    public let parameterValues: [AudioUnitParameterSetting]
    public let latencySeconds: Double?
    public let limitations: [String]
}
