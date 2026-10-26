import Foundation
import Testing
@testable import RaveLinkCore

@Suite struct ExpandedRoutingTests {
    @Test func capacityRejectsOverflowAndNegativeCountsWithoutAllocatingMatrix() {
        #expect(throws: (any Error).self) { try RoutingCapacityEstimate(expandedChannelCount: .max, expandedBusCount: 2) }
        #expect(throws: (any Error).self) { try RoutingCapacityEstimate(expandedChannelCount: .max, expandedBusCount: 0) }
        #expect(throws: (any Error).self) { try RoutingCapacityEstimate(expandedChannelCount: -1, expandedBusCount: 1) }
    }

    @Test func capacityBudgetBoundaryIsExplicitAndConfigurable() throws {
        let session = groupedSession()
        let estimate = try session.routingCapacityEstimate()
        #expect(estimate.expandedChannelCount == 2)
        #expect(estimate.expandedBusCount == 3)
        #expect(estimate.expandedRouteCount == 6)
        #expect(estimate.constructionWorkItems == 11)
        #expect(estimate.estimatedBytes > 0)
        #expect(throws: (any Error).self) { try session.expandedRouting(memoryBudgetBytes: estimate.estimatedBytes - 1) }
        #expect(try session.expandedRouting(memoryBudgetBytes: estimate.estimatedBytes).session.sends.count == 6)
        do {
            _ = try session.expandedRouting(memoryBudgetBytes: 0)
            Issue.record("Zero budget should reject the routing preparation")
        } catch {
            #expect(error.localizedDescription.contains("2 inputs, 3 outputs, and 6 routes"))
            #expect(error.localizedDescription.contains("0-byte budget"))
        }
    }

    @Test func sameVisibleCountsCanRequireDifferentExpansionBudgets() throws {
        var compact = groupedSession()
        compact.channels[0].additionalSources = []; compact.buses[0].additionalOutputs = []
        let grouped = groupedSession()
        #expect(compact.channels.count == grouped.channels.count)
        #expect(compact.buses.count == grouped.buses.count)
        let smaller = try compact.routingCapacityEstimate(), larger = try grouped.routingCapacityEstimate()
        #expect(smaller.expandedRouteCount == 2 && larger.expandedRouteCount == 6)
        #expect(smaller.estimatedBytes < larger.estimatedBytes)
        _ = try compact.expandedRouting(memoryBudgetBytes: smaller.estimatedBytes)
        #expect(throws: (any Error).self) { try grouped.expandedRouting(memoryBudgetBytes: smaller.estimatedBytes) }
    }

    @Test func compactMillionRouteExpansionFailsBeforeMaterialization() throws {
        let sources = (0..<2_000).map { InputSource(kind: .application, bundleID: "test.app.\($0)") }
        let destinations = (0..<2_000).map { MixDestination(deviceUID: "test.output.\($0)") }
        let channel = MixerChannel(name: "Group", source: sources[0], additionalSources: Array(sources.dropFirst()))
        let bus = MixBus(name: "Monitor", deviceUID: destinations[0].deviceUID, additionalOutputs: Array(destinations.dropFirst()))
        let session = MixerSession(channels: [channel], buses: [bus], sends: [.init(channelID: channel.id, busID: bus.id)])
        let estimate = try session.routingCapacityEstimate()
        #expect(estimate.expandedRouteCount == 4_000_000)
        #expect(estimate.estimatedBytes > 512 * 1024 * 1024)
        #expect(throws: (any Error).self) { try session.expandedRouting(memoryBudgetBytes: 512 * 1024 * 1024) }
    }

    private func groupedSession() -> MixerSession {
        var effects = ChannelEffects()
        effects.gainDB = -3; effects.highPassHz = 90; effects.compressorRatio = 3
        let channel = MixerChannel(id: UUID(uuidString: "00000000-0000-4000-8000-000000000001")!, name: "Music and browser",
                                   source: .init(kind: .application, bundleID: "example.music"), muted: true, solo: true,
                                   effects: effects, additionalSources: [.init(kind: .application, bundleID: "example.browser")])
        let monitor = MixBus(id: UUID(uuidString: "00000000-0000-4000-8000-000000000002")!, name: "Personal", deviceUID: "headphones", gainDB: -6,
                             additionalOutputs: [.init(deviceUID: "speakers", channelOffset: 2)])
        let stream = MixBus(id: UUID(uuidString: "00000000-0000-4000-8000-000000000003")!, name: "Stream", deviceUID: "stream")
        return MixerSession(channels: [channel], buses: [monitor, stream], sends: [
            .init(channelID: channel.id, busID: monitor.id, gainDB: -9, pan: -0.25),
            .init(channelID: channel.id, busID: stream.id, gainDB: -18, pan: 0.5, muted: true)
        ])
    }

    @Test func groupedSourcesAndDestinationsPreserveIndependentMixes() throws {
        let original = groupedSession(), expanded = try original.expandedRouting()
        #expect(expanded.session.channels.count == 2)
        #expect(expanded.session.buses.count == 3)
        #expect(expanded.session.sends.count == 6)
        #expect(expanded.session.channels[0].id == original.channels[0].id)
        #expect(expanded.session.buses[0].id == original.buses[0].id)
        #expect(expanded.session.channels.map(\.source) == original.channels[0].allSources)
        #expect(expanded.session.buses.map(\.deviceUID) == ["headphones", "speakers", "stream"])
        #expect(expanded.session.buses.map(\.channelOffset) == [0, 2, 0])
        for channel in expanded.session.channels {
            #expect(channel.additionalSources.isEmpty)
            #expect(channel.effects == original.channels[0].effects)
            #expect(channel.muted && channel.solo)
            #expect(expanded.channelParents[channel.id] == original.channels[0].id)
        }
        for bus in expanded.session.buses {
            #expect(bus.additionalOutputs.isEmpty)
            let parent = try #require(original.buses.first { $0.id == expanded.busParents[bus.id] })
            #expect(bus.gainDB == parent.gainDB && bus.muted == parent.muted)
        }
        for send in expanded.session.sends {
            let originalSend = try #require(original.sends.first { $0.channelID == expanded.channelParents[send.channelID] && $0.busID == expanded.busParents[send.busID] })
            #expect(send.gainDB == originalSend.gainDB)
            #expect(send.pan == originalSend.pan)
            #expect(send.muted == originalSend.muted)
        }
    }

    @Test func generatedIdentitiesStayStableAcrossControlAndLabelEdits() throws {
        var session = groupedSession()
        let first = try session.expandedRouting()
        session.channels[0].name = "Renamed"; session.channels[0].muted = false
        session.channels[0].effects.gainDB = 2; session.buses[0].gainDB = -1
        session.sends[0].pan = 0.75
        let second = try session.expandedRouting()
        #expect(first.session.channels.map(\.id) == second.session.channels.map(\.id))
        #expect(first.session.buses.map(\.id) == second.session.buses.map(\.id))
        #expect(first.channelParents == second.channelParents && first.busParents == second.busParents)
        #expect(first.session != second.session)
        #expect(try second.session.expandedRouting().session == second.session)
    }

    @Test func meterMappingUsesGreatestMemberAndDoesNotInventSummedPeak() throws {
        let session = groupedSession(), expanded = try session.expandedRouting()
        let childIDs = expanded.session.channels.map(\.id), busIDs = expanded.session.buses.map(\.id)
        let channels = expanded.aggregateChannelPeaks([childIDs[0]: 0.4, childIDs[1]: 0.7, UUID(): 9])
        #expect(channels == [session.channels[0].id: 0.7])
        let buses = expanded.aggregateBusPeaks([busIDs[0]: 0.2, busIDs[1]: 0.6, busIDs[2]: 0.3])
        #expect(buses == [session.buses[0].id: 0.6, session.buses[1].id: 0.3])
        #expect(expanded.aggregateChannelPeaks([childIDs[0]: .nan, childIDs[1]: -1])[session.channels[0].id] == 0)
    }

    @Test func oldSchemaOneSessionsDecodeWithoutNewFields() throws {
        var original = groupedSession()
        original.channels[0].additionalSources = []; original.buses[0].additionalOutputs = []
        var json = try #require(JSONSerialization.jsonObject(with: JSONEncoder().encode(original)) as? [String: Any])
        var channels = try #require(json["channels"] as? [[String: Any]])
        var buses = try #require(json["buses"] as? [[String: Any]])
        for index in channels.indices { channels[index].removeValue(forKey: "additionalSources") }
        for index in buses.indices { buses[index].removeValue(forKey: "additionalOutputs") }
        json["channels"] = channels; json["buses"] = buses
        let decoded = try JSONDecoder().decode(MixerSession.self, from: JSONSerialization.data(withJSONObject: json))
        #expect(decoded == original)
        #expect(try decoded.expandedRouting().session == original)
    }

    @Test func groupsRoundTripThroughPersistence() throws {
        let original = groupedSession()
        let decoded = try JSONDecoder().decode(MixerSession.self, from: JSONEncoder().encode(original))
        #expect(decoded == original)
        #expect(try decoded.expandedRouting() == original.expandedRouting())
    }

    @Test func generatedIDCollisionFailsClosed() throws {
        var original = groupedSession()
        let collision = try original.expandedRouting().session.channels[1].id
        original.addChannel(.init(id: collision, name: "Collision", source: .init(kind: .tone)))
        #expect(throws: (any Error).self) { try original.expandedRouting() }
    }

    @Test func additionalOutputWorksWhenPrimaryIsUnassigned() throws {
        var original = groupedSession()
        original.buses[0].deviceUID = nil
        let expanded = try original.expandedRouting()
        #expect(expanded.session.buses[0].deviceUID == nil)
        #expect(expanded.session.buses[1].deviceUID == "speakers")
        #expect(expanded.busParents[expanded.session.buses[1].id] == original.buses[0].id)
    }
}
