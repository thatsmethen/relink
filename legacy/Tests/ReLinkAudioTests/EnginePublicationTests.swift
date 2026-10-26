import Combine
import Foundation
import Testing
@testable import RaveLinkAudio

@Suite @MainActor struct EnginePublicationTests {
    private func inventory() -> EngineSnapshot {
        var snapshot = EngineSnapshot()
        snapshot.devices = [.init(id: "virtual", name: "Virtual input", inputChannels: 8,
                                  outputChannels: 8, sampleRate: 48000, isVirtual: true)]
        snapshot.applications = [.init(id: "test.application", name: "Playing app")]
        return snapshot
    }

    @Test func identicalIdleHeartbeatsDoNotInvalidateSubscribers() {
        let client = EngineClient(helperURL: nil, autoDiscover: false)
        let snapshot = inventory()
        client.receive(.init(generation: 0, kind: .state, snapshot: snapshot))
        var invalidations = 0
        var inventoryPublications = 0
        let wholeObject = client.objectWillChange.sink { invalidations += 1 }
        let devices = client.$devices.dropFirst().sink { _ in inventoryPublications += 1 }
        let applications = client.$applications.dropFirst().sink { _ in inventoryPublications += 1 }
        for _ in 0..<200 { client.receive(.init(generation: 0, kind: .state, snapshot: snapshot)) }
        #expect(invalidations == 0, "Unchanged snapshots must not continuously invalidate the studio")
        #expect(inventoryPublications == 0)
        #expect(client.devices == snapshot.devices && client.applications == snapshot.applications)
        withExtendedLifetime([wholeObject, devices, applications]) {}
    }

    @Test func changedLiveMetersAndDiagnosticsStillReachSubscribers() {
        let client = EngineClient(helperURL: nil, autoDiscover: false)
        var snapshot = inventory(); snapshot.running = true
        let channels = (0..<128).map { _ in UUID() }, buses = (0..<32).map { _ in UUID() }
        snapshot.channelMeters = Dictionary(uniqueKeysWithValues: channels.map { ($0, Float(0)) })
        snapshot.busMeters = Dictionary(uniqueKeysWithValues: buses.map { ($0, Float(0)) })
        client.receive(.init(generation: 0, kind: .state, snapshot: snapshot))
        var invalidations = 0, inventoryPublications = 0, statePublications = 0
        var channelPublications = 0, busPublications = 0, diagnosticPublications = 0
        let subscriptions: [AnyCancellable] = [
            client.objectWillChange.sink { invalidations += 1 },
            client.$devices.dropFirst().sink { _ in inventoryPublications += 1 },
            client.$applications.dropFirst().sink { _ in inventoryPublications += 1 },
            client.$running.dropFirst().sink { _ in statePublications += 1 },
            client.$starting.dropFirst().sink { _ in statePublications += 1 },
            client.$channelMeters.dropFirst().sink { _ in channelPublications += 1 },
            client.$busMeters.dropFirst().sink { _ in busPublications += 1 },
            client.$diagnostics.dropFirst().sink { _ in diagnosticPublications += 1 }
        ]
        for step in 1...40 {
            snapshot.channelMeters[channels[step % channels.count]] = Float(step) / 40
            snapshot.busMeters[buses[step % buses.count]] = Float(step) / 80
            snapshot.diagnostics.renderedFrames = UInt64(step * 2400)
            let response = EngineResponse(generation: 0, kind: .state, snapshot: snapshot)
            client.receive(response); client.receive(response)
        }
        #expect(channelPublications == 40 && busPublications == 40 && diagnosticPublications == 40)
        #expect(invalidations == 120 && inventoryPublications == 0 && statePublications == 0)
        #expect(client.channelMeters == snapshot.channelMeters && client.busMeters == snapshot.busMeters)
        #expect(client.diagnostics.renderedFrames == 96000 && client.running)
        snapshot.devices = []
        client.receive(.init(generation: 0, kind: .state, snapshot: snapshot))
        #expect(inventoryPublications == 1 && client.devices.isEmpty, "Hotplug inventory changes remain observable")
        withExtendedLifetime(subscriptions) {}
    }

    @Test func staleGenerationCannotPublishChangedValuesAfterStop() {
        let client = EngineClient(helperURL: nil, autoDiscover: false)
        client.stop() // Invalidates generation zero even without a live helper.
        var invalidations = 0
        let subscription = client.objectWillChange.sink { invalidations += 1 }
        var snapshot = inventory(); snapshot.running = true
        snapshot.channelMeters[UUID()] = 0.8; snapshot.diagnostics.renderedFrames = 2400
        client.receive(.init(generation: 0, kind: .state, snapshot: snapshot))
        #expect(invalidations == 0)
        #expect(!client.running && client.devices.isEmpty && client.channelMeters.isEmpty)
        withExtendedLifetime(subscription) {}
    }
}
