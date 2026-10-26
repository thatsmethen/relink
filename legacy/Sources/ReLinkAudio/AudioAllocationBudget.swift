import Foundation
import RaveLinkCore

/// A conservative preflight estimate, not a measurement of process RSS. Separate
/// from model expansion: this accounts for realtime buffers at actual device rates.
struct AudioAllocationBudget {
    let limit: UInt64
    private(set) var reserved: UInt64 = 0
    init(limit: UInt64 = RoutingCapacity.defaultMemoryBudgetBytes) { self.limit = limit }
    mutating func reserve(_ bytes: UInt64) throws {
        let (total, overflow) = reserved.addingReportingOverflow(bytes)
        guard !overflow, total <= limit else {
            throw AudioRoutingError.configuration("This routing graph exceeds the audio buffer budget (\(limit / 1024 / 1024) MiB on this Mac). Reduce sources, output destinations, or sample-rate differences before starting audio.")
        }
        reserved = total
    }
    static func product(_ values: UInt64...) throws -> UInt64 {
        try values.reduce(1) { result, value in
            let (next, overflow) = result.multipliedReportingOverflow(by: value)
            guard !overflow else { throw AudioRoutingError.configuration("The requested audio buffers exceed the machine's addressable memory.") }
            return next
        }
    }
    static func output(sources: Int, buses: Int, frames: UInt32) throws -> UInt64 {
        let sourceCount = UInt64(sources), busCount = UInt64(buses)
        var budget = AudioAllocationBudget(limit: UInt64.max)
        try budget.reserve(try product(sourceCount + busCount + 1, UInt64(frames), 8))
        try budget.reserve(try product(sourceCount, busCount, 64)) // sends + alignment allowance
        try budget.reserve(try product(sourceCount + busCount + 1, 512)) // descriptors + DSP
        return budget.reserved
    }
    static func ring(ratio: Double, frames: UInt32) throws -> (capacity: UInt32, bytes: UInt64) {
        guard ratio.isFinite, ratio >= 1 / 24.0, ratio <= 24 else {
            throw AudioRoutingError.configuration("The input and output sample rates are too far apart. Use rates from 8 to 384 kHz that differ by no more than 24 times in Audio MIDI Setup.")
        }
        // Retain a rounded-up priming reserve and room for another whole producer
        // burst while a faster input clock is being corrected.
        let requested = UInt64(max(65536, Double(frames) * max(1, ratio) * 4 + 2048))
        var capacity: UInt64 = 4
        while capacity < requested { capacity <<= 1 }
        guard capacity <= UInt32.max else { throw AudioRoutingError.configuration("The requested audio buffer is too large.") }
        let taps = UInt64(64 * ceil(max(1, ratio)))
        let bytes = try product(capacity, 8) + product(taps, 512, 4) + 132_096 // temporary default kernel and descriptors
        return (UInt32(capacity), bytes)
    }
}
