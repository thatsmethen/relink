import Foundation
import Testing
import RaveLinkCore
import RaveLinkAudio
@testable import RaveLinkApp

@MainActor struct SoundCheckIsolationTests {
    @Test func globalAudioCommandCannotStartStudioInsideSoundCheck() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let audio = EngineClient(autoDiscover: false)
        defer { audio.shutdown() }
        let studio = StudioModel(store: SessionStore(url: directory.appendingPathComponent("studio.json")), audio: audio)
        let channel = MixerChannel(name: "Test", source: .init(kind: .tone))
        studio.session.addChannel(channel)
        studio.openSoundCheck(channel.id)
        studio.toggleAudio()
        #expect(studio.showingSoundCheck)
        #expect(!audio.running && !audio.starting)
        #expect(audio.helperProcessID == nil)
        #expect(studio.soundCheckNotice?.contains("Close this window") == true)
        studio.session.channels[0].effects.gainDB = -12
        studio.toggleAudio()
        #expect(!audio.running && !audio.starting)
        #expect(audio.helperProcessID == nil)
        #expect(studio.session.channels[0].effects.gainDB == -12)
    }

    @Test func openingAndClosingAuditionKeepsStudioRoutesAndMutes() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let audio = EngineClient(autoDiscover: false)
        defer { audio.shutdown() }
        let studio = StudioModel(store: SessionStore(url: directory.appendingPathComponent("studio.json")), audio: audio)
        let channel = MixerChannel(name: "Voice", source: .init(kind: .tone), muted: true)
        studio.session.addChannel(channel)
        studio.session.sends[1].muted = false
        studio.session.sends[1].gainDB = -9
        let original = studio.session
        studio.openSoundCheck(channel.id)
        studio.endSoundCheck()
        #expect(studio.session == original)
        #expect(!audio.running && !audio.starting)
    }
}
