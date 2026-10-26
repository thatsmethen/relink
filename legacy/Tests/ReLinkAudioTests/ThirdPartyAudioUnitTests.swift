import Foundation
import Testing
import AudioToolbox
import RaveLinkCore
import RaveLinkRealtime
@testable import RaveLinkAudio

@Suite struct ThirdPartyAudioUnitTests {
    private func block(_ render: OpaquePointer) -> [Float] {
        var samples = [Float](repeating: 0, count: 1024)
        samples.withUnsafeMutableBytes { bytes in
            var list = AudioBufferList(mNumberBuffers: 1, mBuffers: .init(mNumberChannels: 2, mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
            #expect(rl_render_callback(0, nil, nil, nil, &list, nil, UnsafeMutableRawPointer(render)) == noErr)
        }
        return samples
    }
    private func fixture() throws -> OpaquePointer {
        let render = try #require(rl_render_create(48000, 512, 1, 1))
        rl_render_set_source(render, 0, nil, 48000, true)
        rl_render_source_controls(render, 0, true, 0, 0, -80, 0, 1)
        rl_render_send_controls(render, 0, 0, 0, 0, false)
        rl_render_set_bus(render, 0, 0); rl_render_bus_controls(render, 0, 0, false)
        return render
    }
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_PLUGIN_RENDER_MASTER_ME"] == "1"))
    func installedMasterMeRendersRestoresAndRunsThroughNativeHook() async throws {
        let helper = try #require(PluginCatalog.defaultHelperURL())
        let catalog = PluginCatalog(helperURL: helper)
        let id = AudioUnitComponentID(type: 1635083896, subtype: 1399743559, manufacturer: 1265390435)
        let selected = try #require(try await catalog.discover().first { $0.component == id })
        var samples: [Float] = []
        samples.reserveCapacity(8192 * 2)
        let leftPhaseStep: Double = 2.0 * Double.pi * 317.0 / 48_000.0
        let rightPhaseStep: Double = 2.0 * Double.pi * 719.0 / 48_000.0
        for frame in 0..<8192 {
            let phase = Double(frame)
            samples.append(Float(0.01 * sin(phase * leftPhaseStep)))
            samples.append(Float(0.015 * sin(phase * rightPhaseStep)))
        }
        let rendered = try await catalog.render(selected, samples: samples, parameters: [.init(address: 50, value: 0.5)], blockFrames: 257)
        #expect(rendered.samples.allSatisfy { $0.isFinite })
        #expect(rendered.samples.contains { abs($0) > 0.000_001 })
        #expect(rendered.parameterValues.first { $0.address == 50 }?.value == 0.5)
        let state = try #require(rendered.state)
        let restored = try await catalog.render(selected, samples: samples, state: state, blockFrames: 257)
        #expect(zip(rendered.samples, restored.samples).allSatisfy { abs($0 - $1) < 0.000_001 })
        let bypassed = try await catalog.render(selected, samples: samples, state: state, bypassed: true)
        #expect(bypassed.samples == samples)
        var session = MixerSession.initial
        let slot = AudioUnitEffectSlot(descriptor: selected, state: state, parameters: [.init(address: 50, value: 0.5)])
        session.addChannel(.init(name: "Third-party fixture", source: .init(kind: .tone), audioUnitEffects: [slot]))
        let channel = session.channels[0].id
        let graph = try LiveAudioUnitGraph(session: session, channelParents: [channel: channel], sampleRate: 48000, maxFrames: 512)
        let render = try fixture(), dry = try fixture()
        defer { rl_render_destroy(render); rl_render_destroy(dry); withExtendedLifetime(graph) {} }
        graph.install(on: render)
        var peak: Float = 0
        for _ in 0..<32 {
            let output = block(render); _ = block(dry)
            #expect(output.allSatisfy { $0.isFinite })
            peak = max(peak, output.map(abs).max()!)
        }
        #expect(peak > 0.000_001)
        #expect(graph.failureMessage == nil)
        session.channels[0].audioUnitEffects[0].parameters = [.init(address: 50, value: 0.6)]
        try graph.updateControls(session: session)
        for _ in 0..<8 { _ = block(render); _ = block(dry) }
        let liveState = try #require(try graph.snapshotStates()[channel]?[slot.id])
        let verified = try await catalog.render(selected, samples: [Float](repeating: 0, count: 1024), state: liveState)
        #expect(abs((verified.parameterValues.first { $0.address == 50 }?.value ?? -1) - 0.6) < 0.000_001)
        session.channels[0].audioUnitEffects[0].isBypassed = true
        try graph.updateControls(session: session)
        var actual: [Float] = [], expected: [Float] = []
        for _ in 0..<16 { actual = block(render); expected = block(dry) }
        #expect(zip(actual, expected).allSatisfy { abs($0 - $1) < 0.000_01 })
    }
}
