import Foundation
import RaveLinkCore

/// Lookup storage changes only with matrix structure, never with meter updates.
/// The session remains the single owner of routing and parameter values.
struct StudioIndex {
    struct Route: Hashable { let channelID: UUID; let busID: UUID }
    var channels: [UUID: Int] = [:]
    var buses: [UUID: Int] = [:]
    var sends: [String: Int] = [:]
    var routes: [Route: Int] = [:]

    init(_ session: MixerSession? = nil) {
        guard let session else { return }
        for (offset, channel) in session.channels.enumerated() { channels[channel.id] = offset }
        for (offset, bus) in session.buses.enumerated() { buses[bus.id] = offset }
        for (offset, send) in session.sends.enumerated() {
            sends[send.id] = offset
            routes[Route(channelID: send.channelID, busID: send.busID)] = offset
        }
    }

    static func sameStructure(_ lhs: MixerSession, _ rhs: MixerSession) -> Bool {
        guard lhs.channels.count == rhs.channels.count, lhs.buses.count == rhs.buses.count,
              lhs.sends.count == rhs.sends.count else { return false }
        return zip(lhs.channels, rhs.channels).allSatisfy { $0.id == $1.id }
            && zip(lhs.buses, rhs.buses).allSatisfy { $0.id == $1.id }
            && zip(lhs.sends, rhs.sends).allSatisfy { $0.channelID == $1.channelID && $0.busID == $1.busID }
    }
}
