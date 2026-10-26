import Foundation

struct OutputProgress {
    let id: String
    let frames: UInt64
    let sampleRate: Double
}
/// Checks each output independently using monotonic time. A device with the
/// largest supported block/rate receives four complete buffer durations.
struct RenderProgressMonitor {
    private struct State { var frames: UInt64; var advancedAt: Double }
    private var states: [String: State] = [:]
    mutating func observe(_ outputs: [OutputProgress], at now: Double, maxFrames: UInt32) -> String? {
        let ids = Set(outputs.map(\.id))
        states = states.filter { ids.contains($0.key) }
        for output in outputs {
            guard let state = states[output.id], state.frames == output.frames else {
                states[output.id] = .init(frames: output.frames, advancedAt: now); continue
            }
            let limit = max(5, Double(maxFrames) * 4 / max(8000, output.sampleRate))
            if now - state.advancedAt > limit { return output.id }
        }
        return nil
    }
    mutating func reset() { states.removeAll() }
}
