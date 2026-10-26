import Testing
import RaveLinkRealtime

@Suite struct DSPRecoveryTests {
    @Test func finiteOverflowSpikeDoesNotPoisonFollowingAudio() throws {
        let damaged = try #require(rl_dsp_create(48000))
        let reference = try #require(rl_dsp_create(48000))
        defer { rl_dsp_destroy(damaged); rl_dsp_destroy(reference) }
        for dsp in [damaged, reference] { rl_dsp_controls(dsp, 24, 100, -80, -20, 4) }
        var warmA = [Float](repeating: 0.1, count: 1024), warmB = warmA
        rl_dsp_process(damaged, &warmA, 512); rl_dsp_process(reference, &warmB, 512)
        var spike: [Float] = [.greatestFiniteMagnitude, -.greatestFiniteMagnitude]
        var sanitized: [Float] = [0, 0]
        rl_dsp_process(damaged, &spike, 1); rl_dsp_process(reference, &sanitized, 1)
        #expect(spike == sanitized)
        var actual = [Float](repeating: 0, count: 4096)
        for frame in 0..<2048 { actual[frame * 2] = frame % 16 < 8 ? 0.1 : -0.1; actual[frame * 2 + 1] = -actual[frame * 2] }
        var expected = actual
        rl_dsp_process(damaged, &actual, 2048); rl_dsp_process(reference, &expected, 2048)
        #expect(actual.allSatisfy { $0.isFinite })
        #expect(actual == expected)
        #expect(actual.contains { abs($0) > 0.01 })
    }
    @Test func malformedControlUpdatesPreservePriorValidSettings() throws {
        let dsp = try #require(rl_dsp_create(48000))
        defer { rl_dsp_destroy(dsp) }
        rl_dsp_controls(dsp, 0, 0, -80, 0, 1)
        for value: Float in [.nan, .infinity, -.infinity] {
            rl_dsp_controls(dsp, value, 0, -80, 0, 1)
            rl_dsp_controls(dsp, 24, value, -80, 0, 1)
            rl_dsp_controls(dsp, 24, 0, value, 0, 1)
            rl_dsp_controls(dsp, 24, 0, -80, value, 1)
            rl_dsp_controls(dsp, 24, 0, -80, 0, value)
        }
        var samples = [Float](repeating: 0.1, count: 4096)
        rl_dsp_process(dsp, &samples, 2048)
        #expect(samples.allSatisfy { $0 == 0.1 })
    }
}
