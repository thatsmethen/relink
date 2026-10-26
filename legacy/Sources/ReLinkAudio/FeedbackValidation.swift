import Foundation
import RaveLinkCore

/// A virtual driver returns each output channel to its corresponding input.
/// Detect cycles through overlapping stereo pairs; different pairs on the same
/// multichannel driver remain usable for independent routes.
func hasVirtualFeedbackCycle(_ session: MixerSession, virtualDeviceUIDs: Set<String>) -> Bool {
    let anySolo = session.channels.contains(where: \.solo)
    let sources = session.channels.filter {
        $0.source.kind == .device && !$0.muted && (!anySolo || $0.solo)
            && virtualDeviceUIDs.contains($0.source.deviceUID ?? "")
    }
    let inputsByUID = Dictionary(grouping: sources, by: { $0.source.deviceUID! })
    let sends = Dictionary(grouping: session.sends.filter { !$0.muted }, by: \.channelID)
    let buses = Dictionary(uniqueKeysWithValues: session.buses.filter { !$0.muted }.map { ($0.id, $0) })
    var edges: [UUID: Set<UUID>] = [:]
    for source in sources {
        for send in sends[source.id] ?? [] {
            guard let bus = buses[send.busID], let uid = bus.deviceUID else { continue }
            for receiver in inputsByUID[uid] ?? [] {
                // Avoid unchecked offset+1 arithmetic even before model validation.
                if abs(Double(receiver.source.channelOffset) - Double(bus.channelOffset)) < 2 {
                    edges[source.id, default: []].insert(receiver.id)
                }
            }
        }
    }
    var visiting: Set<UUID> = [], complete: Set<UUID> = []
    func visit(_ node: UUID) -> Bool {
        if visiting.contains(node) { return true }
        if complete.contains(node) { return false }
        visiting.insert(node)
        for next in edges[node] ?? [] { if visit(next) { return true } }
        visiting.remove(node); complete.insert(node); return false
    }
    return edges.keys.contains(where: visit)
}
