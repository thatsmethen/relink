import Testing
import CoreAudio
import RaveLinkRealtime
@testable import RaveLinkAudio
import RaveLinkCore

@Suite struct RealtimeTests {
    @Test func testDSPGainHighPassGateAndCompression() throws {
        let dsp = try requireValue(rl_dsp_create(48000))
        defer { rl_dsp_destroy(dsp) }
        var samples = [Float](repeating: 0.5, count: 48000 * 2)
        rl_dsp_controls(dsp, -6.0206, 0, -80, 0, 1)
        samples.withUnsafeMutableBufferPointer { rl_dsp_process(dsp, $0.baseAddress, 48000) }
        expectEqual(samples.last!, 0.25, accuracy: 0.0001)

        rl_dsp_controls(dsp, 0, 500, -80, 0, 1)
        samples = [Float](repeating: 0.5, count: 48000 * 2)
        samples.withUnsafeMutableBufferPointer { rl_dsp_process(dsp, $0.baseAddress, 48000) }
        expectLess(abs(samples.last!), 0.0001, "High-pass must remove DC")

        rl_dsp_controls(dsp, 0, 0, -30, 0, 1)
        samples = [Float](repeating: 0.001, count: 48000 * 2)
        samples.withUnsafeMutableBufferPointer { rl_dsp_process(dsp, $0.baseAddress, 48000) }
        expectLess(abs(samples.last!), 0.000001, "Gate must close below threshold")

        rl_dsp_controls(dsp, 0, 0, -80, -20, 4)
        samples = [Float](repeating: 0.5, count: 48000 * 2)
        samples.withUnsafeMutableBufferPointer { rl_dsp_process(dsp, $0.baseAddress, 48000) }
        let compressed = Float(0.1 * pow(5.0, 0.25))
        expectEqual(samples.last!, compressed, accuracy: 0.001)
    }

    @Test func testNonFiniteInputIsContainedAndInvalidConfigurationRejected() throws {
        expectNil(rl_capture_create(48000, 0, 0, 1))
        expectNil(rl_dsp_create(.nan))
        expectNil(rl_ring_create(0))
        let dsp = try requireValue(rl_dsp_create(48000)); defer { rl_dsp_destroy(dsp) }
        var samples: [Float] = [.nan, .infinity, -.infinity, 0.2]
        samples.withUnsafeMutableBufferPointer { rl_dsp_process(dsp, $0.baseAddress, 2) }
        expectTrue(samples.allSatisfy(\.isFinite))
    }

    @Test func testRingIsBoundedAndReportsOverflowAndUnderrun() throws {
        let ring = try requireValue(rl_ring_create(1024)); defer { rl_ring_destroy(ring) }
        let input = [Float](repeating: 0.25, count: 4096)
        let written = input.withUnsafeBufferPointer { rl_ring_write(ring, $0.baseAddress, 2048) }
        expectEqual(written, 1024); expectEqual(rl_ring_dropped(ring), 1024)
        var output = [Float](repeating: 0, count: 1024)
        let first = output.withUnsafeMutableBufferPointer { rl_ring_read(ring, $0.baseAddress, 512, 1) }
        expectEqual(first, 512)
        expectEqual(output[100], 0.25, accuracy: 0.0001)
        let second = output.withUnsafeMutableBufferPointer { rl_ring_read(ring, $0.baseAddress, 512, 1) }
        expectLess(second, 512)
        expectGreater(rl_ring_underruns(ring), 0)
        expectFalse(rl_ring_prepare_ratio(ring, 0))
        expectFalse(rl_ring_prepare_ratio(ring, .infinity))
        expectFalse(rl_ring_prepare_ratio(ring, 100))
    }

    @Test func testBandLimitedDownsamplingRejectsAliasedHighFrequency() throws {
        func resampleRMS(frequency: Double, sourceRate: Double = 48000, ratio: Double = 2) throws -> Double {
            let ring = try requireValue(rl_ring_create(131072)); defer { rl_ring_destroy(ring) }
            expectTrue(rl_ring_prepare_ratio(ring, ratio))
            var input = [Float](repeating: 0, count: 65536 * 2)
            for frame in 0..<65536 { let sample = Float(sin(2 * .pi * frequency * Double(frame) / sourceRate) * 0.5); input[frame * 2] = sample; input[frame * 2 + 1] = sample }
            input.withUnsafeBufferPointer { _ = rl_ring_write(ring, $0.baseAddress, 65536) }
            var output = [Float](repeating: 0, count: 1024 * 2)
            let count = output.withUnsafeMutableBufferPointer { rl_ring_read(ring, $0.baseAddress, 1024, ratio) }
            expectEqual(count, 1024)
            return sqrt(output.reduce(0.0) { $0 + Double($1 * $1) } / Double(output.count))
        }
        expectEqual(try resampleRMS(frequency: 1000), 0.35355, accuracy: 0.005)
        expectLess(try resampleRMS(frequency: 18000), 0.001)
        expectEqual(try resampleRMS(frequency: 1000, sourceRate: 192000, ratio: 24), 0.35355, accuracy: 0.005)
        expectLess(try resampleRMS(frequency: 5000, sourceRate: 192000, ratio: 24), 0.001)
    }

    @Test func testIndependentBusGainPanMuteAndOffsets() throws {
        let render = try requireValue(rl_render_create(48000, 512, 1, 2)); defer { rl_render_destroy(render) }
        rl_render_set_source(render, 0, nil, 48000, true)
        rl_render_source_controls(render, 0, true, 0, 0, -80, 0, 1)
        rl_render_set_bus(render, 0, 0); rl_render_set_bus(render, 1, 2)
        rl_render_bus_controls(render, 0, 0, false); rl_render_bus_controls(render, 1, 0, false)
        rl_render_send_controls(render, 0, 0, 0, 0, false)
        rl_render_send_controls(render, 0, 1, -6.0206, 1, false)
        var data = [Float](repeating: 0, count: 512 * 4)
        func renderBlock() {
            data.withUnsafeMutableBytes { bytes in
                var buffer = AudioBufferList(mNumberBuffers: 1, mBuffers: AudioBuffer(mNumberChannels: 4, mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
                _ = rl_render_callback(0, nil, nil, nil, &buffer, nil, UnsafeMutableRawPointer(render))
            }
        }
        for _ in 0..<20 { renderBlock() }
        let peak = (0..<4).map { channel in stride(from: channel, to: data.count, by: 4).map { abs(data[$0]) }.max()! }
        expectEqual(peak[0], 0.1258925, accuracy: 0.0001)
        expectEqual(peak[1], peak[0], accuracy: 0.0001)
        expectEqual(peak[2], 0, accuracy: 0.0001)
        expectEqual(peak[3], peak[0] * 0.5, accuracy: 0.0001)
        expectGreater(rl_render_take_bus_peak(render, 0), 0.12)
        expectEqual(rl_render_take_bus_peak(render, 0), 0)

        rl_render_bus_controls(render, 0, 0, true)
        for _ in 0..<20 { renderBlock() }
        expectLess(stride(from: 0, to: data.count, by: 4).map { abs(data[$0]) }.max()!, 0.000001)
        expectGreater(stride(from: 3, to: data.count, by: 4).map { abs(data[$0]) }.max()!, 0.06)
        expectEqual(rl_render_oversized_blocks(render), 0)
        expectEqual(rl_render_frames(render), 40 * 512)
    }

    @Test func testGainChangesRampAndMuteReachesSilence() throws {
        let render = try requireValue(rl_render_create(48000, 512, 1, 1)); defer { rl_render_destroy(render) }
        rl_render_set_source(render, 0, nil, 48000, true)
        rl_render_source_controls(render, 0, true, 0, 0, -80, 0, 1)
        rl_render_set_bus(render, 0, 0); rl_render_bus_controls(render, 0, 0, false)
        rl_render_send_controls(render, 0, 0, 0, 0, false)
        var data = [Float](repeating: 0, count: 1024)
        func renderBlock() {
            data.withUnsafeMutableBytes { bytes in
                var buffer = AudioBufferList(mNumberBuffers: 1, mBuffers: AudioBuffer(mNumberChannels: 2, mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
                _ = rl_render_callback(0, nil, nil, nil, &buffer, nil, UnsafeMutableRawPointer(render))
            }
        }
        for _ in 0..<20 { renderBlock() }
        let prior = data[data.count - 2]
        rl_render_source_controls(render, 0, false, 0, 0, -80, 0, 1)
        renderBlock()
        expectLess(abs(data[0] - prior), 0.01, "Mute transition must preserve a small first-sample delta")
        expectGreater(data.map(abs).max()!, 0.02, "Mute should ramp, not cut discontinuously")
        for _ in 0..<20 { renderBlock() }
        expectLess(data.map(abs).max()!, 0.000001)
    }

    @Test func testCaptureMonoDuplicationAndMeter() throws {
        let capture = try requireValue(rl_capture_create(48000, 0, 512, 1)); defer { rl_capture_destroy(capture) }
        let ring = try requireValue(rl_ring_create(2048)); defer { rl_ring_destroy(ring) }
        rl_capture_set_destination(capture, 0, ring)
        var input = [Float](repeating: 0.2, count: 512)
        input.withUnsafeMutableBytes { bytes in
            var buffer = AudioBufferList(mNumberBuffers: 1, mBuffers: AudioBuffer(mNumberChannels: 1, mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
            _ = rl_capture_callback(0, nil, &buffer, nil, nil, nil, UnsafeMutableRawPointer(capture))
        }
        expectEqual(rl_capture_frames(capture), 512)
        expectEqual(rl_capture_take_peak(capture), 0.2, accuracy: 0.0001)
        var output = [Float](repeating: 0, count: 256)
        output.withUnsafeMutableBufferPointer { expectEqual(rl_ring_read(ring, $0.baseAddress, 128, 1), 128) }
        expectEqual(output[0], 0.2, accuracy: 0.0001); expectEqual(output[1], 0.2, accuracy: 0.0001)
    }
}

@Suite(.serialized) struct AudioCoordinatorTests {
    @Test @MainActor func testInvalidStartFailsWithActionableErrorAndStaysStopped() throws {
        let coordinator = AudioCoordinator()
        expectThrows(try coordinator.start(session: .initial))
        expectFalse(coordinator.running)
        expectTrue(coordinator.errorMessage?.contains("input") == true)
        var session = MixerSession.initial
        session.addChannel(.init(name: "Test", source: .init(kind: .tone)))
        expectThrows(try coordinator.start(session: session))
        expectTrue(coordinator.errorMessage?.contains("output") == true)
        expectFalse(coordinator.running)
    }

    /// Opt-in because this opens a real driver. It never changes the system default
    /// devices and every return path stops both capture and render IOProcs.
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_TEST_LOOPBACK"] == "1")) @MainActor func testBlackHoleLoopbackAndLiveBusControls() async throws {
        
        let coordinator = AudioCoordinator()
        let requestedUID = ProcessInfo.processInfo.environment["RAVELINK_LOOPBACK_DEVICE_UID"]
        for iteration in 0..<200 {
            if coordinator.devices.contains(where: { device in requestedUID.map { device.id == $0 } ?? (device.name == "BlackHole 2ch") }) { break }
            if iteration % 20 == 0 { coordinator.refreshDevices() }
            try await Task.sleep(for: .milliseconds(50))
        }
        guard let blackHole = coordinator.devices.first(where: { device in
            (requestedUID.map { device.id == $0 } ?? (device.name == "BlackHole 2ch")) && device.inputChannels >= 2 && device.outputChannels >= 2
        }) else { Issue.record("Opt-in hardware test requires the requested virtual device (default BlackHole 2ch)"); return }
        try #require(blackHole.isVirtual, "The loopback test must target a virtual device")
        let originalDefaults = [defaultDevice(kAudioHardwarePropertyDefaultInputDevice), defaultDevice(kAudioHardwarePropertyDefaultOutputDevice)]
        let tone = MixerChannel(name: "Reference tone", source: .init(kind: .tone))
        let returnChannel = MixerChannel(name: "Actual driver return", source: .init(kind: .device, deviceUID: blackHole.id))
        var bus = MixBus(name: "Loopback test", deviceUID: blackHole.id)
        var session = MixerSession(channels: [tone, returnChannel], buses: [bus], sends: [.init(channelID: tone.id, busID: bus.id), .init(channelID: returnChannel.id, busID: bus.id, muted: true)])
        defer { coordinator.stop() }
        if let readyFile = ProcessInfo.processInfo.environment["RAVELINK_CAPTURE_READY_FILE"] {
            for _ in 0..<200 where !FileManager.default.fileExists(atPath: readyFile) { try await Task.sleep(for: .milliseconds(50)) }
            try #require(FileManager.default.fileExists(atPath: readyFile), "Independent recorder did not become ready")
        }
        try coordinator.start(session: session)
        for _ in 0..<240 where coordinator.starting { try await Task.sleep(for: .milliseconds(50)) }
        try #require(coordinator.running, Comment(rawValue: coordinator.errorMessage ?? "Audio did not start"))
        try await Task.sleep(for: .seconds(2))
        let unityPeak = coordinator.channelMeters[returnChannel.id] ?? 0
        expectEqual(unityPeak, 0.1258925, accuracy: 0.003)
        expectGreater(coordinator.diagnostics.capturedFrames, 10000)
        expectGreater(coordinator.diagnostics.renderedFrames, 10000)
        let firstStats = coordinator.diagnostics
        bus.gainDB = -12; session.buses = [bus]
        try coordinator.update(session: session)
        try await Task.sleep(for: .seconds(1))
        let attenuatedPeak = coordinator.channelMeters[returnChannel.id] ?? 0
        expectEqual(attenuatedPeak, unityPeak * 0.2511886, accuracy: 0.003)
        bus.muted = true; session.buses = [bus]
        try coordinator.update(session: session)
        try await Task.sleep(for: .seconds(1))
        let mutedPeak = coordinator.channelMeters[returnChannel.id] ?? 0
        expectLess(mutedPeak, 0.00001)
        expectEqual(coordinator.diagnostics.oversizedBlocks, 0)
        print("VIRTUAL_DRIVER_LIVE_EVIDENCE uid=\(blackHole.id) rate=\(blackHole.sampleRate) unityPeak=\(unityPeak) minus12dBPeak=\(attenuatedPeak) mutedPeak=\(mutedPeak) capturedFrames=\(firstStats.capturedFrames) renderedFrames=\(firstStats.renderedFrames) droppedFrames=\(coordinator.diagnostics.droppedFrames) underrunFrames=\(coordinator.diagnostics.underrunFrames)")
        coordinator.stop()
        expectFalse(coordinator.running)
        for _ in 0..<3 {
            try coordinator.start(session: session)
            for _ in 0..<240 where coordinator.starting { try await Task.sleep(for: .milliseconds(50)) }
            try #require(coordinator.running, Comment(rawValue: coordinator.errorMessage ?? "Restart failed"))
            coordinator.stop()
        }
        expectEqual([defaultDevice(kAudioHardwarePropertyDefaultInputDevice), defaultDevice(kAudioHardwarePropertyDefaultOutputDevice)], originalDefaults)
    }

    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_TEST_SILENT_OUTPUT"] == "1")) @MainActor func testSilentPhysicalOutputLifecycle() async throws {
        let coordinator = AudioCoordinator()
        let uid = ProcessInfo.processInfo.environment["RAVELINK_SILENT_OUTPUT_UID"] ?? "BuiltInSpeakerDevice"
        for _ in 0..<100 where !coordinator.devices.contains(where: { $0.id == uid }) { try await Task.sleep(for: .milliseconds(50)) }
        let device = try #require(coordinator.devices.first { $0.id == uid && $0.outputChannels >= 2 })
        let source = MixerChannel(name: "Silent source", source: .init(kind: .tone), muted: true)
        let bus = MixBus(name: "Silent hardware check", deviceUID: device.id)
        let session = MixerSession(channels: [source], buses: [bus], sends: [.init(channelID: source.id, busID: bus.id, muted: true)])
        defer { coordinator.stop() }
        try coordinator.start(session: session)
        for _ in 0..<240 where coordinator.starting { try await Task.sleep(for: .milliseconds(50)) }
        try #require(coordinator.running, Comment(rawValue: coordinator.errorMessage ?? "Output did not start"))
        try await Task.sleep(for: .seconds(1))
        #expect(coordinator.diagnostics.renderedFrames >= 10000)
        #expect(coordinator.busMeters[bus.id] == 0)
        #expect(coordinator.diagnostics.oversizedBlocks == 0)
        print("NATIVE_SILENT_OUTPUT uid=\(device.id) renderedFrames=\(coordinator.diagnostics.renderedFrames) peak=\(coordinator.busMeters[bus.id] ?? -1)")
        coordinator.stop()
        try await Task.sleep(for: .milliseconds(100))
        #expect(coordinator.errorMessage == nil)
    }
}

private func defaultDevice(_ selector: AudioObjectPropertySelector) -> AudioObjectID {
    var address = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var device: AudioObjectID = 0, size = UInt32(MemoryLayout<AudioObjectID>.size)
    _ = AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &device)
    return device
}

// Small assertion helpers preserve source context under Swift Testing on the
// Command Line Tools toolchain, where XCTest is not installed.
private func requireValue<T>(_ value: T?, sourceLocation: SourceLocation = #_sourceLocation) throws -> T { try #require(value, sourceLocation: sourceLocation) }
private func expectEqual<T: Equatable>(_ a: T, _ b: T, _ message: String = "", sourceLocation: SourceLocation = #_sourceLocation) { #expect(a == b, Comment(rawValue: message), sourceLocation: sourceLocation) }
private func expectEqual<T: BinaryFloatingPoint>(_ a: T, _ b: T, accuracy: T, sourceLocation: SourceLocation = #_sourceLocation) { #expect(abs(a - b) <= accuracy, sourceLocation: sourceLocation) }
private func expectLess<T: Comparable>(_ a: T, _ b: T, _ message: String = "", sourceLocation: SourceLocation = #_sourceLocation) { #expect(a < b, Comment(rawValue: message), sourceLocation: sourceLocation) }
private func expectGreater<T: Comparable>(_ a: T, _ b: T, _ message: String = "", sourceLocation: SourceLocation = #_sourceLocation) { #expect(a > b, Comment(rawValue: message), sourceLocation: sourceLocation) }
private func expectTrue(_ value: Bool, _ message: String = "", sourceLocation: SourceLocation = #_sourceLocation) { #expect(value, Comment(rawValue: message), sourceLocation: sourceLocation) }
private func expectFalse(_ value: Bool, sourceLocation: SourceLocation = #_sourceLocation) { #expect(!value, sourceLocation: sourceLocation) }
private func expectNil<T>(_ value: T?, sourceLocation: SourceLocation = #_sourceLocation) { #expect(value == nil, sourceLocation: sourceLocation) }
private func expectThrows(_ action: @autoclosure () throws -> Any, sourceLocation: SourceLocation = #_sourceLocation) {
    var threw = false
    do { _ = try action() } catch { threw = true }
    #expect(threw, sourceLocation: sourceLocation)
}
