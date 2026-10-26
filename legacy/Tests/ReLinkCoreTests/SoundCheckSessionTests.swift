import Foundation
import Testing
@testable import RaveLinkCore

struct SoundCheckSessionTests {
    @Test func auditionPreservesSourcesAndEffectsWithOnlyExplicitDestination() throws {
        var channel = MixerChannel(name: "Voice", source: .init(kind: .device, deviceUID: "mic"), muted: true, solo: true,
            additionalSources: [.init(kind: .application, bundleID: "app")])
        channel.effects.gainDB = -7
        let session = try SoundCheckSession.make(channel: channel, output: .init(deviceUID: "headphones", channelOffset: 2))
        #expect(session.channels.count == 1 && session.buses.count == 1 && session.sends.count == 1)
        #expect(session.channels[0].id == channel.id)
        #expect(session.channels[0].allSources == channel.allSources)
        #expect(session.channels[0].effects == channel.effects)
        #expect(!session.channels[0].muted && !session.channels[0].solo)
        #expect(session.buses[0].allOutputs == [.init(deviceUID: "headphones", channelOffset: 2)])
        #expect(!session.sends[0].muted && session.sends[0].gainDB == 0)
        #expect(channel.muted && channel.solo)
    }
    @Test func listeningOutputMustBeExplicitAndValid() {
        let channel = MixerChannel(name: "Tone", source: .init(kind: .tone))
        #expect(throws: SessionError.self) { try SoundCheckSession.make(channel: channel, output: .init()) }
        #expect(throws: SessionError.self) { try SoundCheckSession.make(channel: channel, output: .init(deviceUID: "")) }
        #expect(throws: SessionError.self) { try SoundCheckSession.make(channel: channel, output: .init(deviceUID: "output", channelOffset: -1)) }
    }
}
