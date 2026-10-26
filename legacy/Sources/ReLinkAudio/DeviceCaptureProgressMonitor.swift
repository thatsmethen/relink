import Foundation

struct CaptureProgress {
    /// A unique capture node, not the logical input or shared device UID.
    let id: String
    let frames: UInt64
    let sampleRate: Double
    let isProcessTap: Bool
}

/// A started device IOProc must continue returning buffers, even for silence.
/// Process taps are exempt: an application that is not playing may stop callbacks.
/// Shared logical sources contribute only their one underlying capture node.
struct DeviceCaptureProgressMonitor {
    private var monitor = RenderProgressMonitor()

    mutating func observe(_ captures: [CaptureProgress], at now: Double, maxFrames: UInt32) -> String? {
        monitor.observe(captures.filter { !$0.isProcessTap }.map {
            OutputProgress(id: $0.id, frames: $0.frames, sampleRate: $0.sampleRate)
        }, at: now, maxFrames: maxFrames)
    }

    mutating func reset() { monitor.reset() }
}
