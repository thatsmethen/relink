import Foundation
import Combine
import Testing
import Darwin
import RaveLinkCore
@testable import RaveLinkAudio

@Suite struct EngineProtocolTests {
    @Test func boundsPartialLinesAndRejectsVersionMismatch() throws {
        var framing = EngineLineBuffer()
        #expect(try framing.append(Data("{\"a\":".utf8)).isEmpty)
        #expect(try framing.append(Data("1}\n{\"b\":2}\n".utf8)).count == 2)
        #expect(!framing.hasPartialLine)
        var oversized = EngineLineBuffer()
        #expect(throws: (any Error).self) { try oversized.append(Data(repeating: 65, count: EngineWire.maximumLineBytes + 1)) }
        let request = EngineRequest(generation: 1, operation: .discover)
        var object = try #require(JSONSerialization.jsonObject(with: EngineWire.encode(request)) as? [String: Any])
        object["version"] = 999
        #expect(throws: (any Error).self) { try EngineWire.request(JSONSerialization.data(withJSONObject: object)) }
    }
    @Test func rejectsContradictoryAndNonfiniteState() throws {
        var snapshot = EngineSnapshot(); snapshot.running = true; snapshot.starting = true
        let bytes = try EngineWire.encode(EngineResponse(generation: 1, kind: .state, snapshot: snapshot))
        #expect(throws: (any Error).self) { try EngineWire.response(bytes) }
        snapshot.starting = false; snapshot.channelMeters[UUID()] = .nan
        #expect(throws: (any Error).self) { try EngineWire.encode(EngineResponse(generation: 1, kind: .state, snapshot: snapshot)) }
    }
}

private struct EngineFixture {
    let directory: URL
    let executable: URL
    let pidFile: URL
    init(_ mode: String) throws {
        directory = FileManager.default.temporaryDirectory.appendingPathComponent("ravelink-engine-test-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        executable = directory.appendingPathComponent("fixture.py"); pidFile = directory.appendingPathComponent("pid")
        let script = """
        #!/usr/bin/python3
        import json, os, signal, sys, time
        from pathlib import Path
        mode = '\(mode)'
        protocol_version = \(EngineWire.version)
        pid_file = Path(__file__).with_name('pid')
        if pid_file.exists():
            try:
                os.kill(int(pid_file.read_text()), 0)
                Path(__file__).with_name('overlap').write_text('previous process was still alive')
            except ProcessLookupError: pass
        pid_file.write_text(str(os.getpid()))
        with Path(__file__).with_name('launches').open('a') as stream: stream.write(str(os.getpid()) + '\\n')
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        if mode == 'crash': os._exit(17)
        if mode == 'hang': time.sleep(30)
        if mode == 'flood':
            sys.stdout.write('X' * (5 * 1024 * 1024)); sys.stdout.flush(); time.sleep(30)
        running = False
        sound_check = None
        def snapshot(active=False):
            value = {'devices': [], 'applications': [], 'running': active, 'starting': False,
                    'channelMeters': [], 'busMeters': [],
                    'diagnostics': {'capturedFrames':0,'renderedFrames':0,'droppedFrames':0,'underrunFrames':0,'oversizedBlocks':0}}
            if sound_check is not None: value['soundCheck'] = sound_check
            return value
        for line in sys.stdin:
            request = json.loads(line)
            operation = request['operation']
            if operation == 'start':
                with Path(__file__).with_name('starts').open('a') as stream: stream.write(request['session']['name'] + '\\n')
            if operation == 'update':
                with Path(__file__).with_name('updates').open('a') as stream: stream.write(request['session']['name'] + '\\n')
            if operation == 'stop' and mode == 'hang_stop': time.sleep(30)
            if operation == 'start' and mode == 'slow_start' and request['session']['name'] == 'old': time.sleep(30)
            if operation in ('start', 'startSoundCheck'): running = True
            if operation == 'startSoundCheck' and mode == 'steady_heartbeat':
                sound_check = {'channelID':request['soundCheckChannelID'],'mode':'empty','recordedFrames':0,
                               'sampleRate':48000,'maximumSeconds':30,'peak':0}
            if operation == 'stop': running = False
            generation = request['generation']
            reply = {'version':protocol_version,'id':request['id'],'generation':generation,'kind':'reply','success':True,'snapshot':snapshot(running)}
            print(json.dumps(reply), flush=True)
            if mode == 'steady_heartbeat' and running:
                for index in range(200):
                    print(json.dumps({'version':protocol_version,'generation':generation,'kind':'state','success':True,'snapshot':snapshot(True)}),flush=True)
                    time.sleep(.02)
                time.sleep(30)
            if mode == 'closed_input':
                os.close(0)
                Path(__file__).with_name('input_closed').write_text('yes')
                time.sleep(30)
            if mode == 'silent_running' and running: time.sleep(30)
            if mode == 'stale_heartbeat' and running:
                while True:
                    print(json.dumps({'version':protocol_version,'generation':generation-1,'kind':'state','success':True,'snapshot':snapshot(True)}),flush=True)
                    time.sleep(.02)
            if mode == 'stale' and operation == 'stop':
                print(json.dumps({'version':protocol_version,'generation':generation-1,'kind':'state','success':True,'snapshot':snapshot(True)}),flush=True)
        """
        try script.write(to: executable, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: executable.path)
    }
    func remove() { try? FileManager.default.removeItem(at: directory) }
    var recordedPID: Int32? { (try? String(contentsOf: pidFile, encoding: .utf8)).flatMap { Int32($0) } }
}
@MainActor private func eventually(_ predicate: () -> Bool, timeout: TimeInterval = 3, message: String = "Expected condition before test deadline", failureDetails: (() -> String)? = nil) async throws {
    let deadline = Date().addingTimeInterval(timeout)
    while !predicate(), Date() < deadline { try await Task.sleep(for: .milliseconds(10)) }
    try #require(predicate(), Comment(rawValue: failureDetails?() ?? message))
}
private func processGone(_ pid: Int32) -> Bool { kill(pid, 0) == -1 && errno == ESRCH }
@MainActor private func fixtureRunning(_ client: EngineClient) async throws {
    // Readiness is setup for control/fault assertions, not the deadline under
    // test. Cold interpreter startup may exceed three seconds on a busy runner.
    try await eventually({ client.running || client.errorMessage != nil }, timeout: 6,
        failureDetails: { "Fixture did not become ready: PID=\(client.helperProcessID.map(String.init) ?? "nil"), error=\(client.errorMessage ?? "nil")" })
    try #require(client.running, Comment(rawValue: client.errorMessage ?? "Fixture never reached running"))
}

@Suite(.serialized) struct EngineClientTests {
    @Test(arguments: [false, true]) @MainActor func identicalRunningHeartbeatsKeepConnectionAliveWithoutPublishing(soundCheck: Bool) async throws {
        let fixture = try EngineFixture("steady_heartbeat"); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(start: 5, heartbeat: 0.25), autoDiscover: false)
        defer { client.shutdown() }
        if soundCheck {
            let channel = MixerChannel(name: "Check", source: .init(kind: .tone))
            let session = try SoundCheckSession.make(channel: channel, output: .init(deviceUID: "fixture-output"))
            try client.startSoundCheck(session: session, channelID: channel.id)
        } else { try client.start(session: .initial) }
        try await fixtureRunning(client)
        let pid = try #require(client.helperProcessID)
        var invalidations = 0, clipPublications = 0
        let subscription = client.objectWillChange.sink { invalidations += 1 }
        let clipSubscription = client.$soundCheck.dropFirst().sink { _ in clipPublications += 1 }
        // More than three heartbeat deadlines: transport must process identical
        // state while the GUI receives no redundant publication.
        try await Task.sleep(for: .milliseconds(850))
        #expect(client.running && client.errorMessage == nil && client.helperProcessID == pid)
        #expect(invalidations == 0 && clipPublications == 0)
        #expect((client.soundCheck != nil) == soundCheck)
        withExtendedLifetime([subscription, clipSubscription]) {}
        client.shutdown()
        try await eventually { processGone(pid) }
    }
    @Test @MainActor func discoveryTimeoutKillsAndReapsUncooperativeChild() async throws {
        let fixture = try EngineFixture("hang"); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(discovery: 1.5))
        defer { client.shutdown() }
        try await eventually({ client.helperProcessID != nil }, message: "Discovery fixture launch was not observed; actor may have been blocked")
        let pid = try #require(client.helperProcessID)
        #expect(kill(pid, SIGSTOP) == 0, "A stopped peer cannot act on TERM; cleanup must escalate to KILL")
        try await eventually({ client.errorMessage != nil }, message: "Discovery deadline did not publish its failure")
        #expect(client.errorMessage?.contains("device discovery") == true)
        #expect(!client.running && !client.starting)
        try await eventually({ processGone(pid) }, message: "Discovery timeout failed to reap stopped helper PID \(pid)")
    }
    @Test @MainActor func abruptExitDoesNotRaiseSIGPIPEInClient() async throws {
        let fixture = try EngineFixture("crash"); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(discovery: 5))
        defer { client.shutdown() }
        try await eventually({ client.errorMessage != nil }, timeout: 6)
        let pid = try #require(fixture.recordedPID, Comment(rawValue: client.errorMessage ?? "Fixture never started"))
        #expect(client.errorMessage?.contains("did not finish") == false, "This case must observe the injected exit, not a startup timeout")
        try await eventually { processGone(pid) }
        #expect(!client.running)
    }
    @Test @MainActor func malformedFloodIsBoundedAndChildReaped() async throws {
        let fixture = try EngineFixture("flood"); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(discovery: 5, start: 5))
        defer { client.shutdown() }
        try await eventually({ client.errorMessage != nil }, timeout: 6)
        let pid = try #require(fixture.recordedPID)
        #expect(client.errorMessage?.contains("4 MiB") == true)
        try await eventually { processGone(pid) }
    }
    @Test @MainActor func brokenPipeWhileChildRemainsAliveDoesNotRaiseSIGPIPE() async throws {
        let fixture = try EngineFixture("closed_input"); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(discovery: 5, start: 5), autoDiscover: false)
        defer { client.shutdown() }
        try client.start(session: .initial)
        try await fixtureRunning(client)
        try await eventually({ FileManager.default.fileExists(atPath: fixture.directory.appendingPathComponent("input_closed").path) || client.errorMessage != nil }, timeout: 6)
        try #require(client.errorMessage == nil)
        let pid = try #require(client.helperProcessID)
        #expect(kill(pid, 0) == 0, "The peer must remain alive so the next write reaches the EPIPE branch")
        client.stop()
        try await eventually { client.errorMessage != nil }
        #expect(client.errorMessage?.contains("closed its control channel") == true)
        try await eventually { processGone(pid) }
    }
    @Test(arguments: ["silent_running", "stale_heartbeat"]) @MainActor func runningHeartbeatDeadlineCannotBeMaskedByStaleState(_ mode: String) async throws {
        let fixture = try EngineFixture(mode); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(start: 5, heartbeat: 0.25), autoDiscover: false)
        defer { client.shutdown() }
        try client.start(session: .initial)
        try await eventually({ client.running || client.errorMessage != nil }, timeout: 6)
        try #require(client.running, Comment(rawValue: client.errorMessage ?? "Fixture did not start"))
        let pid = try #require(client.helperProcessID)
        try await eventually { client.errorMessage?.contains("stopped reporting its running state") == true }
        #expect(!client.running)
        try await eventually { processGone(pid) }
    }
    @Test @MainActor func sleepReapsActiveHelperAndWakeDiscoversWithoutResuming() async throws {
        let fixture = try EngineFixture("echo"); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(discovery: 5, start: 5), autoDiscover: false)
        defer { client.shutdown() }
        try client.start(session: .initial)
        try await fixtureRunning(client)
        let oldPID = try #require(client.helperProcessID)
        client.handlePowerEvent(.sleep)
        #expect(!client.running && !client.starting)
        #expect(client.errorMessage?.contains("sleeping") == true)
        #expect(throws: (any Error).self) { try client.start(session: .initial) }
        try await eventually { processGone(oldPID) }
        let sleepMessage = client.errorMessage
        client.handlePowerEvent(.wake)
        try await eventually({
            if client.errorMessage != sleepMessage && client.helperProcessID == nil { return true }
            guard let pid = client.helperProcessID, pid != oldPID else { return false }
            return fixture.recordedPID == pid
        }, timeout: 6)
        try #require(client.errorMessage == sleepMessage, Comment(rawValue: client.errorMessage ?? "Unexpected wake discovery state"))
        try await Task.sleep(for: .milliseconds(100))
        #expect(!client.running && !client.starting)
        let starts = try String(contentsOf: fixture.directory.appendingPathComponent("starts"), encoding: .utf8)
        #expect(starts.split(separator: "\n").count == 1, "Wake may discover devices but must not resend the saved mix")
        let newPID = try #require(client.helperProcessID)
        client.shutdown(); try await eventually { processGone(newPID) }
    }
    @Test @MainActor func missingSoundCheckSnapshotStopsAndReapsHelper() async throws {
        let fixture = try EngineFixture("echo"); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(discovery: 5, start: 5), autoDiscover: false)
        defer { client.shutdown() }
        let channel = MixerChannel(name: "Check", source: .init(kind: .tone))
        let session = try SoundCheckSession.make(channel: channel, output: .init(deviceUID: "fixture-output"))
        try client.startSoundCheck(session: session, channelID: channel.id)
        try await eventually({ client.errorMessage != nil }, timeout: 6)
        #expect(client.errorMessage?.contains("stopped reporting its Sound Check") == true)
        #expect(!client.running && client.soundCheck == nil)
        let pid = try #require(fixture.recordedPID)
        try await eventually { processGone(pid) }
    }
    @Test @MainActor func stopHasIndependentDeadlineAndReapsHungHelper() async throws {
        let fixture = try EngineFixture("hang_stop"); defer { fixture.remove() }
        // Startup latency is independent of the stop deadline under test.
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(start: 5, stop: 0.2), autoDiscover: false)
        defer { client.shutdown() }
        try client.start(session: .initial)
        try await eventually({ client.running || client.errorMessage != nil }, timeout: 6)
        try #require(client.running, Comment(rawValue: client.errorMessage ?? "Fixture did not finish startup"))
        let pid = try #require(client.helperProcessID)
        client.stop()
        try await eventually { client.errorMessage?.contains("finish stop") == true }
        try await eventually { processGone(pid) }
    }
    @Test @MainActor func startHasHardRoutingDeadline() async throws {
        let fixture = try EngineFixture("slow_start"); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(start: 1.5), autoDiscover: false)
        defer { client.shutdown() }
        var session = MixerSession.initial; session.name = "old"
        try client.start(session: session)
        try await eventually { client.helperProcessID != nil }
        let pid = try #require(client.helperProcessID)
        try await eventually { client.errorMessage != nil }
        #expect(client.errorMessage?.contains("audio routing") == true)
        #expect(!client.running && !client.starting)
        try await eventually { processGone(pid) }
    }
    @Test @MainActor func staleTelemetryCannotRestartStoppedSession() async throws {
        let fixture = try EngineFixture("stale"); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(discovery: 5, start: 5), autoDiscover: false)
        defer { client.shutdown() }
        try client.start(session: .initial)
        try await eventually({ client.running || client.errorMessage != nil }, timeout: 6, failureDetails: {
            "Stale fixture startup: PID=\(client.helperProcessID.map(String.init) ?? "nil"), recordedPID=\(fixture.recordedPID.map(String.init) ?? "nil"), error=\(client.errorMessage ?? "nil")"
        })
        try #require(client.running, Comment(rawValue: client.errorMessage ?? "Stale fixture did not reach running"))
        let pid = try #require(client.helperProcessID)
        client.stop()
        try await Task.sleep(for: .milliseconds(150))
        #expect(!client.running && !client.starting)
        #expect(client.errorMessage == nil)
        client.shutdown()
        try await eventually({ processGone(pid) }, message: "Stale fixture helper was not reaped after shutdown")
    }
    @Test @MainActor func editDuringStartReplacesOldProcessWithLatestSession() async throws {
        let fixture = try EngineFixture("slow_start"); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(discovery: 5, start: 5), autoDiscover: false)
        defer { client.shutdown() }
        var session = MixerSession.initial; session.name = "old"
        try client.start(session: session)
        try await eventually { client.helperProcessID != nil }
        let oldPID = try #require(client.helperProcessID)
        session.name = "new"
        try client.update(session: session)
        try await fixtureRunning(client)
        let newPID = try #require(client.helperProcessID)
        #expect(newPID != oldPID)
        #expect(!FileManager.default.fileExists(atPath: fixture.directory.appendingPathComponent("overlap").path), "Replacement must start after the previous child is reaped")
        try await eventually { processGone(oldPID) }
        client.shutdown()
        try await eventually { processGone(newPID) }
    }
    @Test @MainActor func sustainedControlInputSendsUpdatesBeforeProducerStops() async throws {
        let fixture = try EngineFixture("echo"); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(discovery: 5, start: 5), autoDiscover: false)
        defer { client.shutdown() }
        try client.start(session: .initial)
        try await fixtureRunning(client)
        let log = fixture.directory.appendingPathComponent("updates")
        var deliveredDuringInput = false
        for index in 0..<40 {
            var session = MixerSession.initial; session.name = "edit-\(index)"
            try client.update(session: session)
            try await Task.sleep(for: .milliseconds(5))
            if index < 35 && FileManager.default.fileExists(atPath: log.path) { deliveredDuringInput = true }
        }
        #expect(deliveredDuringInput, "A continuous drag must not postpone every update until release")
        try await eventually { (try? String(contentsOf: log, encoding: .utf8).contains("edit-39")) == true }
        let pid = try #require(client.helperProcessID)
        client.shutdown()
        try await eventually { processGone(pid) }
    }
    @Test @MainActor func pendingReplacementCoalescesRapidStartEdits() async throws {
        let fixture = try EngineFixture("slow_start"); defer { fixture.remove() }
        let client = EngineClient(helperURL: fixture.executable, timeouts: .init(discovery: 5, start: 5), autoDiscover: false)
        defer { client.shutdown() }
        var session = MixerSession.initial; session.name = "old"
        try client.start(session: session)
        let starts = fixture.directory.appendingPathComponent("starts")
        try await eventually({ (try? String(contentsOf: starts, encoding: .utf8).contains("old")) == true || client.errorMessage != nil }, timeout: 6)
        try #require(client.errorMessage == nil, Comment(rawValue: client.errorMessage ?? "Old fixture start was not observed"))
        for index in 0..<100 {
            session.name = "pending-\(index)"
            try client.update(session: session)
        }
        try await fixtureRunning(client)
        let launches = try String(contentsOf: fixture.directory.appendingPathComponent("launches"), encoding: .utf8).split(separator: "\n")
        #expect(launches.count == 2)
        #expect(try String(contentsOf: starts, encoding: .utf8).contains("pending-99"))
        #expect(!FileManager.default.fileExists(atPath: fixture.directory.appendingPathComponent("overlap").path))
        let pid = try #require(client.helperProcessID)
        client.shutdown()
        try await eventually { processGone(pid) }
    }
    @Test @MainActor func realHelperExitsWhenParentControlPipeCloses() async throws {
        let helper = try #require(EngineClient.defaultHelperURL())
        let process = Process(), input = Pipe()
        process.executableURL = helper; process.standardInput = input
        process.standardOutput = FileHandle.nullDevice; process.standardError = FileHandle.nullDevice
        try process.run()
        let pid = process.processIdentifier
        try input.fileHandleForWriting.close()
        try await eventually { !process.isRunning }
        process.waitUntilExit()
        #expect(process.terminationStatus == 0)
        #expect(processGone(pid))
    }
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_TEST_SILENT_OUTPUT"] == "1")) @MainActor func realSilentOutputThroughSupervisedProcess() async throws {
        let helper = try #require(EngineClient.defaultHelperURL())
        let client = EngineClient(helperURL: helper)
        defer { client.shutdown() }
        let uid = ProcessInfo.processInfo.environment["RAVELINK_SILENT_OUTPUT_UID"] ?? "BuiltInSpeakerDevice"
        try await eventually({ client.devices.contains { $0.id == uid } }, timeout: 10)
        let source = MixerChannel(name: "Silent IPC check", source: .init(kind: .tone), muted: true)
        let bus = MixBus(name: "Silent output", deviceUID: uid)
        let session = MixerSession(channels: [source], buses: [bus], sends: [.init(channelID: source.id, busID: bus.id, muted: true)])
        try client.start(session: session)
        try await eventually({ client.running || client.errorMessage != nil }, timeout: 14)
        try #require(client.running, Comment(rawValue: client.errorMessage ?? "Engine did not start"))
        let pid = try #require(client.helperProcessID)
        // Survive two periodic health checks with the real device/listener path.
        try await eventually({ client.diagnostics.renderedFrames > 200000 || client.errorMessage != nil }, timeout: 7)
        #expect(client.running, Comment(rawValue: client.errorMessage ?? "Silent route unexpectedly stopped"))
        #expect(client.busMeters[bus.id] == 0)
        print("SUPERVISED_SILENT_OUTPUT uid=\(uid) frames=\(client.diagnostics.renderedFrames) peak=\(client.busMeters[bus.id] ?? -1) helperPID=\(pid)")
        client.stop()
        try await Task.sleep(for: .milliseconds(150))
        #expect(client.errorMessage == nil)
        client.shutdown()
        try await eventually { processGone(pid) }
    }
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_TEST_SILENT_OUTPUT"] == "1")) @MainActor func soundCheckCommandsAndLiveEditsThroughSilentDevice() async throws {
        let helper = try #require(EngineClient.defaultHelperURL())
        let client = EngineClient(helperURL: helper)
        defer { client.shutdown() }
        let uid = ProcessInfo.processInfo.environment["RAVELINK_SILENT_OUTPUT_UID"] ?? "BuiltInSpeakerDevice"
        try await eventually({ client.devices.contains { $0.id == uid } }, timeout: 10)
        let channel = MixerChannel(name: "Silent Sound Check", source: .init(kind: .tone))
        var session = try SoundCheckSession.make(channel: channel, output: .init(deviceUID: uid))
        session.buses[0].muted = true // Playback also remains silent on physical hardware.
        try client.startSoundCheck(session: session, channelID: channel.id)
        try await eventually({ client.running || client.errorMessage != nil }, timeout: 14)
        try #require(client.running, Comment(rawValue: client.errorMessage ?? "Sound Check did not start"))
        let pid = try #require(client.helperProcessID)
        try client.soundCheckCommand(.record)
        try await eventually { (client.soundCheck?.recordedFrames ?? 0) > 2048 }
        #expect(client.busMeters.values.allSatisfy { $0 == 0 })
        try client.soundCheckCommand(.stop)
        try await eventually { client.soundCheck?.mode == .ready }
        let frames = try #require(client.soundCheck?.recordedFrames)
        session.channels[0].effects.gainDB = -12
        try client.update(session: session)
        try await Task.sleep(for: .milliseconds(100))
        #expect(client.soundCheck?.recordedFrames == frames)
        #expect(client.helperProcessID == pid)
        try client.soundCheckCommand(.play)
        try await eventually { client.soundCheck?.mode == .playing }
        #expect(client.soundCheck?.recordedFrames == frames)
        #expect(client.busMeters.values.allSatisfy { $0 == 0 })
        print("SUPERVISED_SOUND_CHECK uid=\(uid) recordedFrames=\(frames) retainedAfterLiveEdit=true playbackOutputPeak=0")
        session.channels[0].source = .init(kind: .device, deviceUID: "different-source")
        try client.update(session: session)
        try await eventually { client.errorMessage?.contains("temporary clip was discarded") == true }
        #expect(!client.running && client.soundCheck == nil)
        try await eventually { processGone(pid) }
    }
}
