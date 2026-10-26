import Foundation
import Testing
import Darwin
import RaveLinkCore
import RaveLinkAudio

/// Two separate installed virtual devices are recorded by independent C processes
/// in CI. Assertions here cover control delivery and engine health; PCM analysis
/// is deliberately performed outside this executable by verify-matrix.py.
@Suite(.serialized) struct InstalledMatrixTests {
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_TEST_MATRIX"] == "1"))
    @MainActor func installedIndependentMixControlSequence() async throws {
        let audio = EngineClient()
        let aUID = ProcessInfo.processInfo.environment["RAVELINK_MATRIX_A_UID"] ?? "RaveLink.ci-mix-a_UID"
        let bUID = ProcessInfo.processInfo.environment["RAVELINK_MATRIX_B_UID"] ?? "RaveLink.ci-mix-b_UID"
        for _ in 0..<200 where ![aUID, bUID].allSatisfy({ uid in audio.devices.contains { $0.id == uid } }) {
            try await Task.sleep(for: .milliseconds(50))
        }
        for uid in [aUID, bUID] {
            let device = try #require(audio.devices.first { $0.id == uid })
            try #require(device.isVirtual && device.outputChannels >= 2,
                         "This opt-in test only sends audio to named virtual devices")
        }
        let main = MixerChannel(name: "Main calibration", source: .init(kind: .tone))
        var secondary = MixerChannel(name: "Solo calibration", source: .init(kind: .tone), muted: true)
        secondary.effects.gainDB = -12
        let a = MixBus(name: "Independent A", deviceUID: aUID)
        let b = MixBus(name: "Independent B", deviceUID: bUID)
        var session = MixerSession(channels: [main, secondary], buses: [a, b], sends: [
            .init(channelID: main.id, busID: a.id),
            .init(channelID: main.id, busID: b.id, gainDB: -6.0206),
            .init(channelID: secondary.id, busID: a.id, muted: true),
            .init(channelID: secondary.id, busID: b.id)
        ])
        defer { audio.shutdown() }
        try audio.start(session: session)
        for _ in 0..<240 where audio.starting { try await Task.sleep(for: .milliseconds(50)) }
        try #require(audio.running, Comment(rawValue: audio.errorMessage ?? "Engine did not start"))
        func phase(_ name: String) async throws {
            print("MATRIX_PHASE \(name) uptime=\(ProcessInfo.processInfo.systemUptime)")
            try await Task.sleep(for: .milliseconds(1200))
            try #require(audio.running, Comment(rawValue: audio.errorMessage ?? "Engine stopped during \(name)"))
        }
        try await phase("baseline")
        session.sends[0].gainDB = -12; try audio.update(session: session)
        try await phase("route-level-A")
        session.sends[0].pan = -1; try audio.update(session: session)
        try await phase("route-pan-A-left")
        session.sends[0].muted = true; try audio.update(session: session)
        try await phase("route-mute-A")
        session.buses[1].muted = true; try audio.update(session: session)
        try await phase("master-mute-B")
        session.sends[0].muted = false; session.sends[0].pan = 0; session.sends[0].gainDB = 0
        session.buses[1].muted = false; try audio.update(session: session)
        try await phase("restored-baseline")
        session.channels[0].muted = true; try audio.update(session: session)
        try await phase("source-mute")
        session.channels[0].muted = false; session.channels[1].muted = false; session.channels[1].solo = true
        try audio.update(session: session)
        try await phase("secondary-solo")
        session.channels[1].solo = false; try audio.update(session: session)
        try await phase("summed-sources")
        session.channels[0].effects.gainDB = -6.0206; try audio.update(session: session)
        try await phase("source-gain")
        session.buses[1].gainDB = -12; try audio.update(session: session)
        try await phase("master-level-B")
        session.buses[0].muted = true; session.buses[1].muted = true
        try audio.update(session: session)
        try await phase("final-mute")
        #expect(audio.diagnostics.renderedFrames > 48_000 * 20)
        #expect(audio.diagnostics.oversizedBlocks == 0)
        print("MATRIX_EVIDENCE renderedFrames=\(audio.diagnostics.renderedFrames) dropped=\(audio.diagnostics.droppedFrames) underruns=\(audio.diagnostics.underrunFrames)")
        let pid = try #require(audio.helperProcessID)
        audio.shutdown()
        for _ in 0..<200 where kill(pid, 0) == 0 { try await Task.sleep(for: .milliseconds(10)) }
        #expect(kill(pid, 0) != 0, "The actual engine helper must be reaped at the end of the sequence")
    }
}
