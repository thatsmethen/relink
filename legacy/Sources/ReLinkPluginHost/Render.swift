import Foundation
import AudioToolbox
import AVFAudio

struct ParameterSetting: Codable {
    let address: UInt64
    let value: Float
}
struct UnitState: Codable {
    let formatVersion: Int
    let component: ComponentID
    let componentVersion: UInt32?
    let propertyList: Data
}
private func renderError(_ text: String) -> NSError {
    NSError(domain: "RaveLinkPluginHost.Render", code: 1, userInfo: [NSLocalizedDescriptionKey: text])
}
private func stateObject(_ state: UnitState) -> [String: Any] {
    ["formatVersion": state.formatVersion, "component": identity(state.component.description),
     "componentVersion": state.componentVersion as Any? ?? NSNull(), "propertyList": state.propertyList.base64EncodedString()]
}
private func instantiateForRender(_ id: ComponentID) throws -> AVAudioUnit {
    let completion = InstantiationResult()
    AVAudioUnit.instantiate(with: id.description, options: .loadOutOfProcess) { unit, error in
        if let unit { completion.set(.success(unit)) }
        else { completion.set(.failure(error ?? renderError("Audio Unit instantiation returned no instance."))) }
    }
    let started = DispatchTime.now().uptimeNanoseconds
    while completion.get() == nil {
        guard DispatchTime.now().uptimeNanoseconds - started < 15_000_000_000 else { throw renderError("Audio Unit instantiation timed out.") }
        _ = RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.02))
    }
    return try completion.get()!.get()
}

func render(_ request: Request) throws -> Never {
    guard let id = request.component, [kAudioUnitType_Effect, kAudioUnitType_MusicEffect].contains(id.type),
          id.subtype != 0, id.manufacturer != 0,
          let rate = request.sampleRate, rate.isFinite, (8_000...192_000).contains(rate),
          let channels = request.channels, (1...2).contains(channels), let samples = request.samples,
          !samples.isEmpty, samples.count % Int(channels) == 0, samples.count / Int(channels) <= 16_384,
          samples.allSatisfy({ $0.isFinite && abs($0) <= 16 }),
          let blockFrames = request.blockFrames, (1...512).contains(blockFrames),
          let bypassed = request.bypassed else { throw renderError("Invalid bounded render request.") }
    let settings = request.parameters ?? []
    guard settings.count <= 256, Set(settings.map(\.address)).count == settings.count,
          settings.allSatisfy({ $0.value.isFinite }) else { throw renderError("Invalid or repeated parameter assignments.") }
    var description = id.description
    guard let component = AudioComponentFindNext(nil, &description) else { throw renderError("The selected Audio Unit is no longer registered.") }
    let descriptor = try metadata(component)
    let currentVersion = descriptor["version"] as? UInt32
    guard request.expectedVersion == nil || request.expectedVersion == currentVersion else { throw renderError("Audio Unit version changed; refresh the catalog.") }
    var restored: [String: Any]?
    if let state = request.state {
        guard state.formatVersion == 1, state.component == id, !state.propertyList.isEmpty,
              state.propertyList.count <= 256 * 1024,
              state.componentVersion == nil || state.componentVersion == currentVersion else { throw renderError("Saved state does not match this Audio Unit or exceeds the state limit.") }
        guard let object = try PropertyListSerialization.propertyList(from: state.propertyList, options: [], format: nil) as? [String: Any] else {
            throw renderError("Saved state must contain a property-list dictionary.")
        }
        restored = object
    }
    let wrapper = try instantiateForRender(id), unit = wrapper.auAudioUnit
    if let restored { unit.fullState = restored }
    guard unit.inputBusses.count > 0, unit.outputBusses.count > 0,
          unit.inputBusses.count <= 64, unit.outputBusses.count <= 64,
          let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: channels) else {
        throw renderError("The Audio Unit requires an unsupported bus arrangement.")
    }
    unit.inputBusses[0].isEnabled = true
    unit.outputBusses[0].isEnabled = true
    unit.maximumFramesToRender = 512
    unit.isRenderingOffline = true
    try unit.inputBusses[0].setFormat(format)
    try unit.outputBusses[0].setFormat(format)
    guard [unit.inputBusses[0].format, unit.outputBusses[0].format].allSatisfy({
        $0.sampleRate == rate && $0.channelCount == channels && $0.commonFormat == .pcmFormatFloat32 && !$0.isInterleaved
    }) else { throw renderError("The Audio Unit did not retain the requested bus format.") }
    for assignment in settings {
        guard let parameter = unit.parameterTree?.parameter(withAddress: assignment.address),
              parameter.flags.contains(.flag_IsWritable), parameter.minValue.isFinite, parameter.maxValue.isFinite, parameter.minValue <= parameter.maxValue,
              (parameter.minValue...parameter.maxValue).contains(assignment.value) else { throw renderError("A parameter is unavailable, read-only, or outside its allowed range.") }
        parameter.value = assignment.value
    }
    let renderBlock = unit.renderBlock
    try unit.allocateRenderResources()
    defer { unit.deallocateRenderResources() }
    unit.reset()
    let frameCount = samples.count / Int(channels)
    var rendered = [Float](repeating: 0, count: samples.count)
    if bypassed {
        // Host bypass is deterministic and does not rely on optional vendor bypass behavior.
        rendered = samples
    } else {
        guard let input = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512),
              let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512),
              let inputPointers = input.floatChannelData, let outputPointers = output.floatChannelData else { throw renderError("Could not allocate render buffers.") }
        let ownedOutputPointers = (0..<Int(channels)).map { outputPointers[$0] }
        var position = 0
        while position < frameCount {
            let frames = min(Int(blockFrames), frameCount - position)
            input.frameLength = AVAudioFrameCount(frames)
            output.frameLength = AVAudioFrameCount(frames)
            let outputBuffers = UnsafeMutableAudioBufferListPointer(output.mutableAudioBufferList)
            for channel in 0..<Int(channels) {
                outputBuffers[channel].mData = UnsafeMutableRawPointer(ownedOutputPointers[channel])
                outputBuffers[channel].mNumberChannels = 1
                outputBuffers[channel].mDataByteSize = UInt32(frames * MemoryLayout<Float>.size)
            }
            for channel in 0..<Int(channels) {
                for frame in 0..<frames { inputPointers[channel][frame] = samples[(position + frame) * Int(channels) + channel] }
            }
            var timestamp = AudioTimeStamp()
            timestamp.mSampleTime = Double(position)
            timestamp.mFlags = .sampleTimeValid
            var flags = AudioUnitRenderActionFlags()
            let pull: AURenderPullInputBlock = { _, _, requestedFrames, bus, data in
                guard bus == 0, requestedFrames == frames else { return kAudioUnitErr_FormatNotSupported }
                let destination = UnsafeMutableAudioBufferListPointer(data)
                guard destination.count == Int(channels) else { return kAudioUnitErr_FormatNotSupported }
                for channel in 0..<Int(channels) {
                    destination[channel].mNumberChannels = 1
                    destination[channel].mDataByteSize = UInt32(frames * MemoryLayout<Float>.size)
                    destination[channel].mData = UnsafeMutableRawPointer(inputPointers[channel])
                }
                return noErr
            }
            let status = renderBlock(&flags, &timestamp, AUAudioFrameCount(frames), 0, output.mutableAudioBufferList, pull)
            guard status == noErr else { throw renderError("Audio Unit render failed with status \(status).") }
            let buffers = UnsafeMutableAudioBufferListPointer(output.mutableAudioBufferList)
            guard buffers.count == Int(channels) else { throw renderError("Audio Unit returned an invalid channel layout.") }
            for channel in 0..<Int(channels) {
                guard buffers[channel].mNumberChannels == 1, buffers[channel].mDataByteSize >= frames * MemoryLayout<Float>.size,
                      let data = buffers[channel].mData?.assumingMemoryBound(to: Float.self) else { throw renderError("Audio Unit returned an invalid audio buffer.") }
                for frame in 0..<frames {
                    let value = flags.contains(.unitRenderAction_OutputIsSilence) ? 0 : data[frame]
                    guard value.isFinite, abs(value) <= 1_000_000 else { throw renderError("Audio Unit output contained invalid or excessive samples.") }
                    rendered[(position + frame) * Int(channels) + channel] = value
                }
            }
            position += frames
        }
    }
    var savedState: Any = NSNull()
    if let state = unit.fullState {
        let data = try PropertyListSerialization.data(fromPropertyList: state, format: .binary, options: 0)
        guard data.count <= 256 * 1024 else { throw renderError("Audio Unit state exceeds the 256 KiB persistence limit.") }
        savedState = stateObject(.init(formatVersion: 1, component: id, componentVersion: currentVersion, propertyList: data))
    }
    let allParameters = unit.parameterTree?.allParameters ?? []
    guard allParameters.count <= 4096 else { throw renderError("Audio Unit parameter count exceeds the report limit.") }
    let parameterValues = allParameters.map { ["address": $0.address, "value": finite(Double($0.value))] as [String: Any] }
    let reportedLatency = finite(unit.latency)
    unit.deallocateRenderResources()
    emit(["protocolVersion": 1, "operation": "render", "status": "ok", "render": [
        "descriptor": descriptor, "sampleRate": rate, "channels": channels, "frameCount": frameCount,
        "blockFrames": blockFrames, "bypassed": bypassed, "samples": rendered, "state": savedState,
        "parameterValues": parameterValues, "latencySeconds": reportedLatency,
        "limitations": ["Bounded offline main-bus render only. No live engine or hardware integration.",
                        "Each request starts a new instance. No streaming continuity, tail flush, automation, or latency compensation.",
                        "Bypass copies the input exactly at the host boundary; it does not exercise vendor bypass behavior."]]])
}
