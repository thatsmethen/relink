import Foundation
import Testing
import Darwin
import RaveLinkCore
import RaveLinkAudio

/// Opt-in disposable-runner test. A separate C HAL client records the output;
/// Python verifies PCM independently of the engine's reported meters.
@Suite(.serialized) struct InstalledAudioUnitAcceptanceTests {
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_TEST_INSTALLED_AU"] == "1"))
    @MainActor func installedAudioUnitGainBypassAndRestoredState() async throws {
        let uid = ProcessInfo.processInfo.environment["RAVELINK_AU_DEVICE_UID"] ?? "RaveLink.ci-loopback_UID"
        let audio = EngineClient()
        defer { audio.shutdown() }
        for _ in 0..<200 where !audio.devices.contains(where: { $0.id == uid }) { try await Task.sleep(for: .milliseconds(50)) }
        let device = try #require(audio.devices.first { $0.id == uid })
        try #require(device.isVirtual && device.outputChannels >= 2, "This test only sends audio to the named virtual device.")
        let catalog = PluginCatalog()
        let component = AudioUnitComponentID(type: 0x61756678, subtype: 0x6c6d7472, manufacturer: 0x6170706c)
        let descriptor = try #require(try await catalog.discover().first { $0.component == component })
        let prepared = try await catalog.render(descriptor, samples: [Float](repeating: 0, count: 1024), parameters: [.init(address: 2, value: 12)])
        let preset = try #require(prepared.state)
        let slot = AudioUnitEffectSlot(descriptor: descriptor, parameters: [.init(address: 2, value: 6.0206)])
        let channel = MixerChannel(name: "Installed AU calibration", source: .init(kind: .tone), audioUnitEffects: [slot])
        let bus = MixBus(name: "Independent AU receiver", deviceUID: uid)
        var session = MixerSession(channels: [channel], buses: [bus], sends: [.init(channelID: channel.id, busID: bus.id)])
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = SessionStore(url: directory.appendingPathComponent("au-session.json"))
        try audio.start(session: session)
        for _ in 0..<240 where audio.starting { try await Task.sleep(for: .milliseconds(50)) }
        try #require(audio.running, Comment(rawValue: audio.errorMessage ?? "Engine did not start"))
        let pid = try #require(audio.helperProcessID)
        var phases: [[String: Any]] = []
        func phase(_ name: String) async throws {
            phases.append(["name": name, "hostUptime": ProcessInfo.processInfo.systemUptime])
            print("AU_HAL_PHASE \(name) uptime=\(ProcessInfo.processInfo.systemUptime)")
            try await Task.sleep(for: .seconds(2))
            try #require(audio.running, Comment(rawValue: audio.errorMessage ?? "Engine stopped during \(name)"))
            #expect(audio.helperProcessID == pid, "Effect controls must preserve the engine process")
        }
        try await phase("active-plus6dB")
        session.channels[0].audioUnitEffects[0].parameters = [.init(address: 2, value: -6.0206)]
        try audio.update(session: session); try await phase("live-minus6dB")
        session.channels[0].audioUnitEffects[0].isBypassed = true
        try audio.update(session: session); try await phase("host-bypass")
        session.channels[0].audioUnitEffects[0].isBypassed = false
        session.channels[0].audioUnitEffects[0].parameters = []
        session.channels[0].audioUnitEffects[0].state = preset
        try store.save(session)
        let restored = try store.load()
        #expect(restored == session)
        try audio.update(session: restored)
        try await phase("restored-plus12dB")
        session.buses[0].muted = true
        try audio.update(session: session); try await phase("final-mute")
        #expect(audio.diagnostics.oversizedBlocks == 0)
        #expect(audio.diagnostics.renderedFrames > 48000)
        if let reportPath = ProcessInfo.processInfo.environment["RAVELINK_AU_REPORT"] {
            let object: [String: Any] = ["deviceUID": uid, "deviceSampleRate": device.sampleRate,
                                        "component": descriptor.component.key, "componentVersion": descriptor.version as Any? ?? NSNull(),
                                        "enginePID": pid, "phases": phases, "presetBytes": preset.propertyList.count,
                                        "oversizedBlocks": audio.diagnostics.oversizedBlocks,
                                        "evidence": "Control sequence metadata only; independent recording is verified separately."]
            try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys]).write(to: URL(fileURLWithPath: reportPath))
        }
        audio.shutdown()
        for _ in 0..<200 where kill(pid, 0) == 0 { try await Task.sleep(for: .milliseconds(10)) }
        #expect(kill(pid, 0) != 0, "The engine helper must be reaped after the sequence")
    }
}
