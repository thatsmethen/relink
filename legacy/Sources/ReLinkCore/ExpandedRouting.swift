import Foundation
import CryptoKit

/// Resource policy for preparation work, separate from audio performance limits.
public enum RoutingCapacity {
    /// At most one sixteenth of physical RAM and 512 MiB per preparation budget.
    /// This is a conservative allowance, not a reading of currently free RAM.
    public static var defaultMemoryBudgetBytes: UInt64 {
        min(ProcessInfo.processInfo.physicalMemory / 16, 512 * 1024 * 1024)
    }
}

/// Predicts the size of lowering before constructing its channel × output matrix.
/// Includes spare array capacity and allowances for maps, validation sets, and
/// generated route identifier strings. It excludes retained original-model data,
/// audio buffers, Core Audio allocations, and allocator/runtime bookkeeping.
/// This is a resource guard, not a guarantee about process RSS or realtime speed.
public struct RoutingCapacityEstimate: Equatable, Sendable {
    public let expandedChannelCount: Int
    public let expandedBusCount: Int
    public let expandedRouteCount: Int
    public let estimatedBytes: UInt64
    /// Node and route records visited during construction; not CPU-time units.
    public let constructionWorkItems: UInt64

    public init(expandedChannelCount: Int, expandedBusCount: Int) throws {
        guard expandedChannelCount >= 0, expandedBusCount >= 0 else {
            throw SessionError.invalid("Expanded routing counts cannot be negative.")
        }
        let (routes, overflow) = expandedChannelCount.multipliedReportingOverflow(by: expandedBusCount)
        guard !overflow else { throw routingCapacityOverflow() }
        self.expandedChannelCount = expandedChannelCount
        self.expandedBusCount = expandedBusCount
        expandedRouteCount = routes
        let channels = UInt64(expandedChannelCount), buses = UInt64(expandedBusCount), sends = UInt64(routes)
        // Allow two inline copies per record plus per-record container/string
        // space. The named allowances are policy estimates, not ABI guarantees.
        let channelAllowance = UInt64(MemoryLayout<MixerChannel>.stride * 2 + 256)
        let busAllowance = UInt64(MemoryLayout<MixBus>.stride * 2 + 256)
        let routeAllowance = UInt64(MemoryLayout<MixSend>.stride * 2 + 256)
        var bytes: UInt64 = 16 * 1024 // Fixed dictionary/array and validation allowance.
        for (count, allowance) in [(channels, channelAllowance), (buses, busAllowance), (sends, routeAllowance)] {
            let (part, multiplyOverflow) = count.multipliedReportingOverflow(by: allowance)
            let (total, addOverflow) = bytes.addingReportingOverflow(part)
            guard !multiplyOverflow, !addOverflow else { throw routingCapacityOverflow() }
            bytes = total
        }
        estimatedBytes = bytes
        let (nodes, nodeOverflow) = channels.addingReportingOverflow(buses)
        let (work, workOverflow) = nodes.addingReportingOverflow(sends)
        guard !nodeOverflow, !workOverflow else { throw routingCapacityOverflow() }
        constructionWorkItems = work
    }

    public func requireMemoryBudget(_ memoryBudgetBytes: UInt64) throws {
        guard estimatedBytes <= memoryBudgetBytes else {
            throw SessionError.invalid("This routing expands to \(expandedChannelCount) inputs, \(expandedBusCount) outputs, and \(expandedRouteCount) routes; preparation is estimated at \(estimatedBytes) bytes, above the \(memoryBudgetBytes)-byte budget. Reduce grouped sources, additional outputs, or mix buses before starting audio.")
        }
    }
}

private func routingCapacityOverflow() -> SessionError {
    .invalid("The expanded routing resource estimate exceeds this machine's addressable size. Reduce grouped sources, additional outputs, or mix buses.")
}

/// A device-level graph derived from the user's grouped channel/mix model.
/// IDs are stable across control and label edits. Primary nodes retain their IDs;
/// additional nodes use UUID v5 identities derived from the parent and array slot.
public struct ExpandedRouting: Sendable, Equatable {
    public let session: MixerSession
    /// Expanded channel ID → original visible channel ID, including primary nodes.
    public let channelParents: [UUID: UUID]
    /// Expanded bus ID → original visible bus ID, including primary nodes.
    public let busParents: [UUID: UUID]

    /// Greatest reported child peak. The grouped renderer reports its summed,
    /// post-DSP waveform peak on the leader; other children report zero. Callers
    /// supplying separate member peaks obtain activity, not a reconstructed sum.
    public func aggregateChannelPeaks(_ peaks: [UUID: Float]) -> [UUID: Float] {
        aggregate(peaks, parents: channelParents)
    }
    /// Greatest output peak for each visible mix, including rate-converted copies.
    public func aggregateBusPeaks(_ peaks: [UUID: Float]) -> [UUID: Float] {
        aggregate(peaks, parents: busParents)
    }
    private func aggregate(_ peaks: [UUID: Float], parents: [UUID: UUID]) -> [UUID: Float] {
        var result = Dictionary(Set(parents.values).map { ($0, Float.zero) }, uniquingKeysWith: max)
        for (child, peak) in peaks where peak.isFinite && peak >= 0 {
            guard let parent = parents[child] else { continue }
            result[parent] = max(result[parent] ?? 0, peak)
        }
        return result
    }
}

public extension MixerSession {
    /// Counts members directly, without materializing allSources/allOutputs arrays.
    func routingCapacityEstimate() throws -> RoutingCapacityEstimate {
        var channelCount = channels.count, busCount = buses.count
        for channel in channels {
            let (total, overflow) = channelCount.addingReportingOverflow(channel.additionalSources.count)
            guard !overflow else { throw routingCapacityOverflow() }
            channelCount = total
        }
        for bus in buses {
            let (total, overflow) = busCount.addingReportingOverflow(bus.additionalOutputs.count)
            guard !overflow else { throw routingCapacityOverflow() }
            busCount = total
        }
        return try RoutingCapacityEstimate(expandedChannelCount: channelCount, expandedBusCount: busCount)
    }

    /// Expands every grouped source into a capture channel and every additional
    /// output into a bus. Each parent send is copied to the cross-product of its
    /// source members and destinations, preserving gain, pan, and mute settings.
    /// Effect settings are copied to each member as control metadata. The audio
    /// backend uses channelParents to sum members before applying the parent
    /// effect chain and routes that group once through the parent sends.
    func expandedRouting(memoryBudgetBytes: UInt64 = RoutingCapacity.defaultMemoryBudgetBytes) throws -> ExpandedRouting {
        let capacity = try routingCapacityEstimate()
        try capacity.requireMemoryBudget(memoryBudgetBytes)
        _ = try validated()
        var channelGroups: [UUID: [UUID]] = [:], busGroups: [UUID: [UUID]] = [:]
        var channelParents: [UUID: UUID] = [:], busParents: [UUID: UUID] = [:]
        var lowered = MixerSession(name: name)
        lowered.schemaVersion = schemaVersion
        lowered.channels.reserveCapacity(capacity.expandedChannelCount)
        lowered.buses.reserveCapacity(capacity.expandedBusCount)
        lowered.sends.reserveCapacity(capacity.expandedRouteCount)
        channelParents.reserveCapacity(capacity.expandedChannelCount)
        busParents.reserveCapacity(capacity.expandedBusCount)
        for parent in channels {
            var ids: [UUID] = []
            for (slot, source) in parent.allSources.enumerated() {
                var child = parent
                child.id = slot == 0 ? parent.id : routingChildID(parent.id, role: "channel-source", slot: slot)
                guard channelParents[child.id] == nil else {
                    throw SessionError.invalid("Expanded input identifiers collide. Recreate the affected input group.")
                }
                child.source = source
                child.additionalSources = []
                lowered.channels.append(child)
                channelParents[child.id] = parent.id
                ids.append(child.id)
            }
            channelGroups[parent.id] = ids
        }
        for parent in buses {
            var ids: [UUID] = []
            for (slot, destination) in parent.allOutputs.enumerated() {
                var child = parent
                child.id = slot == 0 ? parent.id : routingChildID(parent.id, role: "mix-output", slot: slot)
                guard busParents[child.id] == nil else {
                    throw SessionError.invalid("Expanded mix identifiers collide. Recreate the affected mix.")
                }
                child.deviceUID = destination.deviceUID
                child.channelOffset = destination.channelOffset
                child.additionalOutputs = []
                lowered.buses.append(child)
                busParents[child.id] = parent.id
                ids.append(child.id)
            }
            busGroups[parent.id] = ids
        }
        for send in sends {
            guard let children = channelGroups[send.channelID], let outputs = busGroups[send.busID] else {
                throw SessionError.invalid("A grouped route refers to a missing input or mix.")
            }
            for channel in children {
                for bus in outputs {
                    var copy = send
                    copy.channelID = channel; copy.busID = bus
                    lowered.sends.append(copy)
                }
            }
        }
        return ExpandedRouting(session: try lowered.validated(), channelParents: channelParents, busParents: busParents)
    }
}

private func routingChildID(_ parent: UUID, role: String, slot: Int) -> UUID {
    var namespace = parent.uuid
    var data = withUnsafeBytes(of: &namespace) { Data($0) }
    data.append(contentsOf: "ravelink.\(role).\(slot)".utf8)
    var bytes = Array(Insecure.SHA1.hash(data: data).prefix(16))
    bytes[6] = (bytes[6] & 0x0f) | 0x50
    bytes[8] = (bytes[8] & 0x3f) | 0x80
    return UUID(uuid: (bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                       bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]))
}
