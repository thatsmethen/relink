import Testing
import RaveLinkRealtime

@Suite struct ElasticClockTests {
    /// Virtual events call the actual C FIFO/SRC, without sleeping or opening IO.
    private func schedule(ppm: Double, inputFrames: UInt32, outputFrames: [UInt32], seconds: Double) throws {
        let ring = try #require(rl_ring_create(65536))
        defer { rl_ring_destroy(ring) }
        #expect(rl_ring_prepare_clock(ring, 8000, 8000))
        let input = [Float](repeating: 0.125, count: Int(inputFrames) * 2)
        var output = [Float](repeating: 0, count: Int(outputFrames.max()!) * 2)
        var inputTime = 0.0, outputTime = 0.0, primed = false
        var lateEmpty = 0, partial = 0, invalid = 0, index = 0
        while outputTime < seconds {
            if inputTime <= outputTime {
                input.withUnsafeBufferPointer { _ = rl_ring_write(ring, $0.baseAddress, inputFrames) }
                inputTime += Double(inputFrames) / (8000 * (1 + ppm / 1_000_000))
            } else {
                let frames = outputFrames[index % outputFrames.count]; index += 1
                let produced = output.withUnsafeMutableBufferPointer { rl_ring_read(ring, $0.baseAddress, frames, 1) }
                if produced > 0 {
                    primed = true
                    if produced < frames { partial += 1 }
                    if output.prefix(Int(produced) * 2).contains(where: { !$0.isFinite || abs($0 - 0.125) > 0.00001 }) { invalid += 1 }
                } else if primed { lateEmpty += 1 }
                outputTime += Double(frames) / 8000
            }
        }
        #expect(primed)
        #expect(lateEmpty == 0 && partial == 0 && invalid == 0)
        #expect(rl_ring_underruns(ring) == 0)
        #expect(rl_ring_dropped(ring) == 0)
    }

    @Test(arguments: [-1000.0, 1000.0]) func bothClockDirectionsRemainContinuous(ppm: Double) throws {
        try schedule(ppm: ppm, inputFrames: 32, outputFrames: [32], seconds: 30)
    }
    @Test func unequalAndVaryingBlocksKeepStableHeadroom() throws {
        try schedule(ppm: -1000, inputFrames: 128, outputFrames: [16], seconds: 12)
        try schedule(ppm: 1000, inputFrames: 16, outputFrames: [128], seconds: 12)
        try schedule(ppm: -1000, inputFrames: 128, outputFrames: [128, 32, 16, 64], seconds: 12)
    }
    @Test func clockConfigurationRejectsUnsupportedRatesWithoutLosingValidConfiguration() throws {
        let ring = try #require(rl_ring_create(65536)); defer { rl_ring_destroy(ring) }
        #expect(rl_ring_prepare_clock(ring, 96000, 48000))
        for (input, output) in [(Double.nan, 48000.0), (48000.0, .infinity), (7999.0, 48000.0), (48000.0, 0.0), (768000.0, 48000.0), (384000.0, 8000.0)] {
            #expect(!rl_ring_prepare_clock(ring, input, output))
        }
        let input = [Float](repeating: 0.25, count: 512)
        for _ in 0..<8 { input.withUnsafeBufferPointer { _ = rl_ring_write(ring, $0.baseAddress, 256) } }
        var output = [Float](repeating: 0, count: 512)
        #expect(output.withUnsafeMutableBufferPointer { rl_ring_read(ring, $0.baseAddress, 256, 2) } == 256)
        #expect(output.allSatisfy { abs($0 - 0.25) < 0.00001 })
    }
}
