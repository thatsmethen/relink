import Darwin
import Foundation
import Testing
import RaveLinkCore
import RaveLinkAudio

/// The companion harness independently records the audition and a separate
/// guard output. This opt-in test never selects a physical listening device.
@Suite(.serialized) struct InstalledSoundCheckTests {
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_TEST_SOUND_CHECK"] == "1"))
    @MainActor func installedSoundCheckDryRecordLoopGainAndSilence() async throws {
        let environment = ProcessInfo.processInfo.environment
        let outputUID = environment["RAVELINK_SOUND_CHECK_UID"] ?? "RaveLink.ci-mix-a_UID"
        let guardUID = environment["RAVELINK_SOUND_CHECK_GUARD_UID"] ?? "RaveLink.ci-mix-b_UID"
        let report = URL(fileURLWithPath: environment["RAVELINK_SOUND_CHECK_REPORT"] ?? "build/sound-check-engine.json")
        try #require(outputUID != guardUID)
        let audio = EngineClient()
        defer { audio.shutdown() }
        for _ in 0..<240 where ![outputUID, guardUID].allSatisfy({ uid in audio.devices.contains { $0.id == uid } }) {
            try await Task.sleep(for: .milliseconds(50))
        }
        for uid in [outputUID, guardUID] {
            let device = try #require(audio.devices.first { $0.id == uid })
            try #require(device.isVirtual && device.inputChannels >= 2 && device.outputChannels >= 2,
                         "Sound Check acceptance only uses explicitly named virtual devices")
        }
        var input = MixerChannel(name: "Dry Sound Check calibration", source: .init(kind: .tone))
        input.effects.gainDB = -12
        var session = try SoundCheckSession.make(channel: input, output: .init(deviceUID: outputUID))
        var events: [[String: Any]] = []
        func state(_ expected: SoundCheckMode) async throws -> SoundCheckSnapshot {
            for _ in 0..<200 where audio.soundCheck?.mode != expected {
                try await Task.sleep(for: .milliseconds(10))
            }
            let snapshot = try #require(audio.soundCheck)
            try #require(audio.running && snapshot.mode == expected,
                         Comment(rawValue: audio.errorMessage ?? "Sound Check did not enter \(expected.rawValue)"))
            events.append(["mode": snapshot.mode.rawValue, "recordedFrames": snapshot.recordedFrames,
                           "uptime": ProcessInfo.processInfo.systemUptime])
            return snapshot
        }
        try audio.startSoundCheck(session: session, channelID: input.id)
        for _ in 0..<300 where audio.starting { try await Task.sleep(for: .milliseconds(50)) }
        _ = try await state(.empty)
        try await Task.sleep(for: .milliseconds(500))
        try audio.soundCheckCommand(.record)
        _ = try await state(.recording)
        try await Task.sleep(for: .milliseconds(800))
        try audio.soundCheckCommand(.stop)
        let recorded = try await state(.ready)
        try #require(recorded.duration >= 0.6 && recorded.duration <= 2)
        try await Task.sleep(for: .seconds(1))
        // The clip was recorded with a -12dB channel effect. Unity playback
        // after this edit proves the persisted clip contains pre-effect audio.
        session.channels[0].effects.gainDB = 0
        try audio.update(session: session)
        try await Task.sleep(for: .milliseconds(200))
        try audio.soundCheckCommand(.play)
        _ = try await state(.playing)
        try await Task.sleep(for: .milliseconds(3200))
        session.channels[0].effects.gainDB = -12
        try audio.update(session: session)
        try await Task.sleep(for: .milliseconds(3200))
        try #require(audio.running && audio.soundCheck?.mode == .playing)
        try audio.soundCheckCommand(.stop)
        _ = try await state(.ready)
        try await Task.sleep(for: .seconds(1))
        try audio.soundCheckCommand(.clear)
        let cleared = try await state(.empty)
        try #require(cleared.recordedFrames == 0)
        try await Task.sleep(for: .milliseconds(500))
        let pid = try #require(audio.helperProcessID)
        audio.shutdown()
        for _ in 0..<200 where kill(pid, 0) == 0 { try await Task.sleep(for: .milliseconds(10)) }
        let reaped = kill(pid, 0) != 0
        let evidence: [String: Any] = [
            "completed": true, "helperReaped": reaped, "initialGainDB": -12,
            "recordedFrames": recorded.recordedFrames, "sampleRate": recorded.sampleRate,
            "finalMode": cleared.mode.rawValue, "finalFrames": cleared.recordedFrames,
            "visibleInputGroups": 1, "mixBuses": 1, "outputUID": outputUID, "guardUID": guardUID,
            "osVersion": ProcessInfo.processInfo.operatingSystemVersionString, "stateEvents": events,
            "evidence": "Supervised Sound Check state sequence; independent PCM determines actual silence, dry recording and loop gain"
        ]
        try JSONSerialization.data(withJSONObject: evidence, options: [.prettyPrinted, .sortedKeys]).write(to: report, options: .atomic)
        #expect(reaped, "The audition helper must be reaped after closing Sound Check")
        print("SOUND_CHECK_EVIDENCE \(report.path)")
    }
}
