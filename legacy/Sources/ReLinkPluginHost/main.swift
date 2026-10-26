import Foundation
import AudioToolbox
import AVFAudio
import Darwin

// This executable is intentionally disposable. It never starts a hardware device.
// Keep protocol output separate from any third-party plug-in logging to stdout.
let responseHandle = FileHandle(fileDescriptor: dup(STDOUT_FILENO), closeOnDealloc: true)
_ = dup2(STDERR_FILENO, STDOUT_FILENO)

struct ComponentID: Codable, Equatable {
    let type: UInt32
    let subtype: UInt32
    let manufacturer: UInt32
    var description: AudioComponentDescription {
        .init(componentType: type, componentSubType: subtype, componentManufacturer: manufacturer, componentFlags: 0, componentFlagsMask: 0)
    }
}
struct Request: Decodable {
    let protocolVersion: Int
    let operation: String
    let component: ComponentID?
    let expectedVersion: UInt32?
    let sampleRate: Double?
    let channels: UInt32?
    let samples: [Float]?
    let parameters: [ParameterSetting]?
    let state: UnitState?
    let bypassed: Bool?
    let blockFrames: UInt32?
}

func emit(_ value: [String: Any], exitCode: Int32 = 0) -> Never {
    do {
        var bytes = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
        bytes.append(0x0a)
        try responseHandle.write(contentsOf: bytes)
    } catch {
        let fallback = Data("{\"protocolVersion\":1,\"operation\":\"unknown\",\"status\":\"error\",\"error\":{\"code\":\"encoding_failed\",\"message\":\"Could not encode helper response.\"}}\n".utf8)
        try? responseHandle.write(contentsOf: fallback)
        exit(70)
    }
    exit(exitCode)
}
func failure(_ operation: String, _ code: String, _ message: String) -> Never {
    emit(["protocolVersion": 1, "operation": operation, "status": "error", "error": ["code": code, "message": String(message.prefix(2048))]], exitCode: 2)
}
func bounded(_ text: String) -> String { String(text.prefix(512)) }
func identity(_ description: AudioComponentDescription) -> [String: Any] {
    ["type": description.componentType, "subtype": description.componentSubType, "manufacturer": description.componentManufacturer]
}
func metadata(_ component: AudioComponent) throws -> [String: Any] {
    var desc = AudioComponentDescription()
    let status = AudioComponentGetDescription(component, &desc)
    guard status == noErr else { throw NSError(domain: NSOSStatusErrorDomain, code: Int(status)) }
    var name: Unmanaged<CFString>?
    let nameStatus = AudioComponentCopyName(component, &name)
    var version: UInt32 = 0
    let versionStatus = AudioComponentGetVersion(component, &version)
    return ["component": identity(desc), "name": nameStatus == noErr ? bounded((name?.takeRetainedValue() as String?) ?? "Unnamed Audio Unit") : "Unnamed Audio Unit",
            "version": versionStatus == noErr ? version as Any : NSNull(), "flags": desc.componentFlags]
}
func discover() throws -> Never {
    var rows: [[String: Any]] = []
    for type in [kAudioUnitType_Effect, kAudioUnitType_MusicEffect] {
        var search = AudioComponentDescription(componentType: type, componentSubType: 0, componentManufacturer: 0, componentFlags: 0, componentFlagsMask: 0)
        var current: AudioComponent?
        while let next = AudioComponentFindNext(current, &search) {
            guard rows.count < 4096 else { failure("list", "catalog_limit", "The registered effect catalog exceeds 4096 entries.") }
            rows.append(try metadata(next))
            current = next
        }
    }
    emit(["protocolVersion": 1, "operation": "list", "status": "ok", "components": rows,
          "limitations": ["Registry metadata only. No Audio Unit was instantiated or rendered."]])
}

func finite(_ value: Double) -> Any { value.isFinite ? value as Any : NSNull() }
func buses(_ array: AUAudioUnitBusArray) -> [[String: Any]] {
    (0..<array.count).map { index in
        let bus = array[index], format = bus.format
        return ["index": index, "name": bus.name.map(bounded) as Any? ?? NSNull(),
                "sampleRate": finite(format.sampleRate), "channels": format.channelCount,
                "interleaved": format.isInterleaved, "commonFormat": format.commonFormat.rawValue,
                "channelLayoutTag": format.channelLayout.map { $0.layoutTag as Any } ?? NSNull(), "enabled": bus.isEnabled]
    }
}
final class InstantiationResult: @unchecked Sendable {
    private let lock = NSLock()
    private var result: Result<AVAudioUnit, Error>?
    func set(_ value: Result<AVAudioUnit, Error>) { lock.lock(); result = value; lock.unlock() }
    func get() -> Result<AVAudioUnit, Error>? { lock.lock(); defer { lock.unlock() }; return result }
}
func probe(_ request: Request) throws -> Never {
    guard let id = request.component, [kAudioUnitType_Effect, kAudioUnitType_MusicEffect].contains(id.type),
          id.subtype != 0, id.manufacturer != 0, let rate = request.sampleRate, rate.isFinite,
          (8_000...384_000).contains(rate), let channels = request.channels, (1...2).contains(channels) else {
        failure("probe", "invalid_request", "Select one registered effect and a valid mono or stereo sample format.")
    }
    var description = id.description
    guard let component = AudioComponentFindNext(nil, &description) else { failure("probe", "component_missing", "The selected Audio Unit is no longer registered.") }
    let descriptor = try metadata(component)
    if let expected = request.expectedVersion, descriptor["version"] as? UInt32 != expected { failure("probe", "component_changed", "The Audio Unit version changed. Refresh the catalog before probing it.") }
    let completion = InstantiationResult()
    AVAudioUnit.instantiate(with: description, options: .loadOutOfProcess) { unit, error in
        if let unit { completion.set(.success(unit)) }
        else { completion.set(.failure(error ?? NSError(domain: "RaveLinkPluginHost", code: 1, userInfo: [NSLocalizedDescriptionKey: "Audio Unit instantiation returned no instance."]))) }
    }
    let start = DispatchTime.now().uptimeNanoseconds
    while completion.get() == nil {
        if DispatchTime.now().uptimeNanoseconds - start > 15_000_000_000 { failure("probe", "instantiation_timeout", "Audio Unit instantiation did not complete within 15 seconds.") }
        _ = RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.02))
    }
    let wrapper = try completion.get()!.get()
    let unit = wrapper.auAudioUnit
    guard unit.inputBusses.count <= 64, unit.outputBusses.count <= 64 else { failure("probe", "bus_limit", "The Audio Unit declares more than 64 input or output buses.") }
    let allParameters = unit.parameterTree?.allParameters ?? []
    guard allParameters.count <= 4096 else { failure("probe", "parameter_limit", "The Audio Unit declares more than 4096 parameters.") }
    let parameters: [[String: Any]] = allParameters.map { parameter in
        var declaredDefault: Any = NSNull()
        if parameter.address <= UInt64(UInt32.max) {
            var info = AudioUnitParameterInfo(), size = UInt32(MemoryLayout<AudioUnitParameterInfo>.size)
            if AudioUnitGetProperty(wrapper.audioUnit, kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, UInt32(parameter.address), &info, &size) == noErr {
                declaredDefault = finite(Double(info.defaultValue))
                if info.flags.rawValue & (1 << 4) != 0 { info.cfNameString?.release() }
            }
        }
        return ["address": parameter.address, "identifier": bounded(parameter.identifier), "name": bounded(parameter.displayName),
                "minimum": finite(Double(parameter.minValue)), "maximum": finite(Double(parameter.maxValue)),
                "initialValue": finite(Double(parameter.value)), "declaredDefaultValue": declaredDefault,
                "unit": parameter.unit.rawValue, "unitName": parameter.unitName.map(bounded) as Any? ?? NSNull(), "flags": parameter.flags.rawValue]
    }
    var accepted = false, allocated = false
    var configurationError: String?
    do {
        guard unit.inputBusses.count > 0, unit.outputBusses.count > 0,
              let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: channels) else {
            throw NSError(domain: "RaveLinkPluginHost", code: 2, userInfo: [NSLocalizedDescriptionKey: "A main input and output bus are required."])
        }
        unit.maximumFramesToRender = 512
        unit.isRenderingOffline = true
        try unit.inputBusses[0].setFormat(format)
        try unit.outputBusses[0].setFormat(format)
        guard [unit.inputBusses[0].format, unit.outputBusses[0].format].allSatisfy({
            $0.sampleRate == rate && $0.channelCount == channels && $0.commonFormat == .pcmFormatFloat32 && !$0.isInterleaved
        }) else {
            throw NSError(domain: "RaveLinkPluginHost", code: 3, userInfo: [NSLocalizedDescriptionKey: "The Audio Unit did not retain the requested main bus format."])
        }
        accepted = true
        try unit.allocateRenderResources()
        allocated = true
    } catch { configurationError = bounded(error.localizedDescription) }
    let report: [String: Any] = ["descriptor": descriptor, "inputBuses": buses(unit.inputBusses), "outputBuses": buses(unit.outputBusses),
                                "channelCapabilities": unit.channelCapabilities?.map(\.intValue) as Any? ?? NSNull(), "parameters": parameters,
                                "requestedSampleRate": rate, "requestedChannels": channels, "configurationAccepted": accepted,
                                "renderResourcesAllocated": allocated, "configurationError": configurationError as Any? ?? NSNull(),
                                "latencySeconds": finite(unit.latency), "tailSeconds": finite(unit.tailTime),
                                "limitations": ["Only main bus 0 input/output were configured as non-interleaved Float32 with a 512-frame maximum.",
                                                "Instantiation and allocation succeeded only where reported. No audio was rendered, no UI was opened, and no hardware device was started.",
                                                "Initial parameter values are observed instance state; declared defaults are null when the legacy parameter-info query is unsupported.",
                                                "Out-of-process instantiation is requested; AUv2 still loads inside this disposable helper. This is crash containment, not a security sandbox."]]
    unit.deallocateRenderResources()
    emit(["protocolVersion": 1, "operation": "probe", "status": "ok", "probe": report])
}

do {
    var requestBytes = Data()
    while let chunk = try FileHandle.standardInput.read(upToCount: 4096), !chunk.isEmpty {
        requestBytes.append(chunk)
        guard requestBytes.count <= 2 * 1024 * 1024 else { failure("unknown", "invalid_request", "Request is too large.") }
    }
    let request = try JSONDecoder().decode(Request.self, from: requestBytes)
    guard request.protocolVersion == 1 else { failure(request.operation, "unsupported_protocol", "Unsupported helper protocol version.") }
    if request.operation == "list" {
        do { try discover() }
        catch { failure("list", "discovery_failed", error.localizedDescription) }
    }
    if request.operation == "probe" {
        do { try probe(request) }
        catch { failure("probe", "probe_failed", error.localizedDescription) }
    }
    if request.operation == "render" {
        do { try render(request) }
        catch { failure("render", "render_failed", error.localizedDescription) }
    }
    failure(request.operation, "unsupported_operation", "The helper does not support this operation.")
} catch {
    failure("unknown", "invalid_request", error.localizedDescription)
}
