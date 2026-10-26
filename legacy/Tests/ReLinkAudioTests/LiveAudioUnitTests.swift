import Foundation
import Testing
import AudioToolbox
import RaveLinkCore
import RaveLinkRealtime
import RaveLinkAudioUnitBridge
@testable import RaveLinkAudio

private final class AUToneFixture {
    let render: OpaquePointer
    init(sources: UInt32 = 1) throws {
        render = try #require(rl_render_create(48000, 512, sources, 1))
        for source in 0..<sources {
            rl_render_set_source(render, source, nil, 48000, true)
            rl_render_source_controls(render, source, true, 0, 0, -80, 0, 1)
            rl_render_send_controls(render, source, 0, 0, 0, false)
        }
        rl_render_set_bus(render, 0, 0); rl_render_bus_controls(render, 0, 0, false)
    }
    deinit { rl_render_destroy(render) }
    func block() -> [Float] {
        var samples = [Float](repeating: 0.75, count: 1024)
        samples.withUnsafeMutableBytes { bytes in
            var list = AudioBufferList(mNumberBuffers: 1, mBuffers: .init(mNumberChannels: 2, mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
            #expect(rl_render_callback(0, nil, nil, nil, &list, nil, UnsafeMutableRawPointer(render)) == noErr)
        }
        return samples
    }
}

// Production prepares units sequentially on a dedicated control queue. Blocking
// AU preparation must not occupy Swift Testing's cooperative executor threads;
// assertions remain in the test task so failures retain their test attribution.
private let liveAUPreparationQueue = DispatchQueue(label: "RaveLink.tests.au-preparation")
private func makeLiveGraph(session: MixerSession, channelParents: [UUID: UUID], sampleRate: Double,
                           maxFrames: UInt32, delayBudgetBytes: UInt64 = 128 * 1024 * 1024) async throws -> LiveAudioUnitGraph {
    try await withCheckedThrowingContinuation { continuation in
        liveAUPreparationQueue.async {
            do {
                let graph = try LiveAudioUnitGraph(session: session, channelParents: channelParents,
                    sampleRate: sampleRate, maxFrames: maxFrames, delayBudgetBytes: delayBudgetBytes)
                continuation.resume(returning: graph)
            } catch { continuation.resume(throwing: error) }
        }
    }
}

@Suite(.serialized) struct LiveAudioUnitTests {
    private var descriptor: AudioUnitDescriptor { .init(component: .init(type: 0x61756678, subtype: 0x6c6d7472, manufacturer: 0x6170706c), name: "Apple: AUPeakLimiter", version: 67072) }
    private func session() -> MixerSession {
        var session = MixerSession.initial
        session.addChannel(.init(name: "Tone", source: .init(kind: .tone), audioUnitEffects: [.init(descriptor: descriptor, parameters: [.init(address: 2, value: 6.0206)])]))
        return session
    }
    @Test func effectTopologyExcludesSafeLiveControls() {
        let initial = session()
        var changed = initial
        changed.channels[0].audioUnitEffects[0].isBypassed = true
        changed.channels[0].audioUnitEffects[0].parameters = [.init(address: 2, value: -6)]
        #expect(LiveAudioUnitGraph.topologyEquivalent(initial, changed))
        changed.channels[0].audioUnitEffects[0].id = UUID()
        #expect(!LiveAudioUnitGraph.topologyEquivalent(initial, changed))
    }
    @Test func hostScratchBudgetCountsRacksOncePerVisibleGroup() throws {
        var grouped = session()
        grouped.channels[0].additionalSources = [.init(kind: .application, bundleID: "test.member")]
        let expanded = try grouped.expandedRouting()
        let groupedBytes = try LiveAudioUnitGraph.estimatedHostOverhead(session: expanded.session, channelParents: expanded.channelParents)
        let independentBytes = try LiveAudioUnitGraph.estimatedHostOverhead(session: expanded.session, channelParents: [:])
        #expect(groupedBytes > 4 * 512 * MemoryLayout<Float>.size)
        #expect(independentBytes - groupedBytes >= 4 * 512 * MemoryLayout<Float>.size)
        var global = AudioAllocationBudget(limit: groupedBytes)
        try global.reserve(groupedBytes)
        #expect(throws: (any Error).self) { try global.reserve(1) }
        #expect(throws: (any Error).self) {
            var small = AudioAllocationBudget(limit: groupedBytes - 1)
            try small.reserve(groupedBytes)
        }
    }
    @Test func renderErrorLatchesSilenceAcrossTheEntireOutput() throws {
        let bridge = try #require(rl_au_graph_create(2, 512, 48000))
        defer { rl_au_graph_destroy(bridge) }
        let fixture = try AUToneFixture(sources: 2)
        let failing: AURenderBlock = { _, _, _, _, _, _ in -12345 }
        #expect(rl_au_graph_append(bridge, 1, 7, failing, nil, false, 0))
        #expect(rl_au_graph_finalize(bridge))
        rl_render_set_group_processor(fixture.render, rl_au_graph_process, UnsafeMutableRawPointer(bridge))
        #expect(fixture.block().allSatisfy { $0 == 0 })
        #expect(rl_render_processor_error(fixture.render) == -12345)
        var source: UInt32 = 0, slot: UInt32 = 0
        #expect(rl_au_graph_error(bridge, &source, &slot) == -12345)
        #expect(source == 1 && slot == 7)
        // Even disconnecting the failed processor while stopped cannot clear its
        // existing render graph latch. Recovery requires an explicit new graph.
        rl_render_set_group_processor(fixture.render, nil, nil)
        #expect(fixture.block().allSatisfy { $0 == 0 })
    }
    @Test func malformedEffectOutputLatchesFailure() throws {
        let bridge = try #require(rl_au_graph_create(1, 512, 48000))
        defer { rl_au_graph_destroy(bridge) }
        let fixture = try AUToneFixture()
        let invalid: AURenderBlock = { _, _, frames, _, output, _ in
            let list = UnsafeMutableAudioBufferListPointer(output)
            for buffer in list { buffer.mData?.assumingMemoryBound(to: Float.self).update(repeating: .nan, count: Int(frames)) }
            return noErr
        }
        #expect(rl_au_graph_append(bridge, 0, 3, invalid, nil, false, 0))
        #expect(rl_au_graph_finalize(bridge))
        rl_render_set_group_processor(fixture.render, rl_au_graph_process, UnsafeMutableRawPointer(bridge))
        #expect(fixture.block().allSatisfy { $0 == 0 })
        #expect(rl_render_processor_error(fixture.render) == -70003)
    }
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_PLUGIN_RENDER_APPLE"] == "1"))
    func appleEffectRunsInNativeCallbackAndChangesWithoutRebuild() async throws {
        var session = session()
        let id = session.channels[0].id
        session.channels[0].additionalSources = [.init(kind: .application, bundleID: "test.second-source")]
        let expanded = try session.expandedRouting()
        let graph = try await makeLiveGraph(session: expanded.session, channelParents: expanded.channelParents, sampleRate: 48000, maxFrames: 512)
        let dry = try AUToneFixture(sources: 2), wet = try AUToneFixture(sources: 2)
        #expect(rl_render_set_source_group(dry.render, 1, 0))
        #expect(rl_render_set_source_group(wet.render, 1, 0))
        graph.install(on: wet.render)
        var drySamples: [Float] = [], wetSamples: [Float] = []
        for _ in 0..<32 { drySamples += dry.block(); wetSamples += wet.block() }
        var maxError: Float = 0
        for frame in 4096..<16384 { maxError = max(maxError, abs(wetSamples[frame * 2] - drySamples[(frame - 96) * 2] * 2)) }
        #expect(maxError < 0.000_03)
        #expect(graph.failureMessage == nil)
        #expect(abs(graph.alignmentLatencySeconds - 0.002) < 1e-9)
        #expect(graph.delayMemoryBytes == 96 * 8)
        session.channels[0].audioUnitEffects[0].parameters = [.init(address: 2, value: -6.0206)]
        try graph.updateControls(session: session.expandedRouting().session)
        drySamples = []; wetSamples = []
        for _ in 0..<16 { drySamples += dry.block(); wetSamples += wet.block() }
        maxError = 0
        for frame in 2048..<8192 { maxError = max(maxError, abs(wetSamples[frame * 2] - drySamples[(frame - 96) * 2] * 0.5)) }
        #expect(maxError < 0.000_03)
        session.channels[0].audioUnitEffects[0].isBypassed = true
        try graph.updateControls(session: session.expandedRouting().session)
        drySamples = []; wetSamples = []
        for _ in 0..<16 { drySamples += dry.block(); wetSamples += wet.block() }
        maxError = 0
        for index in 8192..<16384 { maxError = max(maxError, abs(wetSamples[index] - drySamples[index - 192])) }
        #expect(maxError < 0.000_01)
        #expect(try graph.snapshotStates()[id]?.count == 1)
        #expect(rl_render_processor_error(wet.render) == 0)
    }
    @Test func bypassAndEmptyRackDelaysSurviveIrregularBlockBoundaries() throws {
        let bridge = try #require(rl_au_graph_create(3, 512, 48000))
        defer { rl_au_graph_destroy(bridge) }
        // The synthetic wet path is irrelevant because both nodes start bypassed.
        let silent: AURenderBlock = { flags, _, _, _, _, _ in
            flags.pointee.insert(.unitRenderAction_OutputIsSilence); return noErr
        }
        #expect(rl_au_graph_append(bridge, 0, 0, silent, nil, true, 3))
        #expect(rl_au_graph_append(bridge, 0, 1, silent, nil, true, 4))
        #expect(rl_au_graph_enable_source(bridge, 1)) // Empty leader still aligns.
        #expect(rl_au_graph_finalize(bridge))
        #expect(rl_au_graph_latency_frames(bridge) == 7)
        #expect(!rl_au_graph_append(bridge, 0, 2, silent, nil, true, 1))
        var frame = 0
        for count in [1, 3, 2, 11, 257, 512, 7] {
            var signal = [Float]()
            var expected = [Float]()
            for offset in 0..<count {
                let input = Float(frame + offset + 1)
                signal += [input, -input]
                let delayed = frame + offset < 7 ? Float.zero : input - 7
                expected += [delayed, -delayed]
            }
            var rack = signal, empty = signal, inactive = signal
            #expect(rl_au_graph_process(UnsafeMutableRawPointer(bridge), 0, &rack, UInt32(count)) == 0)
            #expect(rl_au_graph_process(UnsafeMutableRawPointer(bridge), 1, &empty, UInt32(count)) == 0)
            #expect(rl_au_graph_process(UnsafeMutableRawPointer(bridge), 2, &inactive, UInt32(count)) == 0)
            #expect(rack == expected)
            #expect(empty == expected)
            #expect(inactive == signal)
            frame += count
        }
    }
    @Test func latencyAndDelayMemoryAreBoundedBeforeRendering() throws {
        let bridge = try #require(rl_au_graph_create(2, 512, 48000))
        defer { rl_au_graph_destroy(bridge) }
        let silent: AURenderBlock = { _, _, _, _, _, _ in noErr }
        #expect(rl_au_graph_set_latency_limits(bridge, 10, 80))
        #expect(!rl_au_graph_append(bridge, 0, 0, silent, nil, false, 11))
        #expect(rl_au_graph_append(bridge, 0, 0, silent, nil, false, 10))
        #expect(!rl_au_graph_append(bridge, 0, 1, silent, nil, false, 1))
        #expect(rl_au_graph_enable_source(bridge, 1))
        #expect(!rl_au_graph_finalize(bridge)) // Another 80 bytes needed for empty rack.
        #expect(!rl_au_graph_finalize(bridge)) // Partial configuration cannot be reused.
        var samples = [Float](repeating: 0, count: 2)
        #expect(rl_au_graph_process(UnsafeMutableRawPointer(bridge), 0, &samples, 1) == -70001)
    }
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_PLUGIN_RENDER_APPLE"] == "1"))
    func emptyChannelAlignsWithTwoRealAppleEffects() async throws {
        var session = session()
        let extra = AudioUnitEffectSlot(descriptor: descriptor, parameters: [.init(address: 2, value: 0)])
        session.channels[0].audioUnitEffects[0].parameters = [.init(address: 2, value: 0)]
        session.channels[0].audioUnitEffects.append(extra)
        session.addChannel(.init(name: "Dry", source: .init(kind: .tone)))
        let graph = try await makeLiveGraph(session: session, channelParents: [:], sampleRate: 48000, maxFrames: 512)
        let dry = try AUToneFixture(sources: 2), wet = try AUToneFixture(sources: 2)
        graph.install(on: wet.render)
        var drySamples: [Float] = [], wetSamples: [Float] = []
        for _ in 0..<24 { drySamples += dry.block(); wetSamples += wet.block() }
        var maxError: Float = 0
        for index in 8192..<24576 { maxError = max(maxError, abs(wetSamples[index] - drySamples[index - 384])) }
        #expect(maxError < 0.000_01)
        #expect(abs(graph.alignmentLatencySeconds - 0.004) < 1e-9)
        #expect(graph.delayMemoryBytes == 192 * 8 * 2)
        #expect(graph.failureMessage == nil)
    }
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_PLUGIN_RENDER_APPLE"] == "1"))
    func changingAppleAttackDelayStopsUntilGraphReconstruction() async throws {
        var session = session()
        let graph = try await makeLiveGraph(session: session, channelParents: [:], sampleRate: 48000, maxFrames: 512)
        let wet = try AUToneFixture()
        graph.install(on: wet.render)
        _ = wet.block()
        session.channels[0].audioUnitEffects[0].parameters.append(.init(address: 0, value: 0.01))
        #expect(throws: (any Error).self) { try graph.updateControls(session: session) }
        #expect(graph.failureMessage?.contains("changed its reported latency") == true)
        #expect(wet.block().allSatisfy { $0 == 0 })
        #expect(rl_render_processor_error(wet.render) == -70004)
        let restarted = try await makeLiveGraph(session: session, channelParents: [:], sampleRate: 48000, maxFrames: 512)
        #expect(abs(restarted.alignmentLatencySeconds - 0.01) < 1e-9)
        #expect(restarted.failureMessage == nil)
    }
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_PLUGIN_RENDER_APPLE"] == "1"))
    func remainingGraphBudgetBoundsActualAppleDelayStorage() async throws {
        let session = session()
        await #expect(throws: (any Error).self) {
            try await makeLiveGraph(session: session, channelParents: [:], sampleRate: 48000, maxFrames: 512, delayBudgetBytes: 96 * 8 - 1)
        }
        let exact = try await makeLiveGraph(session: session, channelParents: [:], sampleRate: 48000, maxFrames: 512, delayBudgetBytes: 96 * 8)
        #expect(exact.delayMemoryBytes == 96 * 8)
        #expect(exact.failureMessage == nil)
    }
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_PLUGIN_RENDER_APPLE"] == "1"))
    func failedPreparationCanReleasePartlyCreatedGraph() async throws {
        var session = session()
        session.channels[0].audioUnitEffects[0].parameters = [.init(address: 2, value: 100)]
        await #expect(throws: (any Error).self) {
            try await makeLiveGraph(session: session, channelParents: [:], sampleRate: 48000, maxFrames: 512)
        }
        session.channels[0].audioUnitEffects[0].parameters = []
        session.channels[0].audioUnitEffects[0].state = .init(component: descriptor.component, componentVersion: descriptor.version, propertyList: Data([1, 2, 3]))
        await #expect(throws: (any Error).self) {
            try await makeLiveGraph(session: session, channelParents: [:], sampleRate: 48000, maxFrames: 512)
        }
        session.channels[0].audioUnitEffects[0].state = nil
        // Prepare a real first unit before failing a missing second unit, so the
        // partially built bridge and its retained block ownership are exercised.
        session.channels[0].audioUnitEffects.append(.init(descriptor: .init(component: .init(type: 0x61756678, subtype: 0xdeadbeef, manufacturer: 0xdeadbeef), name: "Missing test unit", version: nil)))
        await #expect(throws: (any Error).self) {
            try await makeLiveGraph(session: session, channelParents: [:], sampleRate: 48000, maxFrames: 512)
        }
    }
}
