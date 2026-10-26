import Foundation
import AudioToolbox
import AVFAudio
import RaveLinkCore
import RaveLinkRealtime
import RaveLinkAudioUnitBridge

/// Owned by one output device graph. Configure/update/deinit on its control queue;
/// the audio callback reaches only the C bridge and cached Audio Unit blocks.
public final class LiveAudioUnitGraph {
    private struct ParameterInfo {
        let minimum: Float, maximum: Float, baseline: Float
        let writable: Bool, canRamp: Bool
    }
    private struct UnitRecord {
        let source: UInt32, slot: UInt32, channelID: UUID, slotID: UUID
        let channelName: String, name: String
        let wrapper: AVAudioUnit
        let latencyFrames: UInt32
        let renderBlock: AURenderBlock
        let scheduleBlock: AUScheduleParameterBlock
        let parameters: [UInt64: ParameterInfo]
        var applied: [UInt64: Float]
        var bypassed: Bool
    }
    private let bridge: OpaquePointer
    private let topology: MixerSession
    private var units: [UnitRecord] = []
    private var latencyFailure: String?
    public let sampleRate: Double
    /// Reported AU processing delay aligned across leader channels on this clock.
    /// Excludes capture FIFO, device, transport, and converter latency.
    public var alignmentLatencySeconds: Double { Double(rl_au_graph_latency_frames(bridge)) / sampleRate }
    /// Host delay rings only; vendor-owned allocations are not measurable here.
    public var delayMemoryBytes: UInt64 { rl_au_graph_delay_bytes(bridge) }

    /// Conservative bridge storage beyond delay rings: chain descriptors, planar
    /// scratch, nodes, pointer-array growth and host pull blocks. This is not RSS;
    /// vendor resources and Swift parameter/state objects are outside this estimate.
    public static func estimatedHostOverhead(session: MixerSession, channelParents: [UUID: UUID]) throws -> UInt64 {
        var budget = AudioAllocationBudget(limit: UInt64.max)
        try budget.reserve(256)
        try budget.reserve(try AudioAllocationBudget.product(UInt64(max(1, session.channels.count)), 128))
        for channel in session.channels {
            guard channelParents[channel.id] == nil || channelParents[channel.id] == channel.id,
                  !channel.audioUnitEffects.isEmpty else { continue }
            try budget.reserve(8192) // Four 512-frame Float32 planes.
            try budget.reserve(try AudioAllocationBudget.product(UInt64(channel.audioUnitEffects.count), 512))
        }
        return budget.reserved
    }

    public init(session: MixerSession, channelParents: [UUID: UUID], sampleRate: Double, maxFrames: UInt32, delayBudgetBytes: UInt64 = 128 * 1024 * 1024) throws {
        _ = try session.validated()
        guard session.channels.count <= Int(UInt32.max), maxFrames > 0,
              let graph = rl_au_graph_create(UInt32(session.channels.count), maxFrames, sampleRate) else {
            throw SessionError.invalid("Could not allocate the Audio Unit graph at this output sample rate.")
        }
        bridge = graph; topology = session; self.sampleRate = sampleRate
        do {
            guard rl_au_graph_set_latency_limits(bridge, UInt32(ceil(sampleRate * 2)), min(delayBudgetBytes, 128 * 1024 * 1024)) else {
                throw SessionError.invalid("Could not configure the Audio Unit delay budget.")
            }
            for (source, channel) in session.channels.enumerated() {
                guard channelParents[channel.id] == channel.id || channelParents[channel.id] == nil else { continue }
                guard rl_au_graph_enable_source(bridge, UInt32(source)) else { throw SessionError.invalid("Could not configure effect alignment for \(channel.name).") }
                for (slotIndex, slot) in channel.audioUnitEffects.enumerated() {
                    guard slotIndex <= Int(UInt32.max) else { throw SessionError.invalid("The effect rack exceeds addressable size.") }
                    let record = try Self.prepare(slot, source: UInt32(source), slotIndex: UInt32(slotIndex), channel: channel, rate: sampleRate)
                    units.append(record)
                    guard rl_au_graph_append(bridge, record.source, record.slot, record.renderBlock, record.scheduleBlock, slot.isBypassed, record.latencyFrames) else {
                        throw SessionError.invalid("Could not prepare \(slot.descriptor.name) on \(channel.name). Racks are limited to two seconds of reported latency and the remaining output delay budget.")
                    }
                }
            }
            guard rl_au_graph_finalize(bridge) else { throw SessionError.invalid("Could not allocate effect alignment delays within the remaining output delay budget. Reduce rack latency or the number of input channels.") }
        } catch {
            // All stored properties are initialized, so Swift runs deinit when
            // this initializer throws. Do not free the bridge a second time.
            throw error
        }
    }
    deinit {
        // The owner must stop/destroy the render callback before releasing us.
        rl_au_graph_destroy(bridge)
        for record in units { record.wrapper.auAudioUnit.deallocateRenderResources() }
    }
    public func install(on render: OpaquePointer) {
        rl_render_set_group_processor(render, rl_au_graph_process, UnsafeMutableRawPointer(bridge))
    }
    public var failureMessage: String? {
        if let changed = checkLatency() { return changed }
        var source: UInt32 = 0, slot: UInt32 = 0
        let error = rl_au_graph_error(bridge, &source, &slot)
        guard error != 0 else { return nil }
        if let record = units.first(where: { $0.source == source && $0.slot == slot }) {
            return "\(record.name) on \(record.channelName) stopped audio (status \(error)). Reconfigure or remove this effect, then start audio again."
        }
        return "The Audio Unit graph stopped audio (status \(error)). Reconfigure the effect rack, then start audio again."
    }
    public static func topologyEquivalent(_ a: MixerSession, _ b: MixerSession) -> Bool {
        guard a.channels.count == b.channels.count else { return false }
        return zip(a.channels, b.channels).allSatisfy { left, right in
            left.id == right.id && left.audioUnitEffects.count == right.audioUnitEffects.count &&
            zip(left.audioUnitEffects, right.audioUnitEffects).allSatisfy { x, y in
                x.id == y.id && x.descriptor.component == y.descriptor.component && x.descriptor.version == y.descriptor.version && x.state == y.state
            }
        }
    }
    public func updateControls(session: MixerSession) throws {
        guard Self.topologyEquivalent(topology, session) else { throw SessionError.invalid("Audio Unit topology changed; rebuild the stopped graph before applying these controls.") }
        // Validate every changed assignment before applying any control.
        var changes: [(Int, [UInt64: Float], Bool)] = []
        for (index, record) in units.enumerated() {
            let channel = session.channels[Int(record.source)]
            let slot = channel.audioUnitEffects[Int(record.slot)]
            let values = Dictionary(slot.parameters.map { ($0.address, $0.value) }, uniquingKeysWith: { _, new in new })
            guard values.count == slot.parameters.count else { throw SessionError.invalid("Duplicate Audio Unit parameter assignments.") }
            for (address, value) in values { try Self.validate(value, address: address, parameters: record.parameters, name: record.name) }
            changes.append((index, values, slot.isBypassed))
        }
        for (index, values, bypassed) in changes {
            var record = units[index]
            for address in Set(values.keys).union(record.applied.keys) {
                guard let info = record.parameters[address] else { continue }
                let old = record.applied[address] ?? info.baseline, new = values[address] ?? info.baseline
                if old != new {
                    guard let parameter = record.wrapper.auAudioUnit.parameterTree?.parameter(withAddress: address) else {
                        throw SessionError.invalid("Could not update \(record.name). Audio must stop before retrying the rack.")
                    }
                    // UI edits use the AU parameter API on the control queue so
                    // vendor fullState and parameter observers remain current.
                    // Some AUv2 bridges advertise ramps but do not persist values
                    // sent only through AudioUnitScheduleParameters.
                    parameter.value = new
                }
            }
            if record.bypassed != bypassed {
                guard rl_au_graph_set_bypass(bridge, record.source, record.slot, bypassed) else { throw SessionError.invalid("Could not change effect bypass.") }
            }
            record.applied = values; record.bypassed = bypassed; units[index] = record
        }
        if let changed = checkLatency() { throw SessionError.invalid(changed) }
    }
    // AU latency may change when a parameter or vendor state changes. Poll only on
    // the engine control queue, then latch the callback silent until a new graph.
    private func checkLatency() -> String? {
        if let latencyFailure { return latencyFailure }
        for record in units {
            let current = Self.latencyFrames(record.wrapper.auAudioUnit.latency, rate: sampleRate)
            if current != record.latencyFrames {
                let message = "\(record.name) on \(record.channelName) changed its reported latency. Audio stopped to preserve channel alignment. Start audio again to rebuild its delays."
                latencyFailure = message
                rl_au_graph_invalidate(bridge, record.source, record.slot, -70004)
                return message
            }
        }
        return nil
    }
    private static func latencyFrames(_ seconds: Double, rate: Double) -> UInt32? {
        guard seconds.isFinite, seconds >= 0, seconds <= 2 else { return nil }
        return UInt32((seconds * rate).rounded())
    }
    /// Control-queue-only state snapshot. Never called from the render callback.
    public func snapshotStates() throws -> [UUID: [UUID: AudioUnitState]] {
        var result: [UUID: [UUID: AudioUnitState]] = [:]
        for record in units {
            guard let state = record.wrapper.auAudioUnit.fullState else { continue }
            let bytes = try PropertyListSerialization.data(fromPropertyList: state, format: .binary, options: 0)
            guard bytes.count <= 256 * 1024 else { throw SessionError.invalid("\(record.name) state exceeds 256 KiB.") }
            let descriptor = topology.channels[Int(record.source)].audioUnitEffects[Int(record.slot)].descriptor
            result[record.channelID, default: [:]][record.slotID] = .init(component: descriptor.component, componentVersion: descriptor.version, propertyList: bytes)
        }
        return result
    }
    private static func validate(_ value: Float, address: UInt64, parameters: [UInt64: ParameterInfo], name: String) throws {
        guard let info = parameters[address], info.writable, value.isFinite, (info.minimum...info.maximum).contains(value) else {
            throw SessionError.invalid("\(name) has an unavailable, read-only, or out-of-range parameter (\(address)).")
        }
    }
    private static func prepare(_ slot: AudioUnitEffectSlot, source: UInt32, slotIndex: UInt32, channel: MixerChannel, rate: Double) throws -> UnitRecord {
        let id = slot.descriptor.component
        var description = AudioComponentDescription(componentType: id.type, componentSubType: id.subtype, componentManufacturer: id.manufacturer, componentFlags: 0, componentFlagsMask: 0)
        guard let component = AudioComponentFindNext(nil, &description) else { throw SessionError.invalid("\(slot.descriptor.name) is missing. Install it or remove the effect from \(channel.name).") }
        var version: UInt32 = 0
        guard AudioComponentGetVersion(component, &version) == noErr,
              slot.descriptor.version == nil || slot.descriptor.version == version else { throw SessionError.invalid("\(slot.descriptor.name) changed version. Inspect it again before starting audio.") }
        let completion = LiveAUInstantiation()
        AVAudioUnit.instantiate(with: description, options: .loadOutOfProcess) { wrapper, error in
            completion.set(wrapper.map(Result.success) ?? .failure(error ?? SessionError.invalid("Audio Unit did not return an instance.")))
        }
        let start = DispatchTime.now().uptimeNanoseconds
        while completion.get() == nil {
            guard DispatchTime.now().uptimeNanoseconds - start < 10_000_000_000 else {
                completion.abandon()
                throw SessionError.invalid("\(slot.descriptor.name) timed out while preparing. Audio remains stopped.")
            }
            _ = RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.01))
        }
        let wrapper = try completion.get()!.get(), unit = wrapper.auAudioUnit
        var prepared = false
        defer { if !prepared { unit.deallocateRenderResources() } }
        if let state = slot.state {
            guard state.isValid(for: slot.descriptor),
                  let values = try PropertyListSerialization.propertyList(from: state.propertyList, options: [], format: nil) as? [String: Any] else {
                throw SessionError.invalid("\(slot.descriptor.name) has invalid saved state.")
            }
            unit.fullState = values
        }
        guard unit.inputBusses.count > 0, unit.outputBusses.count > 0,
              unit.inputBusses.count <= 64, unit.outputBusses.count <= 64,
              let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2) else { throw SessionError.invalid("\(slot.descriptor.name) does not expose stereo main buses.") }
        unit.inputBusses[0].isEnabled = true; unit.outputBusses[0].isEnabled = true
        unit.maximumFramesToRender = 512; unit.isRenderingOffline = false
        try unit.inputBusses[0].setFormat(format); try unit.outputBusses[0].setFormat(format)
        guard [unit.inputBusses[0].format, unit.outputBusses[0].format].allSatisfy({ $0.sampleRate == rate && $0.channelCount == 2 && $0.commonFormat == .pcmFormatFloat32 && !$0.isInterleaved }) else { throw SessionError.invalid("\(slot.descriptor.name) rejected this output's stereo sample format.") }
        let parameters = unit.parameterTree?.allParameters ?? []
        guard parameters.count <= 4096 else { throw SessionError.invalid("\(slot.descriptor.name) exposes too many parameters.") }
        var metadata: [UInt64: ParameterInfo] = [:]
        for parameter in parameters {
            guard parameter.minValue.isFinite, parameter.maxValue.isFinite, parameter.value.isFinite, parameter.minValue <= parameter.maxValue else { continue }
            metadata[parameter.address] = .init(minimum: parameter.minValue, maximum: parameter.maxValue, baseline: parameter.value,
                                               writable: parameter.flags.contains(.flag_IsWritable), canRamp: parameter.flags.contains(.flag_CanRamp))
        }
        for assignment in slot.parameters {
            try validate(assignment.value, address: assignment.address, parameters: metadata, name: slot.descriptor.name)
            unit.parameterTree?.parameter(withAddress: assignment.address)?.value = assignment.value
        }
        // Fetch both host blocks before allocation, as required by the AU contract.
        let renderBlock = unit.renderBlock, scheduleBlock = unit.scheduleParameterBlock
        try unit.allocateRenderResources(); unit.reset()
        guard let latency = latencyFrames(unit.latency, rate: rate) else { throw SessionError.invalid("\(slot.descriptor.name) reports an invalid latency or more than two seconds of delay.") }
        prepared = true
        return .init(source: source, slot: slotIndex, channelID: channel.id, slotID: slot.id, channelName: channel.name,
                     name: slot.descriptor.name, wrapper: wrapper, latencyFrames: latency, renderBlock: renderBlock, scheduleBlock: scheduleBlock, parameters: metadata,
                     applied: Dictionary(slot.parameters.map { ($0.address, $0.value) }, uniquingKeysWith: { _, value in value }), bypassed: slot.isBypassed)
    }
}
private final class LiveAUInstantiation: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Result<AVAudioUnit, Error>?
    private var abandoned = false
    func set(_ result: Result<AVAudioUnit, Error>) { lock.lock(); if !abandoned { value = result }; lock.unlock() }
    func abandon() { lock.lock(); abandoned = true; value = nil; lock.unlock() }
    func get() -> Result<AVAudioUnit, Error>? { lock.lock(); defer { lock.unlock() }; return value }
}
