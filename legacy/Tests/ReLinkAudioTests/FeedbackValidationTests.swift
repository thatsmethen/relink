import Testing
import RaveLinkCore
@testable import RaveLinkAudio

@Suite struct FeedbackValidationTests {
    @Test func detectsMultiDeviceCycleAndAllowsMutedBreak() {
        let a = MixerChannel(name: "A", source: .init(kind: .device, deviceUID: "virtual-A"))
        let b = MixerChannel(name: "B", source: .init(kind: .device, deviceUID: "virtual-B"))
        let toB = MixBus(name: "to B", deviceUID: "virtual-B")
        let toA = MixBus(name: "to A", deviceUID: "virtual-A")
        var session = MixerSession(channels: [a, b], buses: [toB, toA], sends: [.init(channelID: a.id, busID: toB.id), .init(channelID: b.id, busID: toA.id)])
        #expect(hasVirtualFeedbackCycle(session, virtualDeviceUIDs: ["virtual-A", "virtual-B"]))
        session.sends[1].muted = true
        #expect(!hasVirtualFeedbackCycle(session, virtualDeviceUIDs: ["virtual-A", "virtual-B"]))
    }
    @Test func separatesStereoPairsAndRejectsOverlap() {
        let source = MixerChannel(name: "Pair 1", source: .init(kind: .device, deviceUID: "virtual", channelOffset: 0))
        var bus = MixBus(name: "Pair 2", deviceUID: "virtual", channelOffset: 2)
        var session = MixerSession(channels: [source], buses: [bus], sends: [.init(channelID: source.id, busID: bus.id)])
        #expect(!hasVirtualFeedbackCycle(session, virtualDeviceUIDs: ["virtual"]))
        bus.channelOffset = 1; session.buses = [bus]
        #expect(hasVirtualFeedbackCycle(session, virtualDeviceUIDs: ["virtual"]))
        #expect(!hasVirtualFeedbackCycle(session, virtualDeviceUIDs: []), "Physical duplex interfaces are not assumed to loop back")
    }
}
