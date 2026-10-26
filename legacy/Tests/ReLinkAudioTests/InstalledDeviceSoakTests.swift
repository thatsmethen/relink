import Darwin
import Foundation
import Testing
import RaveLinkCore
import RaveLinkAudio

/// Explicit CI acceptance only. An independent process feeds a real HAL input;
/// two independent recorders inspect all eight destination stereo pairs.
@Suite(.serialized) struct InstalledDeviceSoakTests {
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_TEST_DEVICE_SOAK"] == "1"))
    @MainActor func installedGroupedDeviceInputThirtySecondSoak() async throws {
        let env = ProcessInfo.processInfo.environment
        let inputUID = env["RAVELINK_SOAK_INPUT_UID"] ?? "RaveLink.ci-loopback_UID"
        let outputUIDs = [env["RAVELINK_MATRIX_A_UID"] ?? "RaveLink.ci-mix-a_UID", env["RAVELINK_MATRIX_B_UID"] ?? "RaveLink.ci-mix-b_UID"]
        let go = URL(fileURLWithPath: env["RAVELINK_SOAK_GO_FILE"] ?? "build/soak-go")
        let report = URL(fileURLWithPath: env["RAVELINK_SOAK_REPORT"] ?? "build/soak-engine.json")
        try #require(!FileManager.default.fileExists(atPath: go.path), "Remove stale soak trigger before running acceptance")
        let audio = AudioCoordinator()
        defer { audio.stop() }
        let uids = [inputUID] + outputUIDs
        try #require(Set(uids).count == 3, "Input and both output devices must be distinct")
        for _ in 0..<240 where !uids.allSatisfy({ uid in audio.devices.contains { $0.id == uid } }) {
            try await Task.sleep(for: .milliseconds(50))
        }
        let devices = try uids.map { uid in try #require(audio.devices.first { $0.id == uid }) }
        for device in devices {
            try #require(device.isVirtual && device.inputChannels >= 8 && device.outputChannels >= 8,
                         "This opt-in test only uses explicitly named eight-channel virtual devices")
        }
        let groups = 12, mixes = 8
        var session = MixerSession(name: "Device input acceptance")
        for i in 0..<mixes {
            session.addBus(.init(name: "Pair \(i + 1)", deviceUID: outputUIDs[i / 4], channelOffset: (i % 4) * 2, gainDB: Float(-3 * i)))
        }
        for i in 0..<groups {
            session.addChannel(.init(name: "Device group \(i + 1)", source: .init(kind: .device, deviceUID: inputUID),
                                     additionalSources: [.init(kind: .device, deviceUID: inputUID, channelOffset: 2)]))
        }
        for i in session.sends.indices {
            session.sends[i].muted = false
            session.sends[i].gainDB = Float(-20 * log10(Double(groups)))
        }
        try audio.start(session: session)
        for _ in 0..<300 where audio.starting { try await Task.sleep(for: .milliseconds(50)) }
        try #require(audio.running, Comment(rawValue: audio.errorMessage ?? "Device graph did not start"))
        try await Task.sleep(for: .seconds(3)) // Explicitly excluded startup/priming interval.
        let baseline = audio.diagnostics
        try #require(baseline.captureSourceCount == 2,
                     "The 24 logical inputs must share exactly two unique physical capture contexts")
        var resourceStart = rusage(); getrusage(RUSAGE_SELF, &resourceStart)
        let started = ProcessInfo.processInfo.systemUptime
        try Data("go\n".utf8).write(to: go, options: .atomic)
        var samples: [[String: Any]] = []
        for _ in 0..<60 {
            try await Task.sleep(for: .milliseconds(500))
            try #require(audio.running, Comment(rawValue: audio.errorMessage ?? "Engine stopped during measured interval"))
            let stats = audio.diagnostics
            samples.append(["uptime": ProcessInfo.processInfo.systemUptime, "capturedFrames": stats.capturedFrames,
                            "renderedFrames": stats.renderedFrames, "droppedFrames": stats.droppedFrames,
                            "underrunFrames": stats.underrunFrames, "oversizedBlocks": stats.oversizedBlocks,
                            "physicalCaptureSources": stats.captureSourceCount])
        }
        let ended = ProcessInfo.processInfo.systemUptime, stats = audio.diagnostics
        var resourceEnd = rusage(); getrusage(RUSAGE_SELF, &resourceEnd)
        func cpu(_ value: rusage) -> Double {
            Double(value.ru_utime.tv_sec + value.ru_stime.tv_sec) + Double(value.ru_utime.tv_usec + value.ru_stime.tv_usec) * 1e-6
        }
        let elapsed = ended - started
        let captured = stats.capturedFrames - baseline.capturedFrames
        let rendered = stats.renderedFrames - baseline.renderedFrames
        let dropped = stats.droppedFrames - baseline.droppedFrames
        let underruns = stats.underrunFrames - baseline.underrunFrames
        let oversized = stats.oversizedBlocks - baseline.oversizedBlocks
        let expectedCapture = devices[0].sampleRate * Double(baseline.captureSourceCount) * elapsed
        let expectedRender = devices.dropFirst().reduce(0) { $0 + $1.sampleRate } * elapsed
        for i in session.buses.indices { session.buses[i].muted = true }
        try audio.update(session: session)
        try await Task.sleep(for: .seconds(2))
        let evidence: [String: Any] = [
            "evidence": "Native device input routing on the named installed virtual devices; host-specific measured envelope",
            "visibleInputGroups": groups, "logicalCaptureSources": groups * 2,
            "physicalCaptureSources": stats.captureSourceCount, "mixBuses": mixes, "physicalOutputs": outputUIDs.count,
            "parentRoutes": groups * mixes, "expandedRoutes": groups * 2 * mixes,
            "warmupSeconds": 3, "measuredSeconds": elapsed, "captureFrames": captured, "renderFrames": rendered,
            "droppedFrames": dropped, "underrunFrames": underruns, "oversizedBlocks": oversized,
            "startupDroppedFrames": baseline.droppedFrames, "startupUnderrunFrames": baseline.underrunFrames,
            "cpuSeconds": cpu(resourceEnd) - cpu(resourceStart), "cpuCoreEquivalent": (cpu(resourceEnd) - cpu(resourceStart)) / elapsed,
            "processPeakRSSBytes": resourceEnd.ru_maxrss, "logicalProcessors": ProcessInfo.processInfo.activeProcessorCount,
            "physicalMemoryBytes": ProcessInfo.processInfo.physicalMemory, "osVersion": ProcessInfo.processInfo.operatingSystemVersionString,
            "devices": devices.map { ["uid": $0.id, "sampleRate": $0.sampleRate, "inputChannels": $0.inputChannels, "outputChannels": $0.outputChannels] as [String: Any] },
            "counterSamples": samples
        ]
        try JSONSerialization.data(withJSONObject: evidence, options: [.prettyPrinted, .sortedKeys]).write(to: report, options: .atomic)
        #expect(elapsed >= 30 && elapsed < 35)
        #expect(stats.captureSourceCount == 2)
        #expect(Double(captured) >= expectedCapture * 0.97 && Double(captured) <= expectedCapture * 1.03)
        #expect(Double(rendered) >= expectedRender * 0.97 && Double(rendered) <= expectedRender * 1.03)
        #expect(dropped == 0 && underruns == 0 && oversized == 0)
        print("DEVICE_SOAK_EVIDENCE \(report.path)")
    }
}
