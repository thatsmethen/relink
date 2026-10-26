import Foundation
import CoreAudio
import Combine
import AppKit
import AVFoundation
import RaveLinkCore
import RaveLinkRealtime

public struct AudioDeviceInfo: Identifiable, Codable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let inputChannels: Int
    public let outputChannels: Int
    public let sampleRate: Double
    public let isVirtual: Bool
}
public struct AudioApplicationInfo: Identifiable, Codable, Equatable, Sendable {
    public let id: String
    public let name: String
}
public struct AudioDiagnostics: Codable, Equatable, Sendable {
    /// Unique native capture IOProcs; repeated logical inputs share capture.
    public var captureSourceCount: UInt32 = 0
    public var capturedFrames: UInt64 = 0
    public var renderedFrames: UInt64 = 0
    public var droppedFrames: UInt64 = 0
    public var underrunFrames: UInt64 = 0
    public var oversizedBlocks: UInt64 = 0
    public var timestampDiscontinuities: UInt64 = 0
    public var timestampFallbackNodes: UInt32 = 0
    public var clockMismatchPairs: UInt32 = 0
    /// AU rack alignment only; excludes device/FIFO/driver latency.
    public var effectAlignmentLatencySeconds: Double = 0
    public init() {}
    private enum CodingKeys: String, CodingKey { case captureSourceCount, capturedFrames, renderedFrames, droppedFrames, underrunFrames, oversizedBlocks, timestampDiscontinuities, timestampFallbackNodes, clockMismatchPairs, effectAlignmentLatencySeconds }
    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        captureSourceCount = try values.decodeIfPresent(UInt32.self, forKey: .captureSourceCount) ?? 0
        capturedFrames = try values.decode(UInt64.self, forKey: .capturedFrames)
        renderedFrames = try values.decode(UInt64.self, forKey: .renderedFrames)
        droppedFrames = try values.decode(UInt64.self, forKey: .droppedFrames)
        underrunFrames = try values.decode(UInt64.self, forKey: .underrunFrames)
        oversizedBlocks = try values.decode(UInt64.self, forKey: .oversizedBlocks)
        timestampDiscontinuities = try values.decodeIfPresent(UInt64.self, forKey: .timestampDiscontinuities) ?? 0
        timestampFallbackNodes = try values.decodeIfPresent(UInt32.self, forKey: .timestampFallbackNodes) ?? 0
        clockMismatchPairs = try values.decodeIfPresent(UInt32.self, forKey: .clockMismatchPairs) ?? 0
        effectAlignmentLatencySeconds = try values.decodeIfPresent(Double.self, forKey: .effectAlignmentLatencySeconds) ?? 0
    }
}
public enum AudioRoutingError: LocalizedError {
    case configuration(String)
    case coreAudio(String, OSStatus)
    public var errorDescription: String? {
        switch self {
        case .configuration(let message): return message
        case .coreAudio(let operation, let status):
            let bytes = [24, 16, 8, 0].map { UInt8(truncatingIfNeeded: UInt32(bitPattern: status) >> $0) }
            let fourCC = bytes.allSatisfy { $0 >= 32 && $0 < 127 } ? " ('\(String(bytes: bytes, encoding: .ascii) ?? "" )')" : ""
            return "\(operation) failed: CoreAudio \(status)\(fourCC). Check that the device is connected and audio recording permission is allowed in System Settings → Privacy & Security."
        }
    }
}

private func checked(_ status: OSStatus, _ operation: String) throws {
    guard status == noErr else { throw AudioRoutingError.coreAudio(operation, status) }
}
private final class GraphCancellation: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false
    func cancel() { lock.lock(); cancelled = true; lock.unlock() }
    func check() throws {
        lock.lock(); let cancelled = self.cancelled; lock.unlock()
        if cancelled { throw CancellationError() }
    }
}
private enum Hardware {
    static let system = AudioObjectID(kAudioObjectSystemObject)
    static func address(_ selector: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> AudioObjectPropertyAddress {
        .init(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
    }
    static func ids(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> [AudioObjectID] {
        var addr = address(selector, scope), size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(object, &addr, 0, nil, &size) == noErr, size > 0 else { return [] }
        var result = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
        let status = result.withUnsafeMutableBytes { AudioObjectGetPropertyData(object, &addr, 0, nil, &size, $0.baseAddress!) }
        return status == noErr ? result : []
    }
    static func string(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) -> String? {
        var addr = address(selector), value: Unmanaged<CFString>?, size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        guard AudioObjectGetPropertyData(object, &addr, 0, nil, &size, &value) == noErr else { return nil }
        return value?.takeRetainedValue() as String?
    }
    static func isVirtual(_ object: AudioObjectID) -> Bool {
        var addr = address(kAudioDevicePropertyTransportType), value: UInt32 = 0, size = UInt32(MemoryLayout<UInt32>.size)
        guard AudioObjectGetPropertyData(object, &addr, 0, nil, &size, &value) == noErr else { return false }
        return value == kAudioDeviceTransportTypeVirtual
    }
    static func rate(_ object: AudioObjectID) throws -> Double {
        var addr = address(kAudioDevicePropertyNominalSampleRate), result: Double = 0, size = UInt32(MemoryLayout<Double>.size)
        try checked(AudioObjectGetPropertyData(object, &addr, 0, nil, &size, &result), "Read device sample rate")
        try RealtimeDeviceLimits.validateRate(result)
        return result
    }
    static func buffer(_ object: AudioObjectID, rate: Double) throws -> RealtimeDeviceBuffer {
        var addr = address(kAudioDevicePropertyBufferFrameSize), frames: UInt32 = 0, size = UInt32(MemoryLayout<UInt32>.size)
        try checked(AudioObjectGetPropertyData(object, &addr, 0, nil, &size, &frames), "Read device buffer size")
        try RealtimeDeviceLimits.validateBuffer(rate: rate, frames: frames)
        var maximum = frames
        addr = address(kAudioDevicePropertyUsesVariableBufferFrameSizes)
        if AudioObjectHasProperty(object, &addr) {
            var variable: UInt32 = 0
            try checked(AudioObjectGetPropertyData(object, &addr, 0, nil, &size, &variable), "Read maximum device buffer size")
            maximum = max(maximum, variable)
        }
        try RealtimeDeviceLimits.validateBuffer(rate: rate, frames: maximum)
        return .init(minimum: frames, maximum: maximum)
    }
    static func validateAlive(_ object: AudioObjectID) throws {
        var addr = address(kAudioDevicePropertyDeviceIsAlive), alive: UInt32 = 0, size = UInt32(MemoryLayout<UInt32>.size)
        try checked(AudioObjectGetPropertyData(object, &addr, 0, nil, &size, &alive), "Check audio device availability")
        guard alive != 0 else { throw AudioRoutingError.configuration("An active audio device is no longer available. Reconnect it, then start audio again.") }
    }
    static func channels(_ object: AudioObjectID, _ scope: AudioObjectPropertyScope) -> Int {
        var addr = address(kAudioDevicePropertyStreamConfiguration, scope), size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(object, &addr, 0, nil, &size) == noErr, size >= MemoryLayout<AudioBufferList>.size else { return 0 }
        let memory = UnsafeMutableRawPointer.allocate(byteCount: Int(size), alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { memory.deallocate() }
        guard AudioObjectGetPropertyData(object, &addr, 0, nil, &size, memory) == noErr else { return 0 }
        return UnsafeMutableAudioBufferListPointer(memory.assumingMemoryBound(to: AudioBufferList.self)).reduce(0) { $0 + Int($1.mNumberChannels) }
    }
    static func devices() -> [(AudioObjectID, AudioDeviceInfo)] {
        ids(system, kAudioHardwarePropertyDevices).compactMap { object in
            guard let uid = string(object, kAudioDevicePropertyDeviceUID), !uid.hasPrefix("au.textual.ravelink.tap.") else { return nil }
            return (object, AudioDeviceInfo(id: uid, name: string(object, kAudioObjectPropertyName) ?? uid,
                inputChannels: channels(object, kAudioObjectPropertyScopeInput), outputChannels: channels(object, kAudioObjectPropertyScopeOutput), sampleRate: (try? rate(object)) ?? 0, isVirtual: isVirtual(object)))
        }
    }
    static func validateFloatStreams(_ object: AudioObjectID, scope: AudioObjectPropertyScope) throws {
        let streams = ids(object, kAudioDevicePropertyStreams, scope)
        guard !streams.isEmpty else { throw AudioRoutingError.configuration("The selected device has no audio streams in the requested direction.") }
        for stream in streams {
            var addr = address(kAudioStreamPropertyVirtualFormat), format = AudioStreamBasicDescription(), size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
            try checked(AudioObjectGetPropertyData(stream, &addr, 0, nil, &size, &format), "Read stream format")
            guard isSupportedAudioStream(format, deviceRate: try rate(object)) else {
                throw AudioRoutingError.configuration("This device does not expose native Float32 audio. Select a CoreAudio Float32 device or an aggregate device in Audio MIDI Setup.")
            }
        }
    }
    static func processPID(_ object: AudioObjectID) -> pid_t? {
        var addr = address(kAudioProcessPropertyPID), value: pid_t = 0, size = UInt32(MemoryLayout<pid_t>.size)
        return AudioObjectGetPropertyData(object, &addr, 0, nil, &size, &value) == noErr ? value : nil
    }
    static func processes() -> [AudioProcessIdentity] {
        ids(system, kAudioHardwarePropertyProcessObjectList).compactMap {
            guard let pid = processPID($0) else { return nil }
            return ApplicationIdentityResolver.process(id: $0, bundleID: string($0, kAudioProcessPropertyBundleID) ?? "", pid: pid)
        }
    }
    static func ownProcess() -> AudioObjectID? {
        var addr = address(kAudioHardwarePropertyTranslatePIDToProcessObject), pid = getpid(), value: AudioObjectID = 0, size = UInt32(MemoryLayout<AudioObjectID>.size)
        guard AudioObjectGetPropertyData(system, &addr, UInt32(MemoryLayout<pid_t>.size), &pid, &size, &value) == noErr, value != 0 else { return nil }
        return value
    }
    static func applications() -> [AudioApplicationInfo] {
        var names: [String: String] = [:]
        for p in processes() where p.pid != getpid() && !p.applicationBundleID.isEmpty && p.applicationBundleID != "au.textual.ravelink" {
            names[p.applicationBundleID] = p.applicationName
        }
        return names.map { AudioApplicationInfo(id: $0.key, name: $0.value) }.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }
}

/// Owns an IOProc and, optionally, the private tap and aggregate that feed it.
/// Destroy order is stop → unregister callback → destroy aggregate → destroy tap.
private final class CaptureNode {
    let channelID: UUID
    let device: AudioObjectID
    let rate: Double
    let buffer: RealtimeDeviceBuffer
    let context: OpaquePointer
    var proc: AudioDeviceIOProcID?
    var tap: AudioObjectID = 0
    var tapDescription: CATapDescription?
    var tapApplicationBundleID: String?
    var rings: [OpaquePointer] = []
    var aggregate: AudioObjectID = 0
    var started = false
    var abandoned = false
    init(channelID: UUID, device: AudioObjectID, rate: Double, buffer: RealtimeDeviceBuffer, context: OpaquePointer) {
        self.channelID = channelID; self.device = device; self.rate = rate; self.buffer = buffer; self.context = context
    }
    func start() throws {
        rl_capture_reset_timing(context)
        try checked(AudioDeviceCreateIOProcID(device, rl_capture_io_proc(), UnsafeMutableRawPointer(context), &proc), "Create input callback")
        try checked(AudioDeviceStart(device, proc), "Start input")
        started = true
    }
    @discardableResult func stop() -> Bool {
        if let proc {
            if started { _ = AudioDeviceStop(device, proc) }
            // A successful unregister is the boundary after which C memory can
            // be released. On failure, retain its storage until process exit.
            guard AudioDeviceDestroyIOProcID(device, proc) == noErr else { return false }
        }
        proc = nil; started = false
        return true
    }
    deinit {
        guard !abandoned, stop() else { return }
        if aggregate != 0 { AudioHardwareDestroyAggregateDevice(aggregate) }
        if tap != 0 { AudioHardwareDestroyProcessTap(tap) }
        rl_capture_destroy(context)
    }
}
private final class SoundCheckOwner {
    let channelID: UUID
    let context: OpaquePointer
    init(channelID: UUID, sampleRate: Double) throws {
        guard let context = rl_sound_check_create(sampleRate, 30) else {
            throw AudioRoutingError.configuration("Sound Check could not allocate its 30-second clip at this device's sample rate.")
        }
        self.channelID = channelID; self.context = context
    }
    deinit { rl_sound_check_destroy(context) }
    func command(_ command: SoundCheckCommand) throws {
        let code: UInt32
        switch command { case .record: code = 1; case .stop: code = 2; case .play: code = 3; case .clear: code = 4 }
        guard rl_sound_check_command(context, code) else {
            throw AudioRoutingError.configuration("Sound Check is receiving commands too quickly. Wait a moment and try again.")
        }
    }
    func snapshot() -> SoundCheckSnapshot {
        var rawMode: UInt32 = 0, frames: UInt64 = 0
        rl_sound_check_snapshot(context, &rawMode, &frames)
        let mode: SoundCheckMode
        switch rawMode { case 1: mode = .recording; case 2: mode = .ready; case 3: mode = .playing; default: mode = .empty }
        return .init(channelID: channelID, mode: mode, recordedFrames: frames,
                     sampleRate: rl_sound_check_rate(context), maximumSeconds: 30, peak: rl_sound_check_take_peak(context))
    }
}
private final class OutputNode {
    let device: AudioObjectID
    let uid: String
    let rate: Double
    let buffer: RealtimeDeviceBuffer
    let buses: [UUID]
    let context: OpaquePointer
    var audioUnits: LiveAudioUnitGraph?
    var soundCheck: SoundCheckOwner?
    var proc: AudioDeviceIOProcID?
    var started = false
    var abandoned = false
    init(device: AudioObjectID, uid: String, rate: Double, buffer: RealtimeDeviceBuffer, buses: [UUID], context: OpaquePointer) {
        self.device = device; self.uid = uid; self.rate = rate; self.buffer = buffer; self.buses = buses; self.context = context
    }
    func prepare() throws {
        if proc == nil { try checked(AudioDeviceCreateIOProcID(device, rl_render_io_proc(), UnsafeMutableRawPointer(context), &proc), "Create output callback") }
    }
    func start() throws {
        try prepare()
        try checked(AudioDeviceStart(device, proc), "Start output")
        started = true
    }
    @discardableResult func stop() -> Bool {
        if let proc {
            if started { _ = AudioDeviceStop(device, proc) }
            guard AudioDeviceDestroyIOProcID(device, proc) == noErr else { return false }
        }
        proc = nil; started = false
        return true
    }
    deinit {
        if !abandoned, stop() { rl_render_destroy(context) }
        else {
            // A driver that kept an IOProc may still reference the bridge and
            // cached AU blocks. Keep their Swift owners alive until process exit.
            if let audioUnits { _ = Unmanaged.passRetained(audioUnits) }
            if let soundCheck { _ = Unmanaged.passRetained(soundCheck) }
        }
    }
}
// Ownership transfers worker → main only after configure finishes; it transfers
// back before stop. Live controls use C atomics and cached AU schedule blocks;
// meters use C atomics. The GUI never hosts this graph directly.
private final class AudioGraph: @unchecked Sendable {
    var captures: [CaptureNode] = []
    var outputs: [OutputNode] = []
    var rings: [OpaquePointer] = []
    var session: MixerSession
    let channelParents: [UUID: UUID]
    let soundCheckChannelID: UUID?
    let excludedApplications: [String]
    var applicationProcesses: [String: [AudioObjectID]] = [:]
    var events: HardwareEventMonitor?
    static let maxFrames: UInt32 = 16384
    init(session: MixerSession, channelParents: [UUID: UUID], soundCheckChannelID: UUID? = nil, excludedApplications: [String] = []) {
        self.session = session; self.channelParents = channelParents; self.soundCheckChannelID = soundCheckChannelID
        self.excludedApplications = excludedApplications
    }
    @discardableResult func stop() -> String? {
        events?.removeAll(); events = nil
        var safe = true
        for capture in captures { if !capture.stop() { safe = false } }
        for output in outputs { if !output.stop() { safe = false } }
        if !safe {
            // Do not free any graph memory that a misbehaving driver might still
            // reference. This exceptional quarantine intentionally lasts until exit.
            captures.forEach { $0.abandoned = true }; outputs.forEach { $0.abandoned = true }
            captures.removeAll(); outputs.removeAll(); rings.removeAll()
            return "The audio driver failed to release an active callback. Quit RaveLink now to release audio safely before continuing."
        }
        captures.removeAll(); outputs.removeAll()
        rings.forEach(rl_ring_destroy); rings.removeAll()
        return nil
    }
    deinit { stop() }
    func silence() {
        suspend(true)
        for output in outputs {
            for (index, _) in output.buses.enumerated() { rl_render_bus_controls(output.context, UInt32(index), 0, true) }
        }
    }
    func suspend(_ value: Bool) { outputs.forEach { rl_render_set_suspended($0.context, value) } }
    static func sameTopology(_ a: MixerSession, _ b: MixerSession) -> Bool {
        a.channels.map(\.id) == b.channels.map(\.id) && a.channels.map(\.source) == b.channels.map(\.source)
        && a.buses.map(\.id) == b.buses.map(\.id) && a.buses.map(\.deviceUID) == b.buses.map(\.deviceUID)
        && a.buses.map(\.channelOffset) == b.buses.map(\.channelOffset)
        && LiveAudioUnitGraph.topologyEquivalent(a, b)
    }
    func controls(_ session: MixerSession) throws {
        for output in outputs { try output.audioUnits?.updateControls(session: session) }
        let anySolo = session.channels.contains { $0.solo }
        let sendsByChannel = Dictionary(grouping: session.sends, by: \.channelID).mapValues {
            Dictionary(uniqueKeysWithValues: $0.map { ($0.busID, $0) })
        }
        let busesByID = Dictionary(uniqueKeysWithValues: session.buses.map { ($0.id, $0) })
        for capture in captures {
            // Effects are applied after grouped sources have been summed at each
            // destination rate. Capture remains raw to avoid per-source compression.
            rl_capture_controls(capture.context, 0, 0, -80, 0, 1)
        }
        for output in outputs {
            for (i, channel) in session.channels.enumerated() {
                let fx = channel.effects
                rl_render_source_controls(output.context, UInt32(i), !channel.muted && (!anySolo || channel.solo), fx.gainDB, fx.highPassHz, fx.gateThresholdDB, fx.compressorThresholdDB, fx.compressorRatio)
                for (b, busID) in output.buses.enumerated() {
                    let send = sendsByChannel[channel.id]?[busID]
                    rl_render_send_controls(output.context, UInt32(i), UInt32(b), send?.gainDB ?? 0, send?.pan ?? 0, send?.muted ?? true)
                }
            }
            for (b, id) in output.buses.enumerated() {
                guard let bus = busesByID[id] else { continue }
                rl_render_bus_controls(output.context, UInt32(b), bus.gainDB, bus.muted)
            }
        }
        self.session = session
    }
    func configure(cancellation: GraphCancellation) throws {
        try cancellation.check()
        var allocation = AudioAllocationBudget()
        guard session.channels.count <= UInt32.max, session.buses.count <= UInt32.max else { throw AudioRoutingError.configuration("This graph has too many nodes for the audio device API.") }
        let available = Dictionary(uniqueKeysWithValues: Hardware.devices().map { ($0.1.id, ($0.0, $0.1)) })
        let assigned = session.buses.filter { $0.deviceUID != nil }
        guard !session.channels.isEmpty else { throw AudioRoutingError.configuration("Add an input channel before starting audio.") }
        guard !assigned.isEmpty else { throw AudioRoutingError.configuration("Choose an output device on at least one mix bus before starting audio.") }
        // Bound memory by machine resources, rather than imposing a product channel limit.
        let outputGroups = Dictionary(grouping: assigned, by: { $0.deviceUID! })
        for uid in outputGroups.keys.sorted() {
            try cancellation.check()
            guard let (device, info) = available[uid], let buses = outputGroups[uid] else { throw AudioRoutingError.configuration("An output device is disconnected. Select an available device on each assigned mix bus.") }
            for bus in buses {
                guard bus.channelOffset >= 0, bus.channelOffset + 1 < info.outputChannels else { throw AudioRoutingError.configuration("\(bus.name): \(info.name) does not provide the requested stereo output pair.") }
            }
            try Hardware.validateFloatStreams(device, scope: kAudioObjectPropertyScopeOutput)
            let rate = try Hardware.rate(device)
            let buffer = try Hardware.buffer(device, rate: rate)
            try allocation.reserve(try AudioAllocationBudget.output(sources: session.channels.count, buses: buses.count, frames: Self.maxFrames))
            guard let context = rl_render_create(rate, buffer.maximum, UInt32(session.channels.count), UInt32(buses.count)) else { throw AudioRoutingError.configuration("Not enough memory to prepare this mix. Reduce channels or mix buses.") }
            let output = OutputNode(device: device, uid: uid, rate: rate, buffer: buffer, buses: buses.map(\.id), context: context)
            outputs.append(output)
            for (index, bus) in buses.enumerated() { rl_render_set_bus(context, UInt32(index), UInt32(bus.channelOffset)) }
            try output.prepare() // Registers our audio process before creating an exclusion tap.
            try cancellation.check()
        }
        let usesSystemRemainder = session.channels.contains { $0.source.kind == .system }
        let dedicated = Set(session.channels.filter { $0.source.kind == .application }.compactMap { $0.source.bundleID }
                            + (usesSystemRemainder ? excludedApplications : []))
        // Every tap uses one inventory snapshot, so the health watch describes
        // exactly the memberships used for inclusion and catch-all exclusion.
        let processes = (usesSystemRemainder || !dedicated.isEmpty) ? Hardware.processes() : []
        applicationProcesses = applicationMemberships(bundleIDs: usesSystemRemainder ? dedicated.union(["au.textual.ravelink"]) : dedicated, processes: processes, excludingPID: getpid())
        let capturePlan = CaptureSourcePlan(sources: session.channels.map(\.source))
        for (index, channel) in session.channels.enumerated() {
            try cancellation.check()
            if capturePlan.roots[index] != index {
                for output in outputs {
                    guard rl_render_set_source_alias(output.context, UInt32(index), UInt32(capturePlan.roots[index])) else {
                        throw AudioRoutingError.configuration("A shared input has an invalid capture source. Start audio again to rebuild the route.")
                    }
                }
                continue
            }
            if channel.source.kind == .tone {
                for output in outputs { rl_render_set_source(output.context, UInt32(index), nil, output.rate, true) }
                continue
            }
            var device: AudioObjectID = 0, tap: AudioObjectID = 0, aggregate: AudioObjectID = 0
            var tapDescription: CATapDescription?
            do {
                if channel.source.kind == .device {
                    guard let uid = channel.source.deviceUID, let (id, info) = available[uid] else { throw AudioRoutingError.configuration("\(channel.name): choose an available input device.") }
                    guard channel.source.channelOffset >= 0, channel.source.channelOffset < info.inputChannels else { throw AudioRoutingError.configuration("\(channel.name): the selected input channel does not exist on \(info.name).") }
                    device = id
                } else {
                    let description: CATapDescription
                    let selection: TapProcessSelection
                    if channel.source.kind == .application {
                        guard let bundle = channel.source.bundleID, !bundle.isEmpty else { throw AudioRoutingError.configuration("\(channel.name): choose an application to capture.") }
                        _ = try validatedExcludedApplications([bundle])
                        selection = tapProcessSelection(bundleID: bundle, dedicated: dedicated, processes: processes, ownPID: getpid(), ownObjectID: 0)
                        guard !selection.processIDs.isEmpty else { throw AudioRoutingError.configuration("\(channel.name): open the selected application and play audio before starting capture.") }
                        description = CATapDescription(stereoMixdownOfProcesses: selection.processIDs)
                    } else {
                        guard let own = Hardware.ownProcess() else { throw AudioRoutingError.configuration("CoreAudio could not identify RaveLink's audio process. Restart RaveLink before using system audio capture.") }
                        selection = tapProcessSelection(bundleID: nil, dedicated: dedicated, processes: processes, ownPID: getpid(), ownObjectID: own)
                        description = CATapDescription(stereoGlobalTapButExcludeProcesses: selection.processIDs)
                    }
                    configureNativeTapRestoration(description, bundleIDs: selection.bundleIDs)
                    description.name = "RaveLink — \(channel.name)"
                    description.isPrivate = true
                    // Preparation must never suppress another app. Playback is
                    // suppressed only after the main actor accepts this generation.
                    description.muteBehavior = .unmuted
                    tapDescription = description
                    try checked(AudioHardwareCreateProcessTap(description, &tap), "Create application audio tap")
                    let composition: [String: Any] = [
                        kAudioAggregateDeviceNameKey: "RaveLink private capture",
                        kAudioAggregateDeviceUIDKey: "au.textual.ravelink.tap.\(UUID().uuidString)",
                        kAudioAggregateDeviceIsPrivateKey: true,
                        kAudioAggregateDeviceTapAutoStartKey: false,
                        kAudioAggregateDeviceTapListKey: [[kAudioSubTapUIDKey: description.uuid.uuidString, kAudioSubTapDriftCompensationKey: true]]
                    ]
                    try checked(AudioHardwareCreateAggregateDevice(composition as CFDictionary, &aggregate), "Create private capture aggregate")
                    device = aggregate
                }
                try Hardware.validateFloatStreams(device, scope: kAudioObjectPropertyScopeInput)
                let rate = try Hardware.rate(device)
                let buffer = try Hardware.buffer(device, rate: rate)
                try allocation.reserve(UInt64(Self.maxFrames) * 8 + UInt64(outputs.count) * 8 + 4096)
                guard let context = rl_capture_create(rate, UInt32(channel.source.kind == .device ? channel.source.channelOffset : 0), buffer.maximum, UInt32(outputs.count)) else { throw AudioRoutingError.configuration("Not enough memory to prepare the input channel.") }
                let capture = CaptureNode(channelID: channel.id, device: device, rate: rate, buffer: buffer, context: context)
                capture.tap = tap; capture.aggregate = aggregate; tap = 0; aggregate = 0
                rl_capture_allow_idle_pauses(context, capture.tap != 0)
                capture.tapDescription = tapDescription
                capture.tapApplicationBundleID = channel.source.kind == .application ? channel.source.bundleID : nil
                captures.append(capture)
                for (outputIndex, output) in outputs.enumerated() {
                    let ringPlan = try AudioAllocationBudget.ring(ratio: rate / output.rate, frames: Self.maxFrames)
                    try allocation.reserve(ringPlan.bytes)
                    guard let ring = rl_ring_create(ringPlan.capacity) else { throw AudioRoutingError.configuration("Not enough memory to prepare audio buffers.") }
                    rings.append(ring)
                    capture.rings.append(ring)
                    guard rl_ring_prepare_clock(ring, rate, output.rate) else { throw AudioRoutingError.configuration("The input and output sample rates are unsupported or too far apart. Use rates from 8 to 384 kHz that differ by no more than 24 times in Audio MIDI Setup.") }
                    guard rl_ring_prepare_buffers(ring, buffer.maximum, output.buffer.maximum) else {
                        throw AudioRoutingError.configuration("The input and output device buffers cannot be prepared safely. \(RealtimeDeviceLimits.bufferGuidance)")
                    }
                    rl_capture_set_destination(context, UInt32(outputIndex), ring)
                    rl_render_set_source(output.context, UInt32(index), ring, rate, false)
                }
            } catch {
                if aggregate != 0 { AudioHardwareDestroyAggregateDevice(aggregate) }
                if tap != 0 { AudioHardwareDestroyProcessTap(tap) }
                throw error
            }
        }
        let indexes = Dictionary(uniqueKeysWithValues: session.channels.enumerated().map { ($0.element.id, $0.offset) })
        for output in outputs {
            for (index, channel) in session.channels.enumerated() {
                let parent = channelParents[channel.id] ?? channel.id
                guard let leader = indexes[parent] else { throw AudioRoutingError.configuration("An input group is missing its primary source.") }
                guard rl_render_set_source_group(output.context, UInt32(index), UInt32(leader)) else { throw AudioRoutingError.configuration("An input group has an invalid source ordering.") }
            }
            if let channelID = soundCheckChannelID {
                guard outputs.count == 1, output.buses.count == 1, let leader = indexes[channelID],
                      Set(channelParents.values) == [channelID] else {
                    throw AudioRoutingError.configuration("Sound Check requires one input group and one listening destination.")
                }
                guard output.rate <= 384000 else { throw AudioRoutingError.configuration("Sound Check supports output rates from 8 kHz through 384 kHz.") }
                try allocation.reserve(UInt64(ceil(output.rate * 30)) * 8 + 4096)
                let check = try SoundCheckOwner(channelID: channelID, sampleRate: output.rate)
                guard rl_render_set_sound_check(output.context, UInt32(leader), check.context) else {
                    throw AudioRoutingError.configuration("Sound Check could not attach to the selected input group.")
                }
                output.soundCheck = check
            }
            if session.channels.contains(where: { !$0.audioUnitEffects.isEmpty }) {
                try cancellation.check()
                try allocation.reserve(LiveAudioUnitGraph.estimatedHostOverhead(session: session, channelParents: channelParents))
                let effects = try LiveAudioUnitGraph(session: session, channelParents: channelParents, sampleRate: output.rate, maxFrames: Self.maxFrames,
                    delayBudgetBytes: min(128 * 1024 * 1024, allocation.limit - allocation.reserved))
                try allocation.reserve(effects.delayMemoryBytes)
                try cancellation.check()
                output.audioUnits = effects
                effects.install(on: output.context)
            }
        }
        try controls(session)
        // Prepare/start a silent candidate. A stale or cancelled generation can
        // never broadcast old mix settings after a delayed HAL call returns.
        for output in outputs {
            for (index, _) in output.buses.enumerated() { rl_render_bus_controls(output.context, UInt32(index), 0, true) }
        }
        // All objects are configured before any device callbacks can run.
        for capture in captures { try cancellation.check(); try capture.start() }
        for output in outputs { try cancellation.check(); try output.start() }
        try cancellation.check()
    }
    func activateTaps(cancellation: GraphCancellation) throws {
        for capture in captures {
            try cancellation.check()
            guard let description = capture.tapDescription else { continue }
            description.muteBehavior = .mutedWhenTapped
            var reference = Unmanaged.passUnretained(description)
            var address = Hardware.address(kAudioTapPropertyDescription)
            try checked(AudioObjectSetPropertyData(capture.tap, &address, 0, nil, UInt32(MemoryLayout.size(ofValue: reference)), &reference), "Activate application monitoring")
            try cancellation.check()
        }
    }
    func installNotifications(callback: @escaping @Sendable (HardwareEventMonitor.Event) -> Void) throws {
        let monitor = HardwareEventMonitor()
        try monitor.add(object: Hardware.system, selector: kAudioHardwarePropertyDevices, event: .deviceConfiguration, callback: callback)
        try monitor.add(object: Hardware.system, selector: kAudioHardwarePropertyProcessObjectList, event: .processMembership, callback: callback)
        try monitor.add(object: Hardware.system, selector: kAudioHardwarePropertyServiceRestarted, event: .audioServiceRestart, callback: callback)
        for (device, scope) in captures.map({ ($0.device, kAudioObjectPropertyScopeInput) }) + outputs.map({ ($0.device, kAudioObjectPropertyScopeOutput) }) {
            try monitor.add(object: device, selector: kAudioDevicePropertyDeviceIsAlive, event: .deviceConfiguration, callback: callback)
            try monitor.add(object: device, selector: kAudioDevicePropertyNominalSampleRate, event: .deviceConfiguration, callback: callback)
            try monitor.add(object: device, selector: kAudioDevicePropertyBufferFrameSize, event: .deviceConfiguration, callback: callback)
            try monitor.add(object: device, selector: kAudioDevicePropertyUsesVariableBufferFrameSizes, event: .deviceConfiguration, callback: callback)
            try monitor.add(object: device, selector: kAudioDevicePropertyStreams, scope: scope, event: .deviceConfiguration, callback: callback)
            try monitor.add(object: device, selector: kAudioDevicePropertyStreamConfiguration, scope: scope, event: .deviceConfiguration, callback: callback)
            for stream in Hardware.ids(device, kAudioDevicePropertyStreams, scope) {
                try monitor.add(object: stream, selector: kAudioStreamPropertyVirtualFormat, event: .deviceConfiguration, callback: callback)
            }
        }
        events = monitor
    }
    /// Caller gates all outputs before entering this transaction. Every tap uses
    /// the same inventory. Stop/unregister prevents old capture callbacks from
    /// appending frames after the consumer receives its reset request.
    func retargetTaps(processes: [AudioProcessIdentity], dedicated: Set<String>, cancellation: GraphCancellation) throws {
        // Inclusion taps are changed before catch-all exclusions. Keep original
        // app playback suppressed while tap callbacks are briefly unregistered.
        let taps = captures.filter { $0.tapDescription != nil }.sorted {
            ($0.tapApplicationBundleID != nil ? 0 : 1) < ($1.tapApplicationBundleID != nil ? 0 : 1)
        }
        guard let own = Hardware.ownProcess() else { throw AudioRoutingError.configuration("RaveLink lost its CoreAudio process identity. Start audio again to rebuild capture.") }
        for capture in taps {
            try cancellation.check()
            try setTapMute(capture, behavior: .muted)
        }
        try performTapRetargetTransaction(nodes: taps, checkCancellation: cancellation.check, stop: { capture in
            guard capture.stop() else { throw AudioRoutingError.configuration("An application capture callback could not stop during rerouting. The engine must be restarted.") }
        }, apply: { capture in
            guard let description = capture.tapDescription else { return }
            let selection = tapProcessSelection(bundleID: capture.tapApplicationBundleID, dedicated: dedicated, processes: processes, ownPID: getpid(), ownObjectID: own)
            description.processes = selection.processIDs
            configureNativeTapRestoration(description, bundleIDs: selection.bundleIDs)
            var reference = Unmanaged.passUnretained(description)
            var address = Hardware.address(kAudioTapPropertyDescription)
            try checked(AudioObjectSetPropertyData(capture.tap, &address, 0, nil, UInt32(MemoryLayout.size(ofValue: reference)), &reference), "Refresh application capture and exclusions")
        }, flush: { taps.flatMap(\.rings).forEach(rl_ring_request_reset) }, start: { try $0.start() })
        for capture in taps { try cancellation.check(); try setTapMute(capture, behavior: .mutedWhenTapped) }
        try cancellation.check()
    }
    private func setTapMute(_ capture: CaptureNode, behavior: CATapMuteBehavior) throws {
        guard let description = capture.tapDescription else { return }
        description.muteBehavior = behavior
        var reference = Unmanaged.passUnretained(description)
        var address = Hardware.address(kAudioTapPropertyDescription)
        try checked(AudioObjectSetPropertyData(capture.tap, &address, 0, nil, UInt32(MemoryLayout.size(ofValue: reference)), &reference), "Preserve application monitoring during routing")
    }
}

@MainActor public final class AudioCoordinator: ObservableObject {
    @Published public private(set) var devices: [AudioDeviceInfo] = []
    @Published public private(set) var applications: [AudioApplicationInfo] = []
    @Published public private(set) var running = false
    @Published public private(set) var starting = false
    @Published public private(set) var errorMessage: String?
    @Published public private(set) var channelMeters: [UUID: Float] = [:]
    @Published public private(set) var busMeters: [UUID: Float] = [:]
    @Published public private(set) var diagnostics = AudioDiagnostics()
    @Published public private(set) var soundCheck: SoundCheckSnapshot?
    public private(set) var inventoryReady = false
    private var graph: AudioGraph?
    private var routing: ExpandedRouting?
    private var timer: Timer?
    private var ticks = 0
    private let worker = DispatchQueue(label: "au.textual.ravelink.audio-control", qos: .userInitiated)
    private var generation: UInt64 = 0
    private var refreshing = false
    private var workerTimedOut = false
    private var pendingStart: GraphCancellation?
    private var pendingSoundCheckChannelID: UUID?
    private var pendingExcludedApplications: [String] = []
    private var progressMonitor = RenderProgressMonitor()
    private var captureProgressMonitor = DeviceCaptureProgressMonitor()
    private var healthCheckID: UUID?
    private var healthCheckRequested = false
    private var healthCancellation: GraphCancellation?
    private var powerObserver: AudioPowerObserver?

    public init() {
        powerObserver = AudioPowerObserver { [weak self] event in
            Task { @MainActor [weak self] in
                guard let self else { return }
                switch event {
                case .sleep:
                    let active = self.running || self.starting
                    self.stop()
                    if active { self.errorMessage = "Audio stopped because this Mac is sleeping. Start audio again after it wakes." }
                case .wake: self.refreshDevices()
                }
            }
        }
        refreshDevices()
        timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.poll() }
        }
    }
    deinit { timer?.invalidate(); if let graph { worker.async { graph.stop() } } }
    public func refreshDevices() {
        guard !refreshing, !workerTimedOut else { return }
        refreshing = true
        worker.async { [weak self] in
            let devices = Hardware.devices().map(\.1).sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
            let apps = Hardware.applications()
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.devices = devices; self.applications = apps; self.refreshing = false; self.inventoryReady = true
            }
        }
    }
    public func start(session: MixerSession) throws {
        try start(session: session, soundCheckChannelID: nil)
    }
    public func startSoundCheck(session: MixerSession, channelID: UUID, excludedApplications: [String] = []) throws {
        try validateSoundCheckSession(session, channelID: channelID)
        try start(session: session, soundCheckChannelID: channelID, excludedApplications: validatedExcludedApplications(excludedApplications))
    }
    public func soundCheckCommand(_ command: SoundCheckCommand) throws {
        guard running, let check = graph?.outputs.first?.soundCheck else {
            throw AudioRoutingError.configuration("Start Sound Check before controlling its clip.")
        }
        try check.command(command)
    }
    private func start(session: MixerSession, soundCheckChannelID: UUID?, excludedApplications: [String] = []) throws {
        guard !workerTimedOut else { throw AudioRoutingError.configuration("The audio driver stopped responding. Quit and reopen RaveLink before starting another route.") }
        stop()
        do {
            if let soundCheckChannelID { try validateSoundCheckSession(session, channelID: soundCheckChannelID) }
            let expanded = try session.expandedRouting()
            let routed = expanded.session
            try validateFeedback(routed)
            guard !routed.channels.isEmpty else { throw AudioRoutingError.configuration("Add an input channel before starting audio.") }
            guard routed.buses.contains(where: { $0.deviceUID != nil }) else { throw AudioRoutingError.configuration("Choose an output device on at least one mix bus before starting audio.") }
            let virtualIDs = Set(devices.filter(\.isVirtual).map(\.id))
            if routed.channels.contains(where: { $0.source.kind == .device && !virtualIDs.contains($0.source.deviceUID ?? "") }) {
                switch AVCaptureDevice.authorizationStatus(for: .audio) {
                case .denied, .restricted:
                    throw AudioRoutingError.configuration("Allow microphone access for RaveLink in System Settings → Privacy & Security → Microphone, then start audio again.")
                case .notDetermined:
                    AVCaptureDevice.requestAccess(for: .audio) { _ in }
                    throw AudioRoutingError.configuration("Microphone access has been requested. Allow it in the macOS prompt, then start audio again.")
                default: break
                }
            }
            starting = true; errorMessage = nil; pendingSoundCheckChannelID = soundCheckChannelID
            pendingExcludedApplications = excludedApplications
            let request = generation
            let controlQueue = worker
            let cancellation = GraphCancellation()
            pendingStart = cancellation
            worker.async { [weak self] in
                let candidate = AudioGraph(session: routed, channelParents: expanded.channelParents, soundCheckChannelID: soundCheckChannelID, excludedApplications: excludedApplications)
                do {
                    try candidate.configure(cancellation: cancellation)
                    try candidate.installNotifications { [weak self] event in
                        Task { @MainActor [weak self] in
                            guard let self, self.generation == request, self.graph != nil else { return }
                            if case .audioServiceRestart = event {
                                self.stop(); self.errorMessage = "The macOS audio service restarted. Start audio again to rebuild the devices and application captures."
                                self.refreshDevices()
                            } else if case .processMembership = event, self.graph?.applicationProcesses.isEmpty == true {
                                return
                            } else { self.checkDeviceHealth(suspend: true) }
                        }
                    }
                    Task { @MainActor [weak self] in
                        guard let self else { controlQueue.async { candidate.stop() }; return }
                        guard self.generation == request else { self.worker.async { candidate.stop() }; return }
                        self.worker.async { [weak self] in
                            do {
                                try cancellation.check()
                                try candidate.activateTaps(cancellation: cancellation)
                                Task { @MainActor [weak self] in
                                    guard let self else { controlQueue.async { candidate.stop() }; return }
                                    guard self.generation == request else { controlQueue.async { candidate.stop() }; return }
                                    do { try candidate.controls(routed) }
                                    catch {
                                        controlQueue.async { candidate.stop() }
                                        self.starting = false; self.pendingStart = nil; self.errorMessage = error.localizedDescription
                                        return
                                    }
                                    self.graph = candidate; self.routing = expanded; self.errorMessage = nil; self.diagnostics = .init()
                                    self.diagnostics.captureSourceCount = UInt32(candidate.captures.count)
                                    self.starting = false; self.pendingStart = nil; self.running = true
                                    self.soundCheck = candidate.outputs.first?.soundCheck?.snapshot()
                                }
                            } catch {
                                candidate.stop()
                                let message = error.localizedDescription
                                Task { @MainActor [weak self] in
                                    guard let self, self.generation == request else { return }
                                    self.starting = false; self.pendingStart = nil; self.errorMessage = message
                                }
                            }
                        }
                    }
                } catch {
                    candidate.stop()
                    let message = error.localizedDescription
                    Task { @MainActor [weak self] in
                        guard let self, self.generation == request else { return }
                        self.starting = false; self.pendingStart = nil; self.errorMessage = message
                    }
                }
            }
            // CoreAudio driver calls can block without cancellation. Keep the UI
            // responsive, invalidate late results, and require a fresh process.
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: .seconds(10))
                guard let self, self.generation == request, self.starting else { return }
                self.pendingStart?.cancel(); self.pendingStart = nil
                self.generation &+= 1; self.starting = false; self.workerTimedOut = true
                self.errorMessage = "The audio driver did not respond within 10 seconds. Quit and reopen RaveLink. If it persists, reconnect the device or restart macOS. System audio settings were not changed."
            }
        } catch { starting = false; errorMessage = error.localizedDescription; throw error }
    }
    public func update(session: MixerSession) throws {
        do {
            let expanded = try session.expandedRouting()
            let routed = expanded.session
            try validateFeedback(routed)
            guard let graph else {
                if starting { try start(session: session, soundCheckChannelID: pendingSoundCheckChannelID, excludedApplications: pendingExcludedApplications) }
                return
            }
            if AudioGraph.sameTopology(graph.session, routed) { try graph.controls(routed); routing = expanded }
            else if graph.soundCheckChannelID != nil {
                throw AudioRoutingError.configuration("Sound Check stopped and its temporary clip was discarded because the input, output, or effect rack changed. Start a new Sound Check to record again.")
            } else { try start(session: session) }
        } catch {
            stop(); errorMessage = error.localizedDescription; throw error
        }
    }
    public func stop() {
        progressMonitor.reset()
        captureProgressMonitor.reset()
        healthCancellation?.cancel(); healthCancellation = nil; healthCheckID = nil; healthCheckRequested = false
        pendingStart?.cancel(); pendingStart = nil; pendingSoundCheckChannelID = nil; pendingExcludedApplications = []
        generation &+= 1; starting = false
        if let graph {
            graph.silence()
            worker.async { [weak self] in
                if let message = graph.stop() {
                    Task { @MainActor [weak self] in self?.workerTimedOut = true; self?.errorMessage = message }
                }
            }
        }
        graph = nil; routing = nil; running = false; channelMeters = [:]; busMeters = [:]; soundCheck = nil
    }
    /// Used by the supervised helper to acknowledge stop only after HAL teardown
    /// finishes. If HAL is hung, the parent kills the helper on its hard deadline.
    public func finishPendingControlWork() async {
        await withCheckedContinuation { continuation in worker.async { continuation.resume() } }
    }
    private func validateFeedback(_ session: MixerSession) throws {
        let virtualIDs = Set(devices.filter(\.isVirtual).map(\.id))
        if hasVirtualFeedbackCycle(session, virtualDeviceUIDs: virtualIDs) {
            throw AudioRoutingError.configuration("The active routes form a feedback loop through virtual devices. Mute a send or choose another output to break the loop before starting audio.")
        }
    }
    private func poll() {
        ticks += 1
        if ticks % 40 == 0 { refreshDevices(); checkDeviceHealth() }
        guard let graph else { return }
        let now = Double(DispatchTime.now().uptimeNanoseconds) / 1_000_000_000
        let progress = graph.outputs.map { OutputProgress(id: $0.uid, frames: rl_render_frames($0.context), sampleRate: $0.rate) }
        if progressMonitor.observe(progress, at: now, maxFrames: AudioGraph.maxFrames) != nil {
            stop(); errorMessage = "An output device or effect stopped returning audio. Audio has stopped; check the device and effects, then start again."; return
        }
        let captureProgress = graph.captures.map {
            CaptureProgress(id: $0.channelID.uuidString, frames: rl_capture_frames($0.context), sampleRate: $0.rate, isProcessTap: $0.tap != 0)
        }
        if let stalled = captureProgressMonitor.observe(captureProgress, at: now, maxFrames: AudioGraph.maxFrames) {
            let name = graph.session.channels.first { $0.id.uuidString == stalled }?.name ?? "Device input"
            stop(); errorMessage = "Input “\(name)” stopped returning audio. Check or reconnect its device, then start audio again."; return
        }
        if let failure = graph.outputs.lazy.compactMap({ $0.audioUnits?.failureMessage }).first {
            stop(); errorMessage = failure; return
        }
        var channels: [UUID: Float] = [:], buses: [UUID: Float] = [:], stats = AudioDiagnostics()
        stats.captureSourceCount = UInt32(graph.captures.count)
        for capture in graph.captures {
            _ = rl_capture_take_peak(capture.context)
            stats.capturedFrames += rl_capture_frames(capture.context)
            stats.oversizedBlocks += rl_capture_oversized_blocks(capture.context)
            stats.timestampDiscontinuities += rl_capture_timestamp_discontinuities(capture.context)
            if rl_capture_clock_factor(capture.context) == 0 { stats.timestampFallbackNodes += 1 }
        }
        for output in graph.outputs {
            stats.effectAlignmentLatencySeconds = max(stats.effectAlignmentLatencySeconds, output.audioUnits?.alignmentLatencySeconds ?? 0)
            for (i, id) in output.buses.enumerated() { buses[id] = rl_render_take_bus_peak(output.context, UInt32(i)) }
            for (i, channel) in graph.session.channels.enumerated() {
                channels[channel.id] = max(channels[channel.id] ?? 0, rl_render_take_source_peak(output.context, UInt32(i)))
            }
            stats.renderedFrames += rl_render_frames(output.context)
            stats.oversizedBlocks += rl_render_oversized_blocks(output.context)
            stats.timestampDiscontinuities += rl_render_timestamp_discontinuities(output.context)
            if rl_render_clock_factor(output.context) == 0 { stats.timestampFallbackNodes += 1 }
        }
        for ring in graph.rings {
            stats.droppedFrames += rl_ring_dropped(ring); stats.underrunFrames += rl_ring_underruns(ring)
            if rl_ring_clock_mismatch(ring) { stats.clockMismatchPairs += 1 }
        }
        channelMeters = routing?.aggregateChannelPeaks(channels) ?? channels
        busMeters = routing?.aggregateBusPeaks(buses) ?? buses
        diagnostics = stats
        soundCheck = graph.outputs.first?.soundCheck?.snapshot()
        if stats.clockMismatchPairs > 0 {
            stop(); errorMessage = "The input and output hardware clocks differ beyond the supported correction range. Check their clock configuration or choose different devices, then start again."; return
        }
        if stats.timestampDiscontinuities > 0 {
            stop(); errorMessage = "An audio device's sample timeline skipped or became invalid. Audio stopped to avoid mixing misaligned data. Check the device and system load, then start again."; return
        }
        if stats.oversizedBlocks > 0 {
            stop(); errorMessage = "An input or output device requested an unsupported audio buffer. \(RealtimeDeviceLimits.bufferGuidance)"
        }
    }
    private func checkDeviceHealth(suspend: Bool = false) {
        guard let graph else { return }
        if suspend { graph.suspend(true) }
        guard healthCheckID == nil else { healthCheckRequested = true; return }
        let request = generation, checkID = UUID(), cancellation = GraphCancellation()
        healthCheckID = checkID; healthCancellation = cancellation
        // Snapshot mutable actor state; worker accesses only immutable graph nodes.
        let session = graph.session, memberships = graph.applicationProcesses
        let needed = session.buses.compactMap(\.deviceUID) + session.channels.filter { $0.source.kind == .device }.compactMap { $0.source.deviceUID }
        let captures = graph.captures.map { ($0.device, $0.rate, $0.buffer) }
        let outputs = graph.outputs.map { ($0.device, $0.rate, $0.buffer) }
        worker.async { [weak self] in
            do {
                try cancellation.check()
                let available = Dictionary(uniqueKeysWithValues: Hardware.devices().map { ($0.1.id, $0.1) })
                guard !needed.contains(where: { available[$0] == nil }) else {
                    throw AudioRoutingError.configuration("An active audio device was disconnected. Reconnect it or choose another device, then start audio.")
                }
                for (nodes, scope) in [(captures, kAudioObjectPropertyScopeInput), (outputs, kAudioObjectPropertyScopeOutput)] {
                    for (device, rate, buffer) in nodes {
                        try cancellation.check(); try Hardware.validateAlive(device)
                        guard try Hardware.rate(device) == rate else { throw AudioRoutingError.configuration("An audio device changed sample rate. Start audio again to rebuild its route.") }
                        try Hardware.validateFloatStreams(device, scope: scope)
                        guard try Hardware.buffer(device, rate: rate) == buffer else {
                            throw AudioRoutingError.configuration("An audio device changed its buffer size. Start audio again to rebuild its route safely.")
                        }
                    }
                }
                for channel in session.channels where channel.source.kind == .device {
                    guard let uid = channel.source.deviceUID, let info = available[uid], channel.source.channelOffset < info.inputChannels else {
                        throw AudioRoutingError.configuration("An input device changed its available channels. Choose a valid input and start audio again.")
                    }
                }
                for bus in session.buses {
                    if let uid = bus.deviceUID, let info = available[uid], bus.channelOffset + 1 >= info.outputChannels {
                        throw AudioRoutingError.configuration("An output device changed its available channels. Choose a valid stereo pair and start audio again.")
                    }
                }
                let processes = memberships.isEmpty ? [] : Hardware.processes()
                let current = applicationMemberships(bundleIDs: Set(memberships.keys), processes: processes, excludingPID: getpid())
                try cancellation.check()
                Task { @MainActor [weak self] in
                    guard let self, self.generation == request, self.healthCheckID == checkID else { return }
                    if current == memberships { self.finishHealthCheck(checkID: checkID, graph: graph); return }
                    graph.suspend(true)
                    self.worker.async { [weak self] in
                        do {
                            try graph.retargetTaps(processes: processes, dedicated: Set(memberships.keys), cancellation: cancellation)
                            Task { @MainActor [weak self] in
                                guard let self, self.generation == request, self.healthCheckID == checkID else { return }
                                graph.applicationProcesses = current
                                self.finishHealthCheck(checkID: checkID, graph: graph)
                            }
                        } catch {
                            let message = error.localizedDescription
                            Task { @MainActor [weak self] in
                                guard let self, self.generation == request, self.healthCheckID == checkID else { return }
                                self.stop(); self.errorMessage = "Application routing could not refresh safely: \(message) Start audio again."
                            }
                        }
                    }
                }
            } catch {
                let message = error.localizedDescription
                Task { @MainActor [weak self] in
                    guard let self, self.generation == request, self.healthCheckID == checkID else { return }
                    self.stop(); self.errorMessage = message
                }
            }
        }
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(3))
            guard let self, self.generation == request, self.healthCheckID == checkID else { return }
            self.stop(); self.workerTimedOut = true
            self.errorMessage = "The audio driver stopped responding while checking or refreshing a route. Quit and reopen RaveLink before starting audio again."
        }
    }
    private func finishHealthCheck(checkID: UUID, graph: AudioGraph) {
        guard healthCheckID == checkID else { return }
        healthCheckID = nil; healthCancellation = nil
        if healthCheckRequested {
            healthCheckRequested = false; checkDeviceHealth(suspend: true)
        } else { graph.suspend(false) }
    }
}
