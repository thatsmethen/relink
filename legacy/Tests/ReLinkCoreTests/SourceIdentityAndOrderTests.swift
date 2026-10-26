import Foundation
import Testing
@testable import RaveLinkCore

struct SourceIdentityAndOrderTests {
    @Test func canonicalCaptureIdentityDropsIrrelevantFields() {
        #expect(InputSource(kind: .device, deviceUID: "mic", bundleID: "stale", channelOffset: 2).canonicalized == .init(kind: .device, deviceUID: "mic", channelOffset: 2))
        #expect(InputSource(kind: .application, deviceUID: "stale", bundleID: "app", channelOffset: 2).canonicalized == .init(kind: .application, bundleID: "app"))
        for kind: SourceKind in [.system, .tone] {
            #expect(InputSource(kind: kind, deviceUID: "stale", bundleID: "stale", channelOffset: 2).canonicalized == .init(kind: kind))
        }
    }

    @Test func importedStaleMetadataCannotDisguiseDuplicateCapture() throws {
        for kind: SourceKind in [.device, .application, .system, .tone] {
            let source = InputSource(kind: kind, deviceUID: "mic", bundleID: "app", channelOffset: 2).canonicalized
            var disguised = source
            if kind == .device { disguised.bundleID = "unused" }
            else { disguised.deviceUID = "unused"; disguised.channelOffset = 4 }
            let channel = MixerChannel(name: "Group", source: source, additionalSources: [disguised])
            // Exercise the actual JSON import path, not just the initializer.
            let imported = try JSONDecoder().decode(MixerSession.self, from: JSONEncoder().encode(MixerSession(channels: [channel])))
            #expect(throws: SessionError.self) { try imported.validated() }
        }
    }

    @Test func singleLegacySourceRemainsLoadableAndRawBoundsAreChecked() throws {
        var channel = MixerChannel(name: "App", source: .init(kind: .application, deviceUID: "unused", bundleID: "app", channelOffset: 2))
        let normalized = try MixerSession(channels: [channel]).validated()
        #expect(normalized.channels[0].source == .init(kind: .application, bundleID: "app"))
        channel.source.channelOffset = -1
        #expect(throws: SessionError.self) { try MixerSession(channels: [channel]).validated() }
    }

    @Test func reorderingPreservesRouteIdentityAndControls() throws {
        var session = MixerSession.initial
        let a = MixerChannel(name: "A", source: .init(kind: .tone))
        let b = MixerChannel(name: "B", source: .init(kind: .system))
        session.addChannel(a); session.addChannel(b)
        session.sends[0].gainDB = -17
        session.sends[0].pan = 0.3
        session.sends[0].muted = false
        let routes = session.sends
        let bus = session.buses[2].id
        let movedChannel = session.moveChannel(b.id, by: -1)
        let movedBus = session.moveBus(bus, by: -2)
        #expect(movedChannel)
        #expect(movedBus)
        #expect(session.channels.map(\.id) == [b.id, a.id])
        #expect(session.buses.first?.id == bus)
        #expect(session.sends == routes)
        #expect(throws: Never.self) { try session.validated() }
    }

    @Test func staleOrOutOfRangeReorderDoesNotMutate() {
        var session = MixerSession.initial
        let a = MixerChannel(name: "A", source: .init(kind: .tone))
        session.addChannel(a)
        let original = session
        let stale = session.moveChannel(UUID(), by: 1)
        let beforeFirst = session.moveChannel(a.id, by: -1)
        let overflow = session.moveBus(session.buses[2].id, by: Int.max)
        let underflow = session.moveBus(session.buses[0].id, by: Int.min)
        #expect(!stale && !beforeFirst && !overflow && !underflow)
        #expect(session == original)
    }
}
