import Foundation
import Testing
@testable import RaveLinkCore

@Suite struct AudioUnitSessionTests {
    private var descriptor: AudioUnitDescriptor { .init(component: .init(type: 0x61756678, subtype: 0x6c6d7472, manufacturer: 0x6170706c), name: "Apple: AUPeakLimiter", version: 67072) }
    @Test func oldSessionDefaultsToEmptyRackAndRackRoundTrips() throws {
        var session = MixerSession.initial
        session.addChannel(.init(name: "Mic"))
        var object = try #require(JSONSerialization.jsonObject(with: JSONEncoder().encode(session)) as? [String: Any])
        var channels = try #require(object["channels"] as? [[String: Any]])
        channels[0].removeValue(forKey: "audioUnitEffects"); object["channels"] = channels
        let legacy = try JSONDecoder().decode(MixerSession.self, from: JSONSerialization.data(withJSONObject: object)).validated()
        #expect(legacy.channels[0].audioUnitEffects.isEmpty)
        let plist = try PropertyListSerialization.data(fromPropertyList: ["test": 1], format: .binary, options: 0)
        session.channels[0].audioUnitEffects = [.init(descriptor: descriptor, state: .init(component: descriptor.component, componentVersion: descriptor.version, propertyList: plist), parameters: [.init(address: 2, value: 6)], isBypassed: true)]
        let copy = try JSONDecoder().decode(MixerSession.self, from: JSONEncoder().encode(session)).validated()
        #expect(copy == session)
        session.channels[0].additionalSources = [.init(kind: .application, bundleID: "test.app")]
        #expect(try session.expandedRouting().session.channels.count == 2)
    }
    @Test func rejectsDuplicatedSlotsInvalidIdentityStateAndParameters() throws {
        var session = MixerSession.initial
        let slot = AudioUnitEffectSlot(descriptor: descriptor)
        session.addChannel(.init(name: "Mic", audioUnitEffects: [slot, slot]))
        #expect(throws: (any Error).self) { try session.validated() }
        session.channels[0].audioUnitEffects = [slot]
        session.channels[0].audioUnitEffects[0].state = .init(component: descriptor.component, componentVersion: 1, propertyList: Data([1]))
        #expect(throws: (any Error).self) { try session.validated() }
        session.channels[0].audioUnitEffects[0].state = nil
        session.channels[0].audioUnitEffects[0].parameters = [.init(address: 2, value: 1), .init(address: 2, value: 2)]
        #expect(throws: (any Error).self) { try session.validated() }
        session.channels[0].audioUnitEffects[0].parameters = [.init(address: 2, value: .nan)]
        #expect(throws: (any Error).self) { try session.validated() }
        session.channels[0].audioUnitEffects[0].parameters = []
        session.channels[0].audioUnitEffects[0].descriptor = .init(component: .init(type: 1, subtype: 1, manufacturer: 1), name: "Wrong unit type", version: nil)
        #expect(throws: (any Error).self) { try session.validated() }
    }
}
