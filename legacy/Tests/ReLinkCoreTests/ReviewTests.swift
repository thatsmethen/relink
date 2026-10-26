import Foundation
import Testing
@testable import RaveLinkCore

@Suite struct ReviewTests {
    private func temporaryStore() throws -> (URL, SessionStore) {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return (directory, SessionStore(url: directory.appendingPathComponent("session.json")))
    }

    @Test func testOversizedSparseImportIsRejectedWithoutChangingOriginal() throws {
        let (directory, store) = try temporaryStore()
        defer { try? FileManager.default.removeItem(at: directory) }
        #expect(FileManager.default.createFile(atPath: store.url.path, contents: Data("original".utf8)))
        let handle = try FileHandle(forWritingTo: store.url)
        let fileSize = UInt64(SessionStore.maximumDataBytes) * 64
        try handle.truncate(atOffset: fileSize)
        try handle.close()
        do {
            _ = try store.load()
            Issue.record("Oversized import should fail")
        } catch {
            #expect(error.localizedDescription.contains("32 MB"))
        }
        let attributes = try FileManager.default.attributesOfItem(atPath: store.url.path)
        #expect((attributes[.size] as? NSNumber)?.uint64Value == fileSize)
        let reader = try FileHandle(forReadingFrom: store.url)
        defer { try? reader.close() }
        #expect(try reader.read(upToCount: 8) == Data("original".utf8))
    }

    @Test func testOversizedSavePreservesExistingReadableSession() throws {
        let (directory, store) = try temporaryStore()
        defer { try? FileManager.default.removeItem(at: directory) }
        let original = MixerSession.initial
        try store.save(original)
        let originalBytes = try Data(contentsOf: store.url)
        var oversized = original
        oversized.name = String(repeating: "x", count: SessionStore.maximumDataBytes)
        do {
            try store.save(oversized)
            Issue.record("Oversized save should fail")
        } catch {
            #expect(error.localizedDescription.contains("32 MB"))
        }
        #expect(try Data(contentsOf: store.url) == originalBytes)
        #expect(try store.load() == original)
    }

    @Test func testInvalidImportDoesNotRewriteSource() throws {
        let (directory, store) = try temporaryStore()
        defer { try? FileManager.default.removeItem(at: directory) }
        let original = Data("{\"schemaVersion\":999,\"name\":\"future\"}".utf8)
        try original.write(to: store.url)
        #expect(throws: (any Error).self) { try store.load() }
        #expect(try Data(contentsOf: store.url) == original)
    }

    @Test func testIncompleteImportedMatrixIsRejectedAndEmptyAxesAreAllowed() throws {
        var session = MixerSession.initial
        session.addChannel(.init(name: "Voice", source: .init(kind: .tone)))
        session.sends.removeFirst()
        #expect(throws: SessionError.self) { try session.validated() }
        #expect(throws: Never.self) { try MixerSession.initial.validated() }
        let noMixes = MixerSession(channels: [.init(name: "Voice", source: .init(kind: .tone))])
        #expect(throws: Never.self) { try noMixes.validated() }
    }

    @Test func testGroupedSourceValidationIncludesAdditionalSources() throws {
        let primary = InputSource(kind: .application, bundleID: "example.first")
        var session = MixerSession.initial
        session.addChannel(.init(name: "Group", source: primary, additionalSources: [primary]))
        #expect(throws: SessionError.self) { try session.validated() }
        session.channels[0].additionalSources = [.init(kind: .device, deviceUID: "device", channelOffset: -1)]
        #expect(throws: SessionError.self) { try session.validated() }
        session.channels[0].additionalSources[0].channelOffset = 4096
        #expect(throws: SessionError.self) { try session.validated() }
        session.channels[0].additionalSources = [.init(kind: .application, bundleID: "example.second")]
        #expect(throws: Never.self) { try session.validated() }
    }

    @Test func testOutputOverlapRejectedButDistinctDevicesAndPairsAllowed() throws {
        var session = MixerSession(buses: [.init(name: "Personal", deviceUID: "first")])
        session.buses[0].additionalOutputs = [.init(deviceUID: "first", channelOffset: 1)]
        #expect(throws: SessionError.self) { try session.validated() }
        session.buses[0].additionalOutputs = [.init(deviceUID: "first", channelOffset: 0)]
        #expect(throws: SessionError.self) { try session.validated() }
        session.buses[0].additionalOutputs = [.init(deviceUID: "first", channelOffset: 2), .init(deviceUID: "second", channelOffset: 0), .init(), .init()]
        #expect(throws: Never.self) { try session.validated() }
        session.buses[0].additionalOutputs[0].channelOffset = 4096
        #expect(throws: SessionError.self) { try session.validated() }
    }

    @Test func testNewRoutesRemainMutedAfterRoundTripAndChannelRemoval() throws {
        let (directory, store) = try temporaryStore()
        defer { try? FileManager.default.removeItem(at: directory) }
        var session = MixerSession.initial
        session.addChannel(.init(name: "First", source: .init(kind: .tone)))
        session.sends[0].muted = false
        session.sends[0].gainDB = -13
        let firstID = session.channels[0].id
        session.addChannel(.init(name: "Second", source: .init(kind: .tone)))
        #expect(session.sends.filter { $0.channelID != firstID }.allSatisfy { $0.muted })
        session.removeChannel(firstID)
        try store.save(session)
        let loaded = try store.load()
        #expect(loaded == session)
        #expect(loaded.sends.count == loaded.buses.count)
        #expect(loaded.sends.allSatisfy { $0.muted })
    }
}
