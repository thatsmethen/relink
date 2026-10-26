import Foundation
import Testing
import RaveLinkRealtime

@Suite struct SoundCheckBufferTests {
    @Test func dryStereoRecordingLoopsIndependentlyOfNewInput() throws {
        let check = try #require(rl_sound_check_create(8000, 1))
        defer { rl_sound_check_destroy(check) }
        let clip: [Float] = (0..<1600).map { index in
            let frame = index / 2
            return index % 2 == 0 ? Float(frame % 97) / 100 : -Float(frame % 71) / 100
        }
        var recording = clip
        #expect(rl_sound_check_command(check, 1))
        recording.withUnsafeMutableBufferPointer { rl_sound_check_process(check, $0.baseAddress, 800) }
        #expect(recording.allSatisfy { $0 == 0 })
        #expect(rl_sound_check_frames(check) == 800)
        #expect(rl_sound_check_mode(check) == 1)
        #expect(abs(rl_sound_check_take_peak(check) - 0.96) < 0.00001)
        #expect(rl_sound_check_command(check, 2))
        var silence = [Float](repeating: 0.9, count: 128)
        silence.withUnsafeMutableBufferPointer { rl_sound_check_process(check, $0.baseAddress, 64) }
        #expect(rl_sound_check_mode(check) == 2)
        #expect(rl_sound_check_command(check, 3))
        var playback = [Float](repeating: -0.99, count: 3200)
        playback.withUnsafeMutableBufferPointer { rl_sound_check_process(check, $0.baseAddress, 1600) }
        var maxError: Float = 0
        for frame in 0..<1600 {
            let position = frame % 800
            let fade = min(1, Float(min(position, 799 - position)) / 40)
            for side in 0..<2 {
                let expected = clip[position * 2 + side] * fade
                maxError = max(maxError, abs(playback[frame * 2 + side] - expected))
            }
        }
        #expect(maxError < 0.000001)
        #expect(rl_sound_check_frames(check) == 800)
        #expect(rl_sound_check_mode(check) == 3)
    }

    @Test func capacityStopsRecordingAndNeverAutomaticallyPlays() throws {
        let check = try #require(rl_sound_check_create(8000, 0.01))
        defer { rl_sound_check_destroy(check) }
        #expect(rl_sound_check_command(check, 1))
        var input = [Float](repeating: 0.75, count: 512)
        input[21] = .nan; input[27] = .infinity
        input.withUnsafeMutableBufferPointer { rl_sound_check_process(check, $0.baseAddress, 256) }
        #expect(rl_sound_check_frames(check) == 80)
        #expect(rl_sound_check_mode(check) == 2)
        #expect(input.allSatisfy { $0 == 0 })
        #expect(rl_sound_check_command(check, 3))
        input.withUnsafeMutableBufferPointer { rl_sound_check_process(check, $0.baseAddress, 256) }
        #expect(input.allSatisfy { $0.isFinite })
        #expect(input[21] == 0 && input[27] == 0)
    }

    @Test func stoppedRecordingAndPlaybackSuppressEffectTails() throws {
        let check = try #require(rl_sound_check_create(48000, 1))
        defer { rl_sound_check_destroy(check) }
        var samples = [Float](repeating: 0.5, count: 1024)
        samples.withUnsafeMutableBufferPointer { rl_sound_check_process(check, $0.baseAddress, 512) }
        // A reverb/delay could emit nonzero samples even when given silence.
        samples = [Float](repeating: 0.7, count: 1024)
        samples.withUnsafeMutableBufferPointer { rl_sound_check_silence_if_stopped(check, $0.baseAddress, 512) }
        #expect(samples.allSatisfy { $0 == 0 })
        #expect(rl_sound_check_command(check, 1))
        samples = [Float](repeating: 0.5, count: 1024)
        samples.withUnsafeMutableBufferPointer { rl_sound_check_process(check, $0.baseAddress, 512) }
        #expect(rl_sound_check_command(check, 3))
        samples.withUnsafeMutableBufferPointer { rl_sound_check_process(check, $0.baseAddress, 512) }
        #expect(samples.contains { $0 > 0 })
        #expect(rl_sound_check_command(check, 2))
        samples.withUnsafeMutableBufferPointer { rl_sound_check_process(check, $0.baseAddress, 512) }
        samples = [Float](repeating: 0.7, count: 1024)
        samples.withUnsafeMutableBufferPointer { rl_sound_check_silence_if_stopped(check, $0.baseAddress, 512) }
        #expect(samples.allSatisfy { $0 == 0 })
    }

    @Test func rapidCommandsKeepOrderAndMailboxIsBounded() throws {
        let check = try #require(rl_sound_check_create(48000, 1))
        defer { rl_sound_check_destroy(check) }
        var samples = [Float](repeating: 0.5, count: 1024)
        #expect(rl_sound_check_command(check, 1))
        samples.withUnsafeMutableBufferPointer { rl_sound_check_process(check, $0.baseAddress, 512) }
        #expect(rl_sound_check_frames(check) == 512)
        // Recording followed immediately by Stop must discard the previous take.
        #expect(rl_sound_check_command(check, 1)); #expect(rl_sound_check_command(check, 2))
        samples.withUnsafeMutableBufferPointer { rl_sound_check_process(check, $0.baseAddress, 512) }
        #expect(rl_sound_check_frames(check) == 0 && rl_sound_check_mode(check) == 0)
        for _ in 0..<16 { #expect(rl_sound_check_command(check, 4)) }
        #expect(!rl_sound_check_command(check, 1))
        samples.withUnsafeMutableBufferPointer { rl_sound_check_process(check, $0.baseAddress, 512) }
        #expect(rl_sound_check_command(check, 1))
        #expect(!rl_sound_check_command(check, 0)); #expect(!rl_sound_check_command(check, 5))
    }

    @Test func allocationBoundsRejectMalformedRatesAndDurations() {
        for rate: Double in [.nan, .infinity, -1, 0, 7999, 384001] {
            #expect(rl_sound_check_create(rate, 1) == nil)
        }
        for duration: Double in [.nan, .infinity, -1, 0, 30.001] {
            #expect(rl_sound_check_create(48000, duration) == nil)
        }
    }
}
