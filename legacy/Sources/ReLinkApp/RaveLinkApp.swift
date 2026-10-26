import SwiftUI
import AppKit

@main struct RaveLinkApplication: App {
    @StateObject private var studio = StudioModel()
    @NSApplicationDelegateAdaptor(AppDelegate.self) var delegate
    var body: some Scene {
        WindowGroup {
            StudioView(studio: studio, audio: studio.audio)
                .preferredColorScheme(.dark)
                .frame(minWidth: 900, minHeight: 560)
                .onReceive(NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)) { _ in studio.flush(); studio.audio.shutdown() }
        }
        .defaultSize(width: 1250, height: 780)
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Add Input…") { studio.beginInput() }.keyboardShortcut("n")
                Button("Add Mix…") { studio.showingMix = true }.keyboardShortcut("n", modifiers: [.command, .shift])
                Divider()
                Button("Import Studio…") { studio.importSession() }.keyboardShortcut("o")
                Button("Export Studio…") { studio.exportSession() }.keyboardShortcut("s", modifiers: [.command, .shift])
            }
            CommandMenu("Audio") {
                Button(studio.audio.running ? "Stop Audio" : "Start Audio") { studio.toggleAudio() }.keyboardShortcut("r")
                Button("Refresh Devices") { studio.audio.refreshDevices() }
            }
        }
    }
}
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) { NSApp.setActivationPolicy(.regular); NSApp.activate(ignoringOtherApps: true) }
}
