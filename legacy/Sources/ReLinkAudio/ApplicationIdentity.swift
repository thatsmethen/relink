import Foundation
import AppKit
import Darwin

struct AudioProcessIdentity: Sendable {
    let id: UInt32
    let bundle: String
    let pid: Int32
    let applicationBundleID: String
    let applicationName: String
    init(id: UInt32, bundle: String, pid: Int32, applicationBundleID: String? = nil, applicationName: String? = nil) {
        self.id = id; self.bundle = bundle; self.pid = pid
        self.applicationBundleID = applicationBundleID ?? bundle
        self.applicationName = applicationName ?? applicationBundleID ?? bundle
    }
    func belongs(to bundleID: String) -> Bool { applicationBundleID == bundleID || (!bundle.isEmpty && bundle == bundleID) }
}

struct ApplicationBundleIdentity: Equatable {
    let bundleID: String
    let name: String
}

enum ApplicationIdentityResolver {
    /// Outer applications precede embedded Chromium helper applications. Matching
    /// uses actual path components and bundle metadata, never identifier prefixes.
    static func bundleCandidates(for executable: URL) -> [URL] {
        var result: [URL] = [], current = executable.standardizedFileURL
        while current.path != "/" {
            if current.pathExtension.lowercased() == "app" { result.append(current) }
            current.deleteLastPathComponent()
        }
        return result.reversed()
    }
    static func resolve(executable: URL?, fallback: ApplicationBundleIdentity,
                        metadata: (URL) -> ApplicationBundleIdentity?) -> ApplicationBundleIdentity {
        guard let executable else { return fallback }
        for candidate in bundleCandidates(for: executable) {
            if let identity = metadata(candidate), !identity.bundleID.isEmpty { return identity }
        }
        return fallback
    }
    static func process(id: UInt32, bundleID: String, pid: Int32) -> AudioProcessIdentity {
        let app = NSRunningApplication(processIdentifier: pid)
        // libproc's PROC_PIDPATHINFO_MAXSIZE is (4 * MAXPATHLEN), a macro the
        // Swift importer cannot expose on the Command Line Tools SDK.
        var path = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
        let size = proc_pidpath(pid, &path, UInt32(path.count))
        let executable = size > 0 ? URL(fileURLWithPath: String(cString: path)).resolvingSymlinksInPath() : (app?.executableURL ?? app?.bundleURL)
        let fallbackID = bundleID.isEmpty ? (app?.bundleIdentifier ?? "") : bundleID
        let identity = resolve(executable: executable,
            fallback: .init(bundleID: fallbackID, name: app?.localizedName ?? fallbackID)) { url in
                guard let bundle = Bundle(url: url), let identifier = bundle.bundleIdentifier,
                      bundle.object(forInfoDictionaryKey: "CFBundlePackageType") as? String == "APPL" else { return nil }
                let name = bundle.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String
                    ?? bundle.object(forInfoDictionaryKey: "CFBundleName") as? String
                    ?? url.deletingPathExtension().lastPathComponent
                return .init(bundleID: identifier, name: name)
            }
        return .init(id: id, bundle: bundleID, pid: pid, applicationBundleID: identity.bundleID, applicationName: identity.name)
    }
}
