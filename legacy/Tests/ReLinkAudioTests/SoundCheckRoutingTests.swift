import Foundation
import Testing
import CoreAudio
import RaveLinkCore
import RaveLinkRealtime
@testable import RaveLinkAudio

@Suite struct SoundCheckRoutingTests {
    @Test func dryGroupIsRecordedBeforeEffectsAndPlaybackReplacesLiveInput() throws {
        let render = try #require(rl_render_create(48_000, 512, 2, 1))
        let check = try #require(rl_sound_check_create(48_000, 1))
        let left = try #require(rl_ring_create(65_536)), right = try #require(rl_ring_create(65_536))
        defer { rl_render_destroy(render); rl_sound_check_destroy(check); rl_ring_destroy(left); rl_ring_destroy(right) }
        for (index, ring, value) in [(UInt32(0), left, Float(0.1)), (UInt32(1), right, Float(0.2))] {
            let samples = [Float](repeating: value, count: 60_000 * 2)
            samples.withUnsafeBufferPointer { _ = rl_ring_write(ring, $0.baseAddress, 60_000) }
            rl_render_set_source(render, index, ring, 48_000, false)
            rl_render_source_controls(render, index, true, 6.0206, 0, -80, 0, 1)
            rl_render_send_controls(render, index, 0, 0, 0, false)
        }
        #expect(rl_render_set_source_group(render, 1, 0))
        #expect(rl_render_set_sound_check(render, 0, check))
        #expect(!rl_render_set_sound_check(render, 1, check))
        rl_render_set_bus(render, 0, 0); rl_render_bus_controls(render, 0, 0, false)
        func block() -> [Float] {
            var samples = [Float](repeating: 999, count: 1024)
            samples.withUnsafeMutableBytes { bytes in
                var buffers = AudioBufferList(mNumberBuffers: 1, mBuffers: .init(mNumberChannels: 2, mDataByteSize: UInt32(bytes.count), mData: bytes.baseAddress))
                #expect(rl_render_callback(0, nil, nil, nil, &buffers, nil, UnsafeMutableRawPointer(render)) == noErr)
            }
            return samples
        }
        #expect(block().allSatisfy { $0 == 0 })
        #expect(rl_sound_check_command(check, 1))
        for _ in 0..<8 { #expect(block().allSatisfy { $0 == 0 }) }
        #expect(abs(rl_sound_check_take_peak(check) - 0.3) < 0.0001)
        #expect(rl_sound_check_command(check, 2))
        #expect(block().allSatisfy { $0 == 0 })
        #expect(rl_sound_check_frames(check) == 4096)
        // Record happened with +6 dB configured. Reprocessing the dry clip at
        // -6 dB must produce 0.15, not a twice-processed 0.3 waveform.
        rl_render_source_controls(render, 0, true, -6.0206, 0, -80, 0, 1)
        #expect(rl_sound_check_command(check, 3))
        var output: [Float] = []
        for _ in 0..<30 { output = block() }
        #expect(abs((output.max() ?? 0) - 0.15) < 0.0001)
        // An effect can emit a tail on silent input. The post-effect hook must
        // silence every output sample as soon as the clip is stopped.
        rl_render_set_group_processor(render, { _, _, samples, frames in
            guard let samples else { return -1 }
            for index in 0..<Int(frames * 2) { samples[index] = 0.25 }
            return 0
        }, nil)
        #expect(rl_sound_check_command(check, 2))
        #expect(block().allSatisfy { $0 == 0 })
        #expect(rl_sound_check_frames(check) == 4096)
    }

    @Test func protocolRejectsExtraDestinationsAndInvalidClipTelemetry() throws {
        let channel = MixerChannel(name: "Test", source: .init(kind: .tone))
        var session = try SoundCheckSession.make(channel: channel, output: .init(deviceUID: "listening"))
        let request = EngineRequest(generation: 1, operation: .startSoundCheck, session: session, soundCheckChannelID: channel.id)
        #expect(try EngineWire.request(EngineWire.encode(request)).operation == .startSoundCheck)
        #expect(try validatedExcludedApplications(["com.example.music", "com.example.music"]) == ["com.example.music"])
        let badExclusion = EngineRequest(generation: 1, operation: .startSoundCheck, session: session, soundCheckChannelID: channel.id, excludedApplications: ["bad\nidentifier"])
        #expect(throws: (any Error).self) { try EngineWire.request(EngineWire.encode(badExclusion)) }
        session.buses[0].additionalOutputs = [.init(deviceUID: "broadcast")]
        let extra = EngineRequest(generation: 1, operation: .startSoundCheck, session: session, soundCheckChannelID: channel.id)
        #expect(throws: (any Error).self) { try EngineWire.request(EngineWire.encode(extra)) }
        var snapshot = EngineSnapshot(); snapshot.running = true
        snapshot.soundCheck = .init(channelID: channel.id, mode: .playing, recordedFrames: 0)
        #expect(throws: (any Error).self) { try EngineWire.response(EngineWire.encode(EngineResponse(generation: 1, kind: .state, snapshot: snapshot))) }
        snapshot.soundCheck = .init(channelID: channel.id, mode: .ready, recordedFrames: 48_000 * 31)
        #expect(throws: (any Error).self) { try EngineWire.response(EngineWire.encode(EngineResponse(generation: 1, kind: .state, snapshot: snapshot))) }
        snapshot.soundCheck = .init(channelID: channel.id, mode: .ready, recordedFrames: 0)
        #expect(throws: (any Error).self) { try EngineWire.response(EngineWire.encode(EngineResponse(generation: 1, kind: .state, snapshot: snapshot))) }
    }
}
