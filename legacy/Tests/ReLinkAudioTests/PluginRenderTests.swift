import Foundation
import Testing
@testable import RaveLinkAudio

@Suite struct PluginRenderTests {
    private var appleID: AudioUnitComponentID { .init(type: 0x61756678, subtype: 0x6c6d7472, manufacturer: 0x6170706c) }
    private var descriptor: AudioUnitDescriptor { .init(component: appleID, name: "Apple: AUPeakLimiter", version: 67072, flags: 2) }

    @Test func invalidInputIsRejectedBeforeLaunchingAHelper() async throws {
        let catalog = PluginCatalog(helperURL: nil)
        for samples: [Float] in [[], [0], [.nan, 0], [.infinity, 0], [17, 0], Array(repeating: 0, count: 32_770)] {
            do { _ = try await catalog.render(descriptor, samples: samples); Issue.record("Expected input rejection") }
            catch { #expect(error as? PluginCatalogError == .invalidRequest) }
        }
        do {
            _ = try await catalog.render(descriptor, samples: [0, 0], parameters: [.init(address: 2, value: 1), .init(address: 2, value: 2)])
            Issue.record("Expected duplicate parameter rejection")
        } catch { #expect(error as? PluginCatalogError == .invalidRequest) }
        do {
            _ = try await catalog.render(descriptor, samples: [0, 0], blockFrames: 513)
            Issue.record("Expected block size rejection")
        } catch { #expect(error as? PluginCatalogError == .invalidRequest) }
    }
    @Test func savedStateIsBoundToIdentityVersionAndSize() async throws {
        let state = AudioUnitState(component: appleID, componentVersion: 67072, propertyList: Data([1]))
        #expect(state.isValid(for: descriptor))
        let encoded = try JSONEncoder().encode(state)
        #expect(try JSONDecoder().decode(AudioUnitState.self, from: encoded) == state)
        let mismatch = AudioUnitState(component: appleID, componentVersion: 1, propertyList: Data([1]))
        #expect(!mismatch.isValid(for: descriptor))
        let large = AudioUnitState(component: appleID, componentVersion: 67072, propertyList: Data(repeating: 0, count: 256 * 1024 + 1))
        #expect(!large.isValid(for: descriptor))
        do { _ = try await PluginCatalog(helperURL: nil).render(descriptor, samples: [0, 0], state: mismatch); Issue.record("Expected state rejection") }
        catch { #expect(error as? PluginCatalogError == .invalidRequest) }
    }
    @Test func malformedRenderReportsCannotMasqueradeAsValidAudio() throws {
        var report: [String: Any] = ["descriptor": ["component": ["type": appleID.type, "subtype": appleID.subtype, "manufacturer": appleID.manufacturer], "name": descriptor.name, "version": 67072, "flags": 2],
                                    "sampleRate": 48000, "channels": 2, "frameCount": 1, "blockFrames": 512,
                                    "bypassed": false, "samples": [0.0, 0.0], "parameterValues": [], "limitations": []]
        func parse(_ report: [String: Any]) throws -> PluginResponse {
            try PluginResponse.parse(JSONSerialization.data(withJSONObject: ["protocolVersion": 1, "operation": "render", "status": "ok", "render": report]), operation: "render")
        }
        #expect(try parse(report).render?.frameCount == 1)
        report["frameCount"] = 2
        #expect(throws: PluginCatalogError.malformedResponse) { try parse(report) }
        report["frameCount"] = 1
        report["samples"] = [0.0, 1_000_001.0]
        #expect(throws: PluginCatalogError.malformedResponse) { try parse(report) }
        report["samples"] = [0.0, 0.0]
        report["state"] = ["formatVersion": 1, "component": ["type": 1, "subtype": 1, "manufacturer": 1], "propertyList": "AQ=="]
        #expect(throws: PluginCatalogError.malformedResponse) { try parse(report) }
    }
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RAVELINK_PLUGIN_RENDER_APPLE"] == "1"))
    func appleLimiterRendersIndependentGainReferenceAndRestoresState() async throws {
        let helper = try #require(PluginCatalog.defaultHelperURL())
        let catalog = PluginCatalog(helperURL: helper)
        let selected = try #require(try await catalog.discover().first { $0.component == appleID })
        var samples: [Float] = []
        samples.reserveCapacity(8192 * 2)
        let leftPhaseStep: Double = 2.0 * Double.pi * 317.0 / 48_000.0
        let rightPhaseStep: Double = 2.0 * Double.pi * 719.0 / 48_000.0
        for frame in 0..<8192 {
            let phase = Double(frame)
            samples.append(Float(0.01 * sin(phase * leftPhaseStep)))
            samples.append(Float(0.015 * sin(phase * rightPhaseStep)))
        }
        let rendered = try await catalog.render(selected, samples: samples, parameters: [.init(address: 2, value: 20)], blockFrames: 257)
        let delay = Int((try #require(rendered.latencySeconds) * 48000).rounded())
        #expect((0...512).contains(delay))
        // The two independent input tones remain well below the limiter threshold
        // after a 20 dB pre-gain, so expected output is exactly 10x, delayed.
        var maximumError: Float = 0
        for frame in 1024..<8192 {
            for channel in 0..<2 {
                let expected: Float = samples[(frame - delay) * 2 + channel] * 10
                maximumError = max(maximumError, abs(rendered.samples[frame * 2 + channel] - expected))
            }
        }
        #expect(maximumError < 0.000_001)
        #expect(rendered.samples != samples)
        let state = try #require(rendered.state)
        let restored = try await catalog.render(selected, samples: samples, state: state, blockFrames: 257)
        #expect(restored.parameterValues.first { $0.address == 2 }?.value == 20)
        #expect(zip(rendered.samples, restored.samples).allSatisfy { abs($0 - $1) < 0.000_001 })
        let bypass = try await catalog.render(selected, samples: samples, state: state, bypassed: true)
        #expect(bypass.samples == samples)
        #expect(bypass.state != nil)
        do {
            _ = try await catalog.render(selected, samples: [0, 0], parameters: [.init(address: 2, value: 100)])
            Issue.record("Expected Audio Unit parameter range rejection")
        } catch { if case .helperError(let code, _) = error as? PluginCatalogError { #expect(code == "render_failed") } else { Issue.record("Unexpected error: \(error)") } }
    }
}
