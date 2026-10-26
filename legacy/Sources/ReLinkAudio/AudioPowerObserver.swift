import AppKit

// The notification adapter is separate from the policy so tests can exercise
// suspend/wake containment without sleeping the development machine.
enum AudioPowerEvent: Sendable { case sleep, wake }
final class AudioPowerObserver {
    private let center = NSWorkspace.shared.notificationCenter
    private var tokens: [NSObjectProtocol] = []
    init(callback: @escaping @Sendable (AudioPowerEvent) -> Void) {
        tokens.append(center.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { _ in callback(.sleep) })
        tokens.append(center.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { _ in callback(.wake) })
    }
    deinit { tokens.forEach(center.removeObserver) }
}
