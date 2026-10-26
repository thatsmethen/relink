import SwiftUI
import RaveLinkAudio

struct EngineDiagnosticsView: View {
    @ObservedObject var audio: EngineClient
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Engine diagnostics").font(.title2.weight(.semibold))
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    row("State", audio.starting ? "Connecting" : (audio.running ? "Running" : "Stopped"))
                    Divider()
                    row("Captured frames", String(audio.diagnostics.capturedFrames))
                    row("Rendered frames", String(audio.diagnostics.renderedFrames))
                    row("Input overflow frames", String(audio.diagnostics.droppedFrames))
                    row("Buffer underrun frames", String(audio.diagnostics.underrunFrames))
                    row("Oversized device blocks", String(audio.diagnostics.oversizedBlocks))
                    row("Device timeline interruptions", String(audio.diagnostics.timestampDiscontinuities))
                    row("Clocks awaiting timestamps", String(audio.diagnostics.timestampFallbackNodes))
                    row("Clock pairs outside correction range", String(audio.diagnostics.clockMismatchPairs))
                    row("AU alignment delay", String(format: "%.2f ms", audio.diagnostics.effectAlignmentLatencySeconds * 1000))
                    Text("AU alignment uses the effects’ reported delay. It excludes device and capture latency.").font(.caption).foregroundStyle(.secondary)
                    Text("Clocks may await timestamps during startup or while an app is idle. Without usable timestamps, drift correction follows the audio buffer instead.").font(.caption).foregroundStyle(.secondary)
                    Text("Counters combine active paths in this run. Increasing overflow or underrun counts can explain clicks or gaps. Reduce active sources or align device sample rates, then start again.").font(.caption).foregroundStyle(.secondary)
                    if let error = audio.errorMessage { Text(error).font(.callout).foregroundStyle(.orange).textSelection(.enabled) }
                }
            }.frame(maxHeight: 480)
            HStack { Spacer(); Button("Done") { dismiss() }.keyboardShortcut(.defaultAction).buttonStyle(AccentButtonStyle()) }
        }.padding(28).frame(width: 500)
    }
    private func row(_ title: String, _ value: String) -> some View {
        LabeledContent(title, value: value)
            .accessibilityElement(children: .ignore).accessibilityLabel(title).accessibilityValue(value)
    }
}
