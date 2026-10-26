import Foundation
import RaveLinkRealtime

struct RealtimeDeviceBuffer: Equatable {
    let minimum: UInt32
    let maximum: UInt32
}

enum RealtimeDeviceLimits {
    static var bufferGuidance: String {
        "Choose a buffer of at most 16,384 frames in the device's control panel, or select another device, then start audio again."
    }
    static func validateRate(_ rate: Double) throws {
        guard rl_maximum_callback_frames(rate) > 0 else {
            throw AudioRoutingError.configuration("This device's sample rate is unsupported. Choose a rate from 8 to 384 kHz before starting audio.")
        }
    }
    static func validateBuffer(rate: Double, frames: UInt32) throws {
        try validateRate(rate)
        guard frames > 0, frames <= rl_maximum_callback_frames(rate) else {
            throw AudioRoutingError.configuration("This device reports an unsupported audio buffer. \(bufferGuidance)")
        }
    }
}
