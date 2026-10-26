import Testing
@testable import RaveLinkAudio

@Suite struct RenderProgressMonitorTests {
    @Test func progressingOutputCannotHideAnotherStalledDevice() {
        var monitor = RenderProgressMonitor()
        #expect(monitor.observe([.init(id: "a", frames: 1, sampleRate: 48_000), .init(id: "b", frames: 1, sampleRate: 48_000)], at: 0, maxFrames: 16_384) == nil)
        #expect(monitor.observe([.init(id: "a", frames: 2, sampleRate: 48_000), .init(id: "b", frames: 1, sampleRate: 48_000)], at: 4.9, maxFrames: 16_384) == nil)
        #expect(monitor.observe([.init(id: "a", frames: 3, sampleRate: 48_000), .init(id: "b", frames: 1, sampleRate: 48_000)], at: 5.1, maxFrames: 16_384) == "b")
    }
    @Test func maximumBufferDurationAndGraphResetAreRespected() {
        var monitor = RenderProgressMonitor()
        let output = OutputProgress(id: "slow", frames: 1, sampleRate: 8_000)
        #expect(monitor.observe([output], at: 0, maxFrames: 16_384) == nil)
        #expect(monitor.observe([output], at: 8.1, maxFrames: 16_384) == nil)
        #expect(monitor.observe([output], at: 8.3, maxFrames: 16_384) == "slow")
        monitor.reset()
        #expect(monitor.observe([output], at: 20, maxFrames: 16_384) == nil)
        #expect(monitor.observe([], at: 50, maxFrames: 16_384) == nil)
        #expect(monitor.observe([output], at: 51, maxFrames: 16_384) == nil)
    }
}
