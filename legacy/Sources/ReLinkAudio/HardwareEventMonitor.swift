import CoreAudio
import Foundation

/// Register/remove only on the serialized HAL control queue. Listener blocks
/// carry no graph pointers and dispatch lightweight invalidations to the actor.
final class HardwareEventMonitor {
    enum Event: Sendable { case processMembership, deviceConfiguration, audioServiceRestart }
    struct Property { let object: AudioObjectID; var address: AudioObjectPropertyAddress; let block: AudioObjectPropertyListenerBlock }
    private var properties: [Property] = []
    func add(object: AudioObjectID, selector: AudioObjectPropertySelector,
             scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal,
             event: Event, callback: @escaping @Sendable (Event) -> Void) throws {
        var address = AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
        guard AudioObjectHasProperty(object, &address) else { return }
        let block: AudioObjectPropertyListenerBlock = { _, _ in callback(event) }
        let status = AudioObjectAddPropertyListenerBlock(object, &address, .main, block)
        guard status == noErr else { throw AudioRoutingError.coreAudio("Observe audio device changes", status) }
        properties.append(.init(object: object, address: address, block: block))
    }
    func removeAll() {
        for var property in properties {
            AudioObjectRemovePropertyListenerBlock(property.object, &property.address, .main, property.block)
        }
        properties.removeAll()
    }
    deinit { removeAll() }
}
