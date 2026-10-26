import Testing
import CoreAudio
import RaveLinkRealtime
@testable import RaveLinkAudio

@Suite struct AudioLifecycleTests {
    @Test func suspensionCannotBeOverriddenByLiveControlsAndDoesNotFreezeProcessing() throws {
        let render = try #require(rl_render_create(48000, 256, 1, 1))
        defer { rl_render_destroy(render) }
        rl_render_set_source(render, 0, nil, 48000, true)
        rl_render_source_controls(render, 0, true, 0, 0, -80, 0, 1)
        rl_render_set_bus(render, 0, 0)
        rl_render_bus_controls(render, 0, 0, false)
        rl_render_send_controls(render, 0, 0, 0, 0, false)
        func block() -> [Float] {
            var samples = [Float](repeating: 1, count: 512)
            samples.withUnsafeMutableBytes { bytes in
                var buffers = AudioBufferList(mNumberBuffers: 1, mBuffers: AudioBuffer(mNumberChannels: 2, mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
                #expect(rl_render_callback(0, nil, nil, nil, &buffers, nil, UnsafeMutableRawPointer(render)) == noErr)
            }
            return samples
        }
        for _ in 0..<20 { _ = block() }
        #expect(block().map(abs).max()! > 0.12)
        let before = rl_render_frames(render)
        rl_render_set_suspended(render, true)
        rl_render_bus_controls(render, 0, 12, false)
        rl_render_send_controls(render, 0, 0, 6, 0, false)
        for _ in 0..<10 { #expect(block().allSatisfy { $0 == 0 }) }
        #expect(rl_render_frames(render) == before + 2560)
        rl_render_bus_controls(render, 0, -6, false)
        rl_render_send_controls(render, 0, 0, 0, 0, false)
        rl_render_set_suspended(render, false)
        let resumed = block()
        #expect(abs(resumed[0]) < 0.005) // Restart ramps instead of stepping to full output.
        for _ in 0..<30 { _ = block() }
        #expect(abs(block().map(abs).max()! - 0.0630957) < 0.0002)
    }
    @Test func consumerResetDiscardsOldAudioAndReprimesWithFreshCapture() throws {
        let ring = try #require(rl_ring_create(8192))
        defer { rl_ring_destroy(ring) }
        let old = [Float](repeating: 0.75, count: 8192)
        #expect(old.withUnsafeBufferPointer { rl_ring_write(ring, $0.baseAddress, 4096) } == 4096)
        var output = [Float](repeating: 0, count: 512)
        #expect(output.withUnsafeMutableBufferPointer { rl_ring_read(ring, $0.baseAddress, 256, 1) } == 256)
        #expect(abs(output.last! - 0.75) < 0.0001)
        rl_ring_request_reset(ring)
        #expect(output.withUnsafeMutableBufferPointer { rl_ring_read(ring, $0.baseAddress, 256, 1) } == 0)
        #expect(output.allSatisfy { $0 == 0 })
        let fresh = [Float](repeating: -0.25, count: 8192)
        #expect(fresh.withUnsafeBufferPointer { rl_ring_write(ring, $0.baseAddress, 4096) } == 4096)
        #expect(output.withUnsafeMutableBufferPointer { rl_ring_read(ring, $0.baseAddress, 256, 1) } == 256)
        #expect(output.allSatisfy { abs($0 + 0.25) < 0.0001 })
    }
    @Test func nativeStreamValidationRejectsChangedStridesFlagsAndRate() {
        var valid = AudioStreamBasicDescription(mSampleRate: 48000, mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
            mBytesPerPacket: 8, mFramesPerPacket: 1, mBytesPerFrame: 8, mChannelsPerFrame: 2, mBitsPerChannel: 32, mReserved: 0)
        #expect(isSupportedAudioStream(valid, deviceRate: 48000))
        var changed = valid; changed.mFormatFlags = kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked
        #expect(!isSupportedAudioStream(changed, deviceRate: 48000))
        changed = valid; changed.mBytesPerFrame = 12; changed.mBytesPerPacket = 12
        #expect(!isSupportedAudioStream(changed, deviceRate: 48000))
        changed = valid; changed.mFormatFlags |= kAudioFormatFlagIsBigEndian
        #expect(!isSupportedAudioStream(changed, deviceRate: 48000))
        changed = valid; changed.mFramesPerPacket = 2
        #expect(!isSupportedAudioStream(changed, deviceRate: 48000))
        changed = valid; changed.mChannelsPerFrame = 0
        #expect(!isSupportedAudioStream(changed, deviceRate: 48000))
        #expect(!isSupportedAudioStream(valid, deviceRate: 44100))
        #expect(!isSupportedAudioStream(valid, deviceRate: .nan))
        valid.mFormatFlags |= kAudioFormatFlagIsNonInterleaved; valid.mBytesPerFrame = 4; valid.mBytesPerPacket = 4
        #expect(isSupportedAudioStream(valid, deviceRate: 48000))
    }
}

@Suite struct TapRetargetTransactionTests {
    private enum Failure: Error { case injected }
    @Test func allExclusionsAreUpdatedBeforeAnyCaptureRestarts() throws {
        var events: [String] = []
        try performTapRetargetTransaction(nodes: ["app", "remainder"], checkCancellation: {},
            stop: { events.append("stop-\($0)") }, apply: { events.append("apply-\($0)") },
            flush: { events.append("flush") }, start: { events.append("start-\($0)") })
        #expect(events == ["stop-app", "stop-remainder", "apply-app", "apply-remainder", "flush", "start-app", "start-remainder"])
    }
    @Test func failedExclusionUpdateNeverRestartsAnyCapture() {
        var events: [String] = []
        #expect(throws: Failure.self) {
            try performTapRetargetTransaction(nodes: ["app", "remainder"], checkCancellation: {},
                stop: { events.append("stop-\($0)") }, apply: { node in
                    events.append("apply-\(node)"); if node == "remainder" { throw Failure.injected }
                }, flush: { events.append("flush") }, start: { events.append("start-\($0)") })
        }
        #expect(events == ["stop-app", "stop-remainder", "apply-app", "apply-remainder"])
    }
    @Test func cancellationAfterDescriptionChangePreventsLateRestart() {
        var cancelled = false, started = false, flushed = false
        #expect(throws: Failure.self) {
            try performTapRetargetTransaction(nodes: [1], checkCancellation: { if cancelled { throw Failure.injected } },
                stop: { _ in }, apply: { _ in cancelled = true }, flush: { flushed = true }, start: { _ in started = true })
        }
        #expect(!started && !flushed)
    }
}
