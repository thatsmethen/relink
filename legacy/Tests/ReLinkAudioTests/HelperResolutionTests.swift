import Foundation
import Testing
@testable import RaveLinkAudio

@Suite struct HelperResolutionTests {
    private struct Fixture {
        let root: URL
        let app: URL
        var main: URL { app.appendingPathComponent("Contents/MacOS/RaveLink") }
        var location: HelperResolution.Location { .init(bundleURL: app, executableURL: main) }
        init() throws {
            root = FileManager.default.temporaryDirectory.appendingPathComponent("helper-path-\(UUID().uuidString)").resolvingSymlinksInPath()
            app = root.appendingPathComponent("RaveLink.app")
            try FileManager.default.createDirectory(at: app.appendingPathComponent("Contents"), withIntermediateDirectories: true)
            let info: [String: Any] = ["CFBundleIdentifier": "test.ravelink.\(UUID().uuidString)", "CFBundlePackageType": "APPL", "CFBundleExecutable": "RaveLink"]
            try PropertyListSerialization.data(fromPropertyList: info, format: .xml, options: 0).write(to: app.appendingPathComponent("Contents/Info.plist"))
            try executable(main)
        }
        func executable(_ url: URL, mode: Int = 0o700) throws {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data("#!/bin/sh\nexit 0\n".utf8).write(to: url)
            try FileManager.default.setAttributes([.posixPermissions: mode], ofItemAtPath: url.path)
        }
        func remove() { try? FileManager.default.removeItem(at: root) }
        func resolve(_ kind: HelperResolution.Executable) -> URL? {
            HelperResolution.resolve(kind, locations: [location], command: main)
        }
    }

    @Test func intactPackageUsesOnlyItsExpectedHelperAndBothPublicResolversFailClosed() throws {
        let f = try Fixture(); defer { f.remove() }
        let bundle = try #require(Bundle(url: f.app))
        for kind in HelperResolution.Executable.allCases {
            // A sibling outside the bundle and an incorrect in-bundle location
            // must not rescue the missing packaged helper.
            try f.executable(f.root.appendingPathComponent(kind.name))
            try f.executable(f.app.appendingPathComponent("Contents/\(kind.name)"))
            #expect(f.resolve(kind) == nil)
        }
        #expect(EngineClient.defaultHelperURL(bundle: bundle) == nil)
        #expect(PluginCatalog.defaultHelperURL(bundle: bundle) == nil)
        for kind in HelperResolution.Executable.allCases {
            let expected = f.app.appendingPathComponent(kind.embeddedPath)
            try f.executable(expected)
            #expect(f.resolve(kind) == expected)
        }
        #expect(EngineClient.defaultHelperURL(bundle: bundle) == f.app.appendingPathComponent(HelperResolution.Executable.engine.embeddedPath))
        #expect(PluginCatalog.defaultHelperURL(bundle: bundle) == f.app.appendingPathComponent(HelperResolution.Executable.pluginHost.embeddedPath))
    }

    @Test func damagedNonExecutableAndDirectoryHelpersDoNotFallBack() throws {
        let f = try Fixture(); defer { f.remove() }
        for kind in HelperResolution.Executable.allCases {
            let expected = f.app.appendingPathComponent(kind.embeddedPath)
            try f.executable(f.root.appendingPathComponent(kind.name))
            try f.executable(expected, mode: 0o600)
            #expect(f.resolve(kind) == nil)
            try FileManager.default.removeItem(at: expected)
            try FileManager.default.createDirectory(at: expected, withIntermediateDirectories: true)
            #expect(f.resolve(kind) == nil)
        }
    }

    @Test func helperAndParentDirectorySymlinksCannotEscapePackage() throws {
        let f = try Fixture(); defer { f.remove() }
        for kind in HelperResolution.Executable.allCases {
            let expected = f.app.appendingPathComponent(kind.embeddedPath)
            let external = f.root.appendingPathComponent(kind.name)
            try f.executable(external)
            try FileManager.default.createDirectory(at: expected.deletingLastPathComponent(), withIntermediateDirectories: true)
            try FileManager.default.createSymbolicLink(at: expected, withDestinationURL: external)
            #expect(f.resolve(kind) == nil)
            try FileManager.default.removeItem(at: expected)
            // A link to another file inside the app is also not the packaged
            // executable at the signed expected path.
            let internalTarget = f.app.appendingPathComponent("Contents/Resources/\(kind.name)")
            try f.executable(internalTarget)
            try FileManager.default.createSymbolicLink(at: expected, withDestinationURL: internalTarget)
            #expect(f.resolve(kind) == nil)
        }
        try FileManager.default.removeItem(at: f.app.appendingPathComponent("Contents/Helpers"))
        try FileManager.default.createSymbolicLink(at: f.app.appendingPathComponent("Contents/Helpers"), withDestinationURL: f.root)
        #expect(f.resolve(.pluginHost) == nil)
    }

    @Test func appAliasAndExecutableDerivedAppKeepTheSameBoundary() throws {
        let f = try Fixture(); defer { f.remove() }
        let alias = f.root.appendingPathComponent("Alias.app")
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: f.app)
        for kind in HelperResolution.Executable.allCases {
            try f.executable(f.root.appendingPathComponent(kind.name))
            // Even if Foundation reports a non-app directory, the packaged
            // executable itself identifies the app and blocks fallback.
            #expect(HelperResolution.resolve(kind, locations: [.init(bundleURL: f.root, executableURL: f.main)], command: nil) == nil)
            #expect(HelperResolution.resolve(kind, locations: [], command: f.main) == nil)
            let expected = f.app.appendingPathComponent(kind.embeddedPath)
            try f.executable(expected)
            #expect(HelperResolution.resolve(kind, locations: [.init(bundleURL: alias)], command: nil) == expected)
        }
    }

    @Test func swiftPMAndTestBundleFallbacksRemainAvailableOutsideAnApp() throws {
        let f = try Fixture(); defer { f.remove() }
        let build = f.root.appendingPathComponent("debug")
        let testBundle = build.appendingPathComponent("RaveLinkPackageTests.xctest")
        let runner = testBundle.appendingPathComponent("Contents/MacOS/runner")
        let locations: [HelperResolution.Location] = [.init(bundleURL: testBundle, executableURL: runner)]
        for kind in HelperResolution.Executable.allCases {
            let helper = build.appendingPathComponent(kind.name)
            try f.executable(helper)
            #expect(HelperResolution.resolve(kind, locations: locations, command: runner) == helper)
            #expect(HelperResolution.resolve(kind, locations: [], command: runner) == helper)
            #expect(HelperResolution.resolve(kind, locations: [], command: build.appendingPathComponent("RaveLink")) == helper)
        }
        // Exercise the real current SwiftPM context without launching a helper
        // or querying Core Audio. Explicit helperURL process fixtures are tested
        // separately by EngineClientTests and PluginCatalogTests.
        #expect(EngineClient.defaultHelperURL() != nil)
        #expect(PluginCatalog.defaultHelperURL() != nil)
    }
}
