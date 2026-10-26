import Foundation
import Testing
import Darwin
@testable import RaveLinkAudio

@Suite struct PluginCatalogTests {
    private let validList = """
    {"protocolVersion":1,"operation":"list","status":"ok","components":[{"component":{"type":1635083896,"subtype":1819112562,"manufacturer":1634758764},"name":"Apple: AUPeakLimiter","version":1,"flags":0}],"limitations":["Metadata only"]}
    """
    private func script(_ body: String) throws -> (URL, URL) {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let executable = directory.appendingPathComponent("RaveLinkPluginHost")
        try Data(("#!/bin/sh\n" + body + "\n").utf8).write(to: executable)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: executable.path)
        return (directory, executable)
    }
    @Test func strictProtocolRejectsVersionOperationAndCorruptPayload() throws {
        let bytes = Data(validList.utf8)
        let response = try PluginResponse.parse(bytes, operation: "list")
        #expect(response.components?.count == 1)
        #expect(throws: PluginCatalogError.operationMismatch) { try PluginResponse.parse(bytes, operation: "probe") }
        #expect(throws: PluginCatalogError.unsupportedProtocol(9)) {
            try PluginResponse.parse(Data(validList.replacingOccurrences(of: "\"protocolVersion\":1", with: "\"protocolVersion\":9").utf8), operation: "list")
        }
        #expect(throws: PluginCatalogError.malformedResponse) { try PluginResponse.parse(Data("not JSON".utf8), operation: "list") }
        #expect(throws: PluginCatalogError.malformedResponse) {
            try PluginResponse.parse(Data("{\"protocolVersion\":1,\"operation\":\"list\",\"status\":\"ok\"}".utf8), operation: "list")
        }
    }
    @Test func duplicateOrNonEffectMetadataIsRejected() throws {
        var object = try #require(JSONSerialization.jsonObject(with: Data(validList.utf8)) as? [String: Any])
        let entries = try #require(object["components"] as? [[String: Any]])
        object["components"] = entries + entries
        #expect(throws: PluginCatalogError.malformedResponse) { try PluginResponse.parse(JSONSerialization.data(withJSONObject: object), operation: "list") }
        let unrelated = validList.replacingOccurrences(of: "1635083896", with: "1")
        #expect(throws: PluginCatalogError.malformedResponse) { try PluginResponse.parse(Data(unrelated.utf8), operation: "list") }
    }
    @Test func childResponseIsDecodedWithoutLoadingAnAudioUnit() async throws {
        let (directory, executable) = try script("printf '%s\\n' '\(validList)'")
        defer { try? FileManager.default.removeItem(at: directory) }
        let list = try await PluginCatalog(helperURL: executable).discover(timeout: 10)
        #expect(list.count == 1)
        #expect(list[0].name == "Apple: AUPeakLimiter")
    }
    @Test func timedOutChildIgnoringTerminationIsKilled() async throws {
        let (directory, executable) = try script("trap '' TERM\nprintf '%s' \"$$\" > \"$0.pid\"\nexec /bin/sleep 30")
        defer { try? FileManager.default.removeItem(at: directory) }
        let start = ContinuousClock.now
        do { _ = try await PluginCatalog(helperURL: executable).discover(timeout: 2); Issue.record("Expected timeout") }
        catch { #expect(error as? PluginCatalogError == .timeout) }
        #expect(start.duration(to: .now) < .seconds(4))
        // Verify the script reached its TERM-ignoring branch, and the child no
        // longer exists after the deadline rather than merely timing out a task.
        let pidText = try String(contentsOf: executable.appendingPathExtension("pid"), encoding: .utf8)
        let pid = try #require(Int32(pidText))
        #expect(kill(pid, 0) == -1 && errno == ESRCH)
    }
    @Test func cancellationStopsChild() async throws {
        let (directory, executable) = try script("exec /bin/sleep 30")
        defer { try? FileManager.default.removeItem(at: directory) }
        let task = Task { try await PluginCatalog(helperURL: executable).discover(timeout: 30) }
        try await Task.sleep(for: .milliseconds(100))
        task.cancel()
        do { _ = try await task.value; Issue.record("Expected cancellation") }
        catch { #expect(error as? PluginCatalogError == .cancelled) }
    }
    @Test func crashedChildAndOutputFloodAreContained() async throws {
        let (crashDirectory, crash) = try script("kill -KILL $$")
        defer { try? FileManager.default.removeItem(at: crashDirectory) }
        do { _ = try await PluginCatalog(helperURL: crash).discover(timeout: 10); Issue.record("Expected crash") }
        catch { #expect(error as? PluginCatalogError == .crashed(SIGKILL)) }
        let (floodDirectory, flood) = try script("exec /usr/bin/yes x")
        defer { try? FileManager.default.removeItem(at: floodDirectory) }
        do { _ = try await PluginCatalog(helperURL: flood).discover(timeout: 10); Issue.record("Expected output limit") }
        catch { #expect(error as? PluginCatalogError == .outputLimit) }
    }
    @Test func typedHelperErrorsSurviveNonzeroExit() async throws {
        let (directory, executable) = try script("printf '%s\\n' '{\"protocolVersion\":1,\"operation\":\"list\",\"status\":\"error\",\"error\":{\"code\":\"catalog_unavailable\",\"message\":\"Catalog unavailable\"}}'\nexit 2")
        defer { try? FileManager.default.removeItem(at: directory) }
        do { _ = try await PluginCatalog(helperURL: executable).discover(timeout: 10); Issue.record("Expected typed error") }
        catch { #expect(error as? PluginCatalogError == .helperError("catalog_unavailable", "Catalog unavailable")) }
    }
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_PLUGIN_PROBE_APPLE"] == "1"))
    func selectedAppleLimiterAcceptsStereoOfflineAllocation() async throws {
        let helper = try #require(PluginCatalog.defaultHelperURL())
        let catalog = PluginCatalog(helperURL: helper)
        let listed = try await catalog.discover()
        let selected = try #require(listed.first { $0.component == .init(type: 0x61756678, subtype: 0x6c6d7472, manufacturer: 0x6170706c) })
        let result = try await catalog.probe(selected, sampleRate: 48_000, channels: 2)
        #expect(result.descriptor.component == selected.component)
        #expect(result.configurationAccepted)
        #expect(result.renderResourcesAllocated)
        #expect(!result.parameters.isEmpty)
        #expect(result.inputBuses.first?.channels == 2)
        #expect(result.outputBuses.first?.sampleRate == 48_000)
    }
}
