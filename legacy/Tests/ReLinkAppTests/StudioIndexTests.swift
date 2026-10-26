import Foundation
import Testing
import RaveLinkCore
import RaveLinkAudio
@testable import RaveLinkApp

@MainActor struct StudioIndexTests {
    @Test func indexedEditsFollowIdentityAcrossReorderRemovalAndReplacement() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let audio = EngineClient(autoDiscover: false)
        defer { audio.shutdown() }
        let studio = StudioModel(store: SessionStore(url: directory.appendingPathComponent("studio.json")), audio: audio)
        let first = MixerChannel(name: "First", source: .init(kind: .tone))
        let second = MixerChannel(name: "Second", source: .init(kind: .tone))
        studio.session.addChannel(first); studio.session.addChannel(second)
        let bus = try #require(studio.session.buses.first)
        let route = try #require(studio.route(channelID: second.id, busID: bus.id))
        studio.moveChannel(second.id, by: -1)
        studio.moveBus(bus.id, by: 1)
        studio.setChannel(second.id, \.name, to: "Renamed")
        studio.setBus(bus.id, \.gainDB, to: -7)
        studio.setSend(route.id, \.gainDB, to: -11)
        #expect(studio.channel(second.id)?.name == "Renamed")
        #expect(studio.channel(first.id)?.name == "First")
        #expect(studio.bus(bus.id)?.gainDB == -7)
        #expect(studio.route(channelID: second.id, busID: bus.id)?.gainDB == -11)
        studio.removeChannel(second.id)
        studio.setChannel(second.id, \.muted, to: true)
        studio.setSend(route.id, \.muted, to: false)
        #expect(studio.channel(second.id) == nil && studio.send(route.id) == nil)
        #expect(studio.channel(first.id)?.muted == false)
        studio.session = .initial
        #expect(studio.channel(first.id) == nil && studio.bus(bus.id) == nil)
    }

    @Test func routeValuesDoNotInvalidateStructureButRebindingDoes() throws {
        var session = MixerSession.initial
        session.addChannel(.init(name: "A", source: .init(kind: .tone)))
        var edited = session
        edited.channels[0].effects.gainDB = -3
        edited.buses[0].name = "Renamed"
        edited.sends[0].muted.toggle()
        #expect(StudioIndex.sameStructure(session, edited))
        edited.sends[0].busID = session.buses[1].id
        #expect(!StudioIndex.sameStructure(session, edited))
    }
}
