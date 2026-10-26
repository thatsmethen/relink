import Foundation
import Testing
import CoreAudio
import RaveLinkRealtime

private final class GroupedRenderFixture {
    let render: OpaquePointer
    let frames: UInt32 = 256
    private var rings: [OpaquePointer] = []
    init(values: [Float], buses: UInt32 = 1) throws {
        render = try #require(rl_render_create(48_000, 256, UInt32(values.count), buses))
        for (index, value) in values.enumerated() {
            let ring = try #require(rl_ring_create(65_536))
            rings.append(ring)
            let data = [Float](repeating: value, count: 60_000 * 2)
            let written = data.withUnsafeBufferPointer { rl_ring_write(ring, $0.baseAddress, 60_000) }
            #expect(written == 60_000)
            rl_render_set_source(render, UInt32(index), ring, 48_000, false)
            rl_render_source_controls(render, UInt32(index), true, 0, 0, -80, 0, 1)
            for bus in 0..<buses { rl_render_send_controls(render, UInt32(index), bus, 0, 0, false) }
        }
        for bus in 0..<buses {
            rl_render_set_bus(render, bus, bus * 2)
            rl_render_bus_controls(render, bus, 0, false)
        }
    }
    deinit { rl_render_destroy(render); rings.forEach(rl_ring_destroy) }
    func block(channels: UInt32 = 2) -> [Float] {
        var samples = [Float](repeating: 0, count: Int(frames * channels))
        samples.withUnsafeMutableBytes { bytes in
            var buffers = AudioBufferList(mNumberBuffers: 1,
                mBuffers: AudioBuffer(mNumberChannels: channels, mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
            #expect(rl_render_callback(0, nil, nil, nil, &buffers, nil, UnsafeMutableRawPointer(render)) == noErr)
        }
        return samples
    }
}

@Suite struct GroupedRealtimeTests {
    @Test func captureSilencesItsDuplexOutputContribution() throws {
        let capture = try #require(rl_capture_create(48_000, 0, 256, 0))
        defer { rl_capture_destroy(capture) }
        var samples = [Float](repeating: 0.75, count: 512)
        samples.withUnsafeMutableBytes { bytes in
            var buffers = AudioBufferList(mNumberBuffers: 1, mBuffers: AudioBuffer(mNumberChannels: 2,
                mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
            #expect(rl_capture_callback(0, nil, nil, nil, &buffers, nil, UnsafeMutableRawPointer(capture)) == noErr)
        }
        #expect(samples.allSatisfy { $0 == 0 })
        #expect(rl_capture_frames(capture) == 0)
    }

    @Test func sumsGroupOnceAndUsesOnlyParentSendsForIndependentBuses() throws {
        let fixture = try GroupedRenderFixture(values: [0.1, 0.2], buses: 2)
        #expect(rl_render_set_source_group(fixture.render, 1, 0))
        rl_render_send_controls(fixture.render, 0, 1, -6.0206, 1, false)
        // Lowered children also have sends; their gains must never duplicate the group.
        rl_render_send_controls(fixture.render, 1, 0, 12, 0, false)
        rl_render_send_controls(fixture.render, 1, 1, 12, -1, false)
        var samples: [Float] = []
        for _ in 0..<100 { samples = fixture.block(channels: 4) }
        for frame in 0..<256 {
            #expect(abs(samples[frame * 4] - 0.3) < 0.0001)
            #expect(abs(samples[frame * 4 + 1] - 0.3) < 0.0001)
            #expect(abs(samples[frame * 4 + 2]) < 0.0001)
            #expect(abs(samples[frame * 4 + 3] - 0.15) < 0.0001)
        }
        #expect(abs(rl_render_take_source_peak(fixture.render, 0) - 0.3) < 0.0001)
        #expect(rl_render_take_source_peak(fixture.render, 1) == 0)
        rl_render_source_controls(fixture.render, 0, false, 0, 0, -80, 0, 1)
        for _ in 0..<20 { samples = fixture.block(channels: 4) }
        #expect(samples.allSatisfy { abs($0) < 0.00001 })
    }

    @Test func compressorProcessesSummedWaveformOnce() throws {
        let fixture = try GroupedRenderFixture(values: [0.2, 0.2])
        #expect(rl_render_set_source_group(fixture.render, 1, 0))
        rl_render_source_controls(fixture.render, 0, true, 0, 0, -80, -20, 4)
        // Child effect controls must not add a second gain/compression stage.
        rl_render_source_controls(fixture.render, 1, false, 24, 2000, -10, -40, 20)
        var samples: [Float] = []
        for _ in 0..<160 { samples = fixture.block() }
        let summedCompression = Float(0.1 * pow(4.0, 0.25))
        let wrongSeparateCompression = Float(2 * 0.1 * pow(2.0, 0.25))
        #expect(abs(samples.last! - summedCompression) < 0.0002)
        #expect(abs(samples.last! - wrongSeparateCompression) > 0.08)
        // Reset historical transient peak before checking steady-state meter.
        _ = rl_render_take_source_peak(fixture.render, 0)
        samples = fixture.block()
        #expect(abs(rl_render_take_source_peak(fixture.render, 0) - summedCompression) < 0.0002)
    }

    @Test func toneGroupReceivesGainExactlyOnce() throws {
        let render = try #require(rl_render_create(48_000, 256, 2, 1))
        defer { rl_render_destroy(render) }
        for index: UInt32 in 0..<2 {
            rl_render_set_source(render, index, nil, 48_000, true)
            rl_render_source_controls(render, index, true, -6.0206, 0, -80, 0, 1)
            rl_render_send_controls(render, index, 0, 0, 0, false)
        }
        #expect(rl_render_set_source_group(render, 1, 0))
        rl_render_set_bus(render, 0, 0); rl_render_bus_controls(render, 0, 0, false)
        var samples = [Float](repeating: 0, count: 512)
        for _ in 0..<100 {
            samples.withUnsafeMutableBytes { bytes in
                var buffers = AudioBufferList(mNumberBuffers: 1, mBuffers: AudioBuffer(mNumberChannels: 2,
                    mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
                #expect(rl_render_callback(0, nil, nil, nil, &buffers, nil, UnsafeMutableRawPointer(render)) == noErr)
            }
        }
        #expect(abs((samples.map(abs).max() ?? 0) - 0.12589254) < 0.0001)
    }

    @Test func defaultGroupsRemainIndependentAndInvalidGraphsAreRejected() throws {
        let fixture = try GroupedRenderFixture(values: [0.1, 0.2, 0.3])
        #expect(rl_render_set_source_group(fixture.render, 1, 0))
        #expect(!rl_render_set_source_group(fixture.render, 2, 1)) // Child cannot be a leader.
        #expect(!rl_render_set_source_group(fixture.render, 0, 2)) // Would orphan group members.
        #expect(!rl_render_set_source_group(fixture.render, 99, 0))
        #expect(!rl_render_set_source_group(fixture.render, 0, 99))
        var samples: [Float] = []
        for _ in 0..<100 { samples = fixture.block() }
        #expect(abs(samples.last! - 0.6) < 0.0001)
        #expect(abs(rl_render_take_source_peak(fixture.render, 0) - 0.3) < 0.0001)
        #expect(abs(rl_render_take_source_peak(fixture.render, 2) - 0.3) < 0.0001)
        #expect(rl_render_create(48_000, .max, .max, .max) == nil)
    }
}
