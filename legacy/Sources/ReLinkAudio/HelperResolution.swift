import Foundation

/// Bundle selection, not code-signature authentication. The installed app must
/// never recover a missing helper by searching its surrounding directories.
enum HelperResolution {
    enum Executable: CaseIterable {
        case engine, pluginHost

        var name: String { self == .engine ? "RaveLinkEngine" : "RaveLinkPluginHost" }
        var embeddedPath: String {
            self == .engine ? "Contents/MacOS/RaveLinkEngine" : "Contents/Helpers/RaveLinkPluginHost"
        }
    }

    struct Location {
        let bundleURL: URL
        let executableURL: URL?

        init(_ bundle: Bundle) { bundleURL = bundle.bundleURL; executableURL = bundle.executableURL }
        init(bundleURL: URL, executableURL: URL? = nil) {
            self.bundleURL = bundleURL; self.executableURL = executableURL
        }
    }

    static func resolve(_ helper: Executable, locations: [Location], command: URL?) -> URL? {
        // A main bundle or executable recognized as a packaged application is
        // authoritative even when its expected helper is absent or damaged.
        for location in locations {
            if location.bundleURL.pathExtension.lowercased() == "app" {
                return embedded(helper, in: location.bundleURL)
            }
            if let executable = location.executableURL, let app = application(for: executable) {
                return embedded(helper, in: app)
            }
        }
        if let command, let app = application(for: command) { return embedded(helper, in: app) }

        // SwiftPM runners may live inside an xctest bundle while Bundle.main
        // describes a toolchain runner. These fallbacks apply only outside apps.
        var candidates: [URL] = []
        for location in locations {
            candidates.append(location.bundleURL.appendingPathComponent(helper.embeddedPath))
            if helper == .engine {
                candidates.append(location.bundleURL.appendingPathComponent("Contents/Helpers/\(helper.name)"))
            }
            if let executable = location.executableURL {
                candidates.append(executable.deletingLastPathComponent().appendingPathComponent(helper.name))
            }
            if location.bundleURL.pathExtension == "xctest" {
                candidates.append(location.bundleURL.deletingLastPathComponent().appendingPathComponent(helper.name))
            }
        }
        if let command {
            var ancestor = command.standardizedFileURL.deletingLastPathComponent()
            candidates.append(ancestor.appendingPathComponent(helper.name))
            if helper == .engine {
                for _ in 0..<4 { ancestor.deleteLastPathComponent(); candidates.append(ancestor.appendingPathComponent(helper.name)) }
            } else {
                for _ in 0..<3 {
                    if ancestor.pathExtension == "xctest" {
                        candidates.append(ancestor.deletingLastPathComponent().appendingPathComponent(helper.name)); break
                    }
                    ancestor.deleteLastPathComponent()
                }
            }
        }
        return candidates.first(where: isExecutableFile)
    }

    private static func application(for executable: URL) -> URL? {
        let directory = executable.standardizedFileURL.deletingLastPathComponent()
        guard directory.lastPathComponent == "MacOS",
              directory.deletingLastPathComponent().lastPathComponent == "Contents" else { return nil }
        let app = directory.deletingLastPathComponent().deletingLastPathComponent()
        return app.pathExtension.lowercased() == "app" ? app : nil
    }

    private static func embedded(_ helper: Executable, in application: URL) -> URL? {
        // Canonicalize the app root so aliases of the whole app remain usable,
        // then reject helper/directory symlinks, including escapes from the app.
        let root = application.resolvingSymlinksInPath().standardizedFileURL
        let candidate = root.appendingPathComponent(helper.embeddedPath)
        guard candidate.resolvingSymlinksInPath().standardizedFileURL.path == candidate.path,
              isExecutableFile(candidate) else { return nil }
        return candidate
    }

    private static func isExecutableFile(_ url: URL) -> Bool {
        guard let values = try? url.resourceValues(forKeys: [.isRegularFileKey]), values.isRegularFile == true else { return false }
        return FileManager.default.isExecutableFile(atPath: url.path)
    }
}
