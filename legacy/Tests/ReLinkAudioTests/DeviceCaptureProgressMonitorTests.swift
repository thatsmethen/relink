import Testing
@testable import RaveLinkAudio

@Suite struct DeviceCaptureProgressMonitorTests {
    private func capture(_ id: String, _ frames: UInt64, rate: Double = 48_000, tap: Bool = false) -> CaptureProgress {
        .init(id: id, frames: frames, sampleRate: rate, isProcessTap: tap)
    }

    @Test func anotherDeviceAndIdleApplicationCannotHideStalledCapture() {
        var monitor = DeviceCaptureProgressMonitor()
        #expect(monitor.observe([capture("stalled", 256), capture("advancing", 256), capture("app", 0, tap: true)], at: 0, maxFrames: 16_384) == nil)
        #expect(monitor.observe([capture("stalled", 256), capture("advancing", 512), capture("app", 0, tap: true)], at: 4.9, maxFrames: 16_384) == nil)
        #expect(monitor.observe([capture("stalled", 256), capture("advancing", 768), capture("app", 0, tap: true)], at: 5.1, maxFrames: 16_384) == "stalled")
    }

    @Test func silentDeviceProgressIsHealthyAndApplicationPauseIsExempt() {
        var monitor = DeviceCaptureProgressMonitor()
        for second in 0..<30 {
            // Progress is measured in frames, independent of input amplitude.
            #expect(monitor.observe([capture("silent", UInt64(second) * 48_000), capture("paused-app", 128, tap: true)],
                                    at: Double(second), maxFrames: 16_384) == nil)
        }
        #expect(monitor.observe([capture("paused-app", 128, tap: true)], at: 500, maxFrames: 16_384) == nil)
    }

    @Test func neverStartedCallbackHasBoundedGraceAndFreshGraphResetsIt() {
        var monitor = DeviceCaptureProgressMonitor()
        let input = capture("device", 0)
        #expect(monitor.observe([input], at: 100, maxFrames: 16_384) == nil)
        #expect(monitor.observe([input], at: 105, maxFrames: 16_384) == nil)
        #expect(monitor.observe([input], at: 105.01, maxFrames: 16_384) == "device")
        monitor.reset()
        #expect(monitor.observe([input], at: 200, maxFrames: 16_384) == nil)
        #expect(monitor.observe([], at: 300, maxFrames: 16_384) == nil)
        #expect(monitor.observe([input], at: 400, maxFrames: 16_384) == nil)
    }

    @Test func largestSupportedDeviceBlockReceivesFourCompleteDurations() {
        var monitor = DeviceCaptureProgressMonitor()
        let input = capture("slow", 0, rate: 8000)
        #expect(monitor.observe([input], at: 0, maxFrames: 16_384) == nil)
        #expect(monitor.observe([input], at: 8.19, maxFrames: 16_384) == nil)
        #expect(monitor.observe([input], at: 8.2, maxFrames: 16_384) == "slow")
    }
}
