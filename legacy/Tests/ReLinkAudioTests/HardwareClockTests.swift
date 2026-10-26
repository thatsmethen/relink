import Testing
import CoreAudio
import RaveLinkRealtime
@testable import RaveLinkAudio

@Suite struct HardwareClockTests {
    private func stamp(sample: Double, seconds: Double, valid: Bool = true) -> AudioTimeStamp {
        var value = AudioTimeStamp()
        value.mSampleTime = sample; value.mHostTime = UInt64((seconds * 1_000_000_000).rounded())
        value.mFlags = valid ? [.sampleTimeValid, .hostTimeValid] : []
        return value
    }
    private func measured(_ rate: Double, ppm: Double) throws -> Float {
        let clock = try #require(rl_clock_create(rate, 1_000_000_000)); defer { rl_clock_destroy(clock) }
        var factor: Float = 0
        for block in 0..<300 {
            let sample = Double(block * 128)
            var time = stamp(sample: sample, seconds: sample / (rate * (1 + ppm / 1_000_000)))
            factor = rl_clock_observe(clock, &time, 128, false)
            if block == 0 { #expect(factor == 0) }
        }
        #expect(rl_clock_discontinuities(clock) == 0)
        return factor
    }
    @Test func independentlyMeasuresBothHardwareClocks() throws {
        let input = try measured(48000, ppm: -1000)
        let output = try measured(44100, ppm: 1000)
        #expect(abs(input - 0.999) < 0.000001)
        #expect(abs(output - 1.001) < 0.000001)
        #expect(abs(Double(input / output) - 0.999 / 1.001) < 0.000001)
    }
    @Test func missingFlagsUseFallbackAndRestartObservationWindow() throws {
        let clock = try #require(rl_clock_create(48000, 1_000_000_000)); defer { rl_clock_destroy(clock) }
        var invalid = stamp(sample: 0, seconds: 0, valid: false)
        #expect(rl_clock_observe(clock, &invalid, 128, false) == 0)
        var first = stamp(sample: 1024, seconds: 1)
        #expect(rl_clock_observe(clock, &first, 128, false) == 0)
        #expect(rl_clock_discontinuities(clock) == 0)
        var missing = stamp(sample: 2000, seconds: 100, valid: false)
        #expect(rl_clock_observe(clock, &missing, 128, false) == 0)
        var resumed = stamp(sample: 64000, seconds: 101)
        #expect(rl_clock_observe(clock, &resumed, 128, false) == 0)
        #expect(rl_clock_discontinuities(clock) == 0)
    }
    @Test func invalidDeviceTimelinesLatchUntilExplicitReset() throws {
        let cases: [(Double, Double)] = [(.nan, 1.01), (128, 1), (128, 0.5), (256, 1.01), (-128, 1.01), (128, 12)]
        for (sample, seconds) in cases {
            let clock = try #require(rl_clock_create(48000, 1_000_000_000)); defer { rl_clock_destroy(clock) }
            var first = stamp(sample: 0, seconds: 1)
            _ = rl_clock_observe(clock, &first, 128, false)
            var invalid = stamp(sample: sample, seconds: seconds)
            #expect(rl_clock_observe(clock, &invalid, 128, false) == 0)
            #expect(rl_clock_discontinuities(clock) == 1)
            var valid = stamp(sample: 128, seconds: 1 + 128.0 / 48000)
            #expect(rl_clock_observe(clock, &valid, 128, false) == 0)
            #expect(rl_clock_discontinuities(clock) == 1)
            rl_clock_reset(clock)
            #expect(rl_clock_discontinuities(clock) == 0)
            #expect(rl_clock_observe(clock, &valid, 128, false) == 0)
        }
    }
    @Test func idleProcessTapGapRestartsWithoutDeviceFailure() throws {
        let clock = try #require(rl_clock_create(48000, 1_000_000_000)); defer { rl_clock_destroy(clock) }
        var first = stamp(sample: 0, seconds: 1)
        _ = rl_clock_observe(clock, &first, 128, true)
        var resumed = stamp(sample: 480000, seconds: 11)
        #expect(rl_clock_observe(clock, &resumed, 128, true) == 0)
        #expect(rl_clock_discontinuities(clock) == 0)
    }
    @Test func laterClockEstimatesHaveBoundedSlew() throws {
        let clock = try #require(rl_clock_create(48000, 1_000_000_000)); defer { rl_clock_destroy(clock) }
        var time = 0.0, prior: Float = 0, priorChangeTime = 0.0
        for block in 0..<1500 {
            var timestamp = stamp(sample: Double(block * 128), seconds: time)
            let factor = rl_clock_observe(clock, &timestamp, 128, false)
            if factor > 0, factor != prior {
                if prior > 0 { #expect(abs(Double(factor - prior)) <= (time - priorChangeTime) * 0.001 + 0.0000002) }
                prior = factor; priorChangeTime = time
            }
            time += 128 / (48000 * (block < 400 ? 1 : 0.998))
        }
        #expect(abs(prior - 0.998) < 0.000001)
        #expect(rl_clock_discontinuities(clock) == 0)
    }
    @Test func declaredBufferAndActualOversizeBothFailClosed() throws {
        #expect(throws: (any Error).self) { try RealtimeDeviceLimits.validateBuffer(rate: 8000, frames: 16385) }
        #expect(throws: (any Error).self) { try RealtimeDeviceLimits.validateBuffer(rate: 384001, frames: 512) }
        try RealtimeDeviceLimits.validateBuffer(rate: 8000, frames: 16384)
        let capture = try #require(rl_capture_create(48000, 0, 512, 0))
        let render = try #require(rl_render_create(48000, 512, 0, 1))
        defer { rl_capture_destroy(capture); rl_render_destroy(render) }
        var samples = [Float](repeating: 0.25, count: 1026)
        samples.withUnsafeMutableBytes { bytes in
            var buffers = AudioBufferList(mNumberBuffers: 1, mBuffers: .init(mNumberChannels: 2, mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
            _ = rl_capture_callback(0, nil, &buffers, nil, nil, nil, UnsafeMutableRawPointer(capture))
            _ = rl_render_callback(0, nil, nil, nil, &buffers, nil, UnsafeMutableRawPointer(render))
            #expect(rl_capture_oversized_blocks(capture) == 1 && rl_render_oversized_blocks(render) == 1)
            buffers.mBuffers.mDataByteSize = 512 * 8
            _ = rl_capture_callback(0, nil, &buffers, nil, nil, nil, UnsafeMutableRawPointer(capture))
            _ = rl_render_callback(0, nil, nil, nil, &buffers, nil, UnsafeMutableRawPointer(render))
        }
        #expect(samples.allSatisfy { $0 == 0 })
        #expect(rl_capture_frames(capture) == 0 && rl_render_frames(render) == 0)
    }
    @Test func invalidTimingConfigurationIsRejected() {
        for rate in [Double.nan, 7999, 384001] { #expect(rl_clock_create(rate, 1_000_000_000) == nil) }
        for frequency in [Double.nan, .infinity, 0, -1] { #expect(rl_clock_create(48000, frequency) == nil) }
    }
    @Test func nativeCallbacksLatchSkippedSampleTimelineAndSilenceOutput() throws {
        let capture = try #require(rl_capture_create(48000, 0, 128, 0))
        let render = try #require(rl_render_create(48000, 128, 0, 1))
        defer { rl_capture_destroy(capture); rl_render_destroy(render) }
        var samples = [Float](repeating: 0.25, count: 256)
        samples.withUnsafeMutableBytes { bytes in
            var buffers = AudioBufferList(mNumberBuffers: 1, mBuffers: .init(mNumberChannels: 2, mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
            var first = stamp(sample: 0, seconds: 1)
            _ = rl_capture_callback(0, nil, &buffers, &first, nil, nil, UnsafeMutableRawPointer(capture))
            _ = rl_render_callback(0, nil, nil, nil, &buffers, &first, UnsafeMutableRawPointer(render))
            var skipped = stamp(sample: 256, seconds: 1 + 256.0 / 48000)
            _ = rl_capture_callback(0, nil, &buffers, &skipped, nil, nil, UnsafeMutableRawPointer(capture))
            _ = rl_render_callback(0, nil, nil, nil, &buffers, &skipped, UnsafeMutableRawPointer(render))
        }
        #expect(rl_capture_frames(capture) == 128 && rl_render_frames(render) == 128)
        #expect(rl_capture_timestamp_discontinuities(capture) == 1)
        #expect(rl_render_timestamp_discontinuities(render) == 1)
        #expect(samples.allSatisfy { $0 == 0 })
    }
    @Test func relativeClockMismatchLatchesBeforeReadingInvalidMix() throws {
        let ring = try #require(rl_ring_create(65536)); defer { rl_ring_destroy(ring) }
        #expect(rl_ring_prepare_clock(ring, 48000, 48000))
        #expect(rl_ring_prepare_buffers(ring, 256, 256))
        let input = [Float](repeating: 0.25, count: 512)
        for _ in 0..<8 { input.withUnsafeBufferPointer { _ = rl_ring_write(ring, $0.baseAddress, 256) } }
        rl_ring_set_source_clock(ring, 1.002)
        var output = [Float](repeating: 1, count: 512)
        #expect(output.withUnsafeMutableBufferPointer { rl_ring_read_clocked(ring, $0.baseAddress, 256, 1, 0.998) } == 0)
        #expect(rl_ring_clock_mismatch(ring))
        #expect(output.allSatisfy { $0 == 0 })
        rl_ring_set_source_clock(ring, 1)
        #expect(output.withUnsafeMutableBufferPointer { rl_ring_read_clocked(ring, $0.baseAddress, 256, 1, 1) } == 0)
        // Re-preparation is permitted only after both callbacks have stopped.
        #expect(rl_ring_prepare_clock(ring, 48000, 48000))
        #expect(rl_ring_prepare_buffers(ring, 256, 256))
        #expect(!rl_ring_clock_mismatch(ring))
        rl_ring_set_source_clock(ring, 1)
        #expect(output.withUnsafeMutableBufferPointer { rl_ring_read_clocked(ring, $0.baseAddress, 256, 1, 1) } == 256)
    }
    @Test func declaredHeadroomSupportsSmallFirstCallbackThenLargerBlocks() throws {
        let ring = try #require(rl_ring_create(65536)); defer { rl_ring_destroy(ring) }
        #expect(rl_ring_prepare_clock(ring, 48000, 48000))
        #expect(!rl_ring_prepare_buffers(ring, 0, 512))
        #expect(!rl_ring_prepare_buffers(ring, 16385, 512))
        #expect(rl_ring_prepare_buffers(ring, 512, 512))
        let input = [Float](repeating: 0.25, count: 1024)
        var output = [Float](repeating: 0, count: 1024)
        for _ in 0..<2 { input.withUnsafeBufferPointer { _ = rl_ring_write(ring, $0.baseAddress, 512) } }
        // Even a small first read waits for the declared maximum's headroom.
        #expect(output.withUnsafeMutableBufferPointer { rl_ring_read(ring, $0.baseAddress, 16, 1) } == 0)
        input.withUnsafeBufferPointer { _ = rl_ring_write(ring, $0.baseAddress, 512) }
        #expect(output.withUnsafeMutableBufferPointer { rl_ring_read(ring, $0.baseAddress, 16, 1) } == 16)
        #expect(output.withUnsafeMutableBufferPointer { rl_ring_read(ring, $0.baseAddress, 512, 1) } == 512)
        #expect(rl_ring_underruns(ring) == 0)
        let tooSmall = try #require(rl_ring_create(1024)); defer { rl_ring_destroy(tooSmall) }
        #expect(rl_ring_prepare_clock(tooSmall, 48000, 48000))
        #expect(!rl_ring_prepare_buffers(tooSmall, 512, 512))
    }
}
