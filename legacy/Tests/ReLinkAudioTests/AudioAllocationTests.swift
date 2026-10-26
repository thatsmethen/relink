import Testing
@testable import RaveLinkAudio

@Suite struct AudioAllocationTests {
    @Test func rejectsBudgetOverflowBeforeAllocating() throws {
        var budget = AudioAllocationBudget(limit: 1024)
        try budget.reserve(1000)
        #expect(throws: (any Error).self) { try budget.reserve(25) }
        #expect(budget.reserved == 1000)
        try budget.reserve(24)
        #expect(budget.reserved == 1024)
        var huge = AudioAllocationBudget(limit: UInt64.max)
        try huge.reserve(UInt64.max)
        #expect(throws: (any Error).self) { try huge.reserve(1) }
        #expect(throws: (any Error).self) { try AudioAllocationBudget.product(UInt64.max, 2) }
    }
    @Test func rateExtremesReserveEnoughFramesAndFilterSpace() throws {
        let normal = try AudioAllocationBudget.ring(ratio: 1, frames: 16384)
        let extreme = try AudioAllocationBudget.ring(ratio: 24, frames: 16384)
        #expect(normal.capacity == 131072)
        #expect(extreme.capacity >= 4 * 16384 * 24 + 1536)
        #expect(extreme.bytes > UInt64(extreme.capacity) * 8 + 1536 * 512 * 4)
        #expect(throws: (any Error).self) { try AudioAllocationBudget.ring(ratio: .nan, frames: 16384) }
        #expect(throws: (any Error).self) { try AudioAllocationBudget.ring(ratio: 25, frames: 16384) }
    }
}
