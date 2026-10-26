import Foundation
import Testing
import CoreAudio
import RaveLinkCore
import RaveLinkRealtime
@testable import RaveLinkAudio

private final class AliasRenderFixture {
    let render: OpaquePointer
    let ring: OpaquePointer
    let frames: UInt32 = 256
    init(sourceCount: UInt32, buses: UInt32, inputRate: Double, samples: [Float]) throws {
        render = try #require(rl_render_create(48000, 256, sourceCount, buses))
        ring = try #require(rl_ring_create(65536))
        #expect(rl_ring_prepare_ratio(ring, inputRate / 48000))
        #expect(samples.withUnsafeBufferPointer { rl_ring_write(ring, $0.baseAddress, UInt32(samples.count / 2)) } == samples.count / 2)
        rl_render_set_source(render, 0, ring, inputRate, false)
        for source in 0..<sourceCount { rl_render_source_controls(render, source, true, 0, 0, -80, 0, 1) }
        for bus in 0..<buses { rl_render_set_bus(render, bus, bus * 2); rl_render_bus_controls(render, bus, 0, false) }
    }
    deinit { rl_render_destroy(render); rl_ring_destroy(ring) }
    func block(channels: UInt32 = 2) -> [Float] {
        var samples = [Float](repeating: 999, count: Int(frames * channels))
        samples.withUnsafeMutableBytes { bytes in
            var list = AudioBufferList(mNumberBuffers: 1, mBuffers: .init(mNumberChannels: channels, mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
            #expect(rl_render_callback(0, nil, nil, nil, &list, nil, UnsafeMutableRawPointer(render)) == noErr)
        }
        return samples
    }
}

@Suite struct SourceAliasTests {
    @Test func resampledSharedCaptureIsIdenticalAcrossIndependentGroups() throws {
        // Two frequencies and unequal stereo content expose cursor/phase errors.
        var samples: [Float] = []; samples.reserveCapacity(120000)
        for frame in 0..<60000 {
            let t = Double(frame) / 96000
            samples += [Float(0.2 * sin(2 * .pi * 997 * t) + 0.04 * cos(2 * .pi * 311 * t)), Float(0.15 * cos(2 * .pi * 1733 * t))]
        }
        let fixture = try AliasRenderFixture(sourceCount: 2, buses: 1, inputRate: 96000, samples: samples)
        #expect(rl_render_set_source_alias(fixture.render, 1, 0))
        for source: UInt32 in 0..<2 { rl_render_send_controls(fixture.render, source, 0, 0, 0, false) }
        let comparison = UnsafeMutablePointer<Float>.allocate(capacity: 513)
        comparison.initialize(repeating: 0, count: 513)
        defer { comparison.deallocate() }
        rl_render_set_group_processor(fixture.render, { context, source, data, frames in
            guard let data, let comparison = context?.assumingMemoryBound(to: Float.self) else { return -1 }
            for index in 0..<Int(frames * 2) {
                if source == 0 { comparison[index] = data[index] }
                else if source == 1 {
                    comparison[512] = max(comparison[512], abs(data[index] - comparison[index]))
                    data[index] = -data[index]
                }
            }
            return 0
        }, UnsafeMutableRawPointer(comparison))
        var residual: Float = 0
        for _ in 0..<100 { residual = max(residual, fixture.block().map(abs).max() ?? 0) }
        print("CAPTURE_ALIAS rawDifference=\(comparison[512]) cancellationResidual=\(residual)")
        #expect(comparison[512] == 0, "The post-SRC samples entering independent group chains must be bit-identical")
        // The mix accumulator may contract its second addition into FMA. Its
        // residual is bounded by one Float32 rounding step of this <0.25 signal.
        #expect(residual <= Float.ulpOfOne * 0.25)
        #expect(rl_render_take_source_peak(fixture.render, 0) > 0.20)
        #expect(rl_render_take_source_peak(fixture.render, 1) > 0.20)
        #expect(rl_ring_underruns(fixture.ring) == 0, "The single reader must not consume the same ring once per logical input")
        #expect(rl_render_frames(fixture.render) == 25600)
    }

    @Test func sharedRawAudioKeepsGroupSumEffectsMuteAndSendsIndependent() throws {
        let fixture = try AliasRenderFixture(sourceCount: 3, buses: 2, inputRate: 48000, samples: [Float](repeating: 0.2, count: 120000))
        #expect(rl_render_set_source_alias(fixture.render, 1, 0))
        #expect(rl_render_set_source_alias(fixture.render, 2, 0))
        #expect(rl_render_set_source_group(fixture.render, 2, 1))
        rl_render_source_controls(fixture.render, 0, true, -6.0206, 0, -80, 0, 1)
        rl_render_source_controls(fixture.render, 1, true, 0, 0, -80, -20, 4)
        rl_render_source_controls(fixture.render, 2, true, 24, 5000, -10, -40, 20) // Child chain must be ignored.
        rl_render_send_controls(fixture.render, 0, 0, 0, 0, false)
        rl_render_send_controls(fixture.render, 1, 1, -6.0206, 1, false)
        rl_render_send_controls(fixture.render, 2, 1, 24, -1, false) // Child send must be ignored.
        var output: [Float] = []
        for _ in 0..<120 { output = fixture.block(channels: 4) }
        let compressedSum = Float(0.1 * pow(4.0, 0.25))
        for frame in 0..<256 {
            #expect(abs(output[frame * 4] - 0.1) < 0.0002)
            #expect(abs(output[frame * 4 + 1] - 0.1) < 0.0002)
            #expect(output[frame * 4 + 2] == 0)
            #expect(abs(output[frame * 4 + 3] - compressedSum * 0.5) < 0.0002)
        }
        // Muting the input that owns capture must not silence another alias.
        rl_render_source_controls(fixture.render, 0, false, -6.0206, 0, -80, 0, 1)
        for _ in 0..<20 { output = fixture.block(channels: 4) }
        #expect(abs(output[0]) < 0.000001)
        #expect(abs(output[3] - compressedSum * 0.5) < 0.0002)
        #expect(rl_ring_underruns(fixture.ring) == 0)
    }

    @Test func aliasGraphRejectsChainsCyclesReparentingAndConfiguredAliases() throws {
        let render = try #require(rl_render_create(48000, 256, 5, 1))
        defer { rl_render_destroy(render) }
        #expect(!rl_render_set_source_alias(render, 1, 0)) // No configured owner.
        rl_render_set_source(render, 0, nil, 48000, true)
        #expect(rl_render_set_source_alias(render, 1, 0))
        #expect(rl_render_set_source_alias(render, 1, 0)) // Idempotent, no second list link.
        #expect(!rl_render_set_source_alias(render, 1, 1)) // Cannot detach a linked member.
        #expect(!rl_render_set_source_alias(render, 2, 1)) // Alias cannot own another alias.
        rl_render_set_source(render, 2, nil, 48000, true)
        #expect(rl_render_set_source_alias(render, 3, 2))
        #expect(!rl_render_set_source_alias(render, 2, 0)) // Existing owner cannot be reparented.
        #expect(!rl_render_set_source_alias(render, 0, 2)) // Forward link/cycle.
        #expect(!rl_render_set_source_alias(render, 1, 2))
        #expect(rl_render_set_source_alias(render, 4, 2))
        #expect(!rl_render_set_source_alias(render, 5, 0))
        #expect(!rl_render_set_source_alias(render, 0, 5))
        #expect(!rl_render_set_source_alias(nil, 0, 0))
    }

    @Test func capturePlanSharesCanonicalSelectorsWithoutMergingDifferentPairsOrApps() {
        let plan = CaptureSourcePlan(sources: [
            .init(kind: .device, deviceUID: "input", bundleID: "ignored"),
            .init(kind: .device, deviceUID: "input"),
            .init(kind: .device, deviceUID: "input", channelOffset: 2),
            .init(kind: .application, bundleID: "com.browser"),
            .init(kind: .application, deviceUID: "ignored", bundleID: "com.browser", channelOffset: 4),
            .init(kind: .application, bundleID: "com.browser.helper"),
            .init(kind: .system), .init(kind: .system, deviceUID: "ignored"),
            .init(kind: .tone), .init(kind: .tone)
        ])
        #expect(plan.roots == [0, 0, 2, 3, 3, 5, 6, 6, 8, 9])
        #expect(plan.uniqueCaptureCount == 5)
    }

    @Test func diagnosticsRetainLegacyDecodingAndReportPhysicalCaptureCount() throws {
        let legacy = Data(#"{"capturedFrames":100,"renderedFrames":100,"droppedFrames":0,"underrunFrames":0,"oversizedBlocks":0}"#.utf8)
        let old = try JSONDecoder().decode(AudioDiagnostics.self, from: legacy)
        #expect(old.captureSourceCount == 0)
        var current = old; current.captureSourceCount = 2
        let decoded = try JSONDecoder().decode(AudioDiagnostics.self, from: JSONEncoder().encode(current))
        #expect(decoded == current)
    }
}
