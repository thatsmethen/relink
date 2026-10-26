import Testing
@testable import RaveLinkAudio
import Foundation
import CoreAudio

private func process(_ id: UInt32, _ bundle: String, _ pid: Int32, owner: String? = nil) -> AudioProcessIdentity {
    .init(id: id, bundle: bundle, pid: pid, applicationBundleID: owner)
}

@Suite struct ApplicationMembershipTests {
    @Test func initiallyAbsentExcludedApplicationRemainsWatched() {
        let ids: Set<String> = ["com.example.captured", "com.example.excluded"]
        let initial = applicationMemberships(bundleIDs: ids,
            processes: [process(11, "com.example.captured", 101), process(90, "com.other.app", 901)], excludingPID: 999)
        #expect(initial == ["com.example.captured": [11], "com.example.excluded": []])
        let afterLaunch = applicationMemberships(bundleIDs: ids,
            processes: [process(11, "com.example.captured", 101), process(22, "com.example.excluded", 202)], excludingPID: 999)
        #expect(afterLaunch != initial)
    }
    @Test func restartAdditionalProcessAndExitChangeExclusions() {
        let ids: Set<String> = ["com.example.excluded"]
        let before = applicationMemberships(bundleIDs: ids, processes: [process(22, "com.example.excluded", 202)], excludingPID: 999)
        let restart = applicationMemberships(bundleIDs: ids, processes: [process(23, "com.example.excluded", 203)], excludingPID: 999)
        let additional = applicationMemberships(bundleIDs: ids, processes: [process(22, "com.example.excluded", 202), process(24, "com.example.excluded", 204)], excludingPID: 999)
        let exited = applicationMemberships(bundleIDs: ids, processes: [], excludingPID: 999)
        #expect(restart != before && additional != before && exited != before)
    }
    @Test func orderingDuplicatesOwnProcessAndUnrelatedAppsDoNotChangeMembership() {
        let ids: Set<String> = ["com.example.excluded"]
        let initial = applicationMemberships(bundleIDs: ids, processes: [process(22, "com.example.excluded", 202), process(23, "com.example.excluded", 203)], excludingPID: 999)
        let unchanged = applicationMemberships(bundleIDs: ids, processes: [process(23, "com.example.excluded", 203), process(22, "com.example.excluded", 202),
            process(22, "com.example.excluded", 202), process(99, "com.example.excluded", 999), process(33, "com.other", 303)], excludingPID: 999)
        #expect(initial == unchanged)
    }
    @Test func chromiumHelpersUseOwnerIdentityWithoutPrefixGuessing() {
        let processes = [process(1, "com.browser", 100), process(2, "com.browser.helper.audio", 101, owner: "com.browser"),
                         process(3, "com.browser.other", 102), process(4, "", 103, owner: "com.browser")]
        let memberships = applicationMemberships(bundleIDs: ["com.browser"], processes: processes, excludingPID: 999)
        #expect(memberships["com.browser"] == [1, 2, 4])
        #expect(applicationTapBundleIDs(selected: ["com.browser"], processes: processes) == ["com.browser", "com.browser.helper.audio"])
        #expect(applicationMemberships(bundleIDs: ["com.browser.helper.audio"], processes: processes, excludingPID: 999)["com.browser.helper.audio"] == [2])
    }
    @Test func inclusionAndCatchAllShareCanonicalHelpersAndOwnProcessExclusions() {
        let processes = [process(1, "com.browser", 100), process(2, "com.browser.audio", 101, owner: "com.browser"),
                         process(3, "com.browser.unrelated", 102), process(4, "au.textual.ravelink.engine", 999, owner: "au.textual.ravelink"),
                         process(5, "au.textual.ravelink", 888)]
        let included = tapProcessSelection(bundleID: "com.browser", dedicated: ["com.browser"], processes: processes, ownPID: 999, ownObjectID: 4)
        let excluded = tapProcessSelection(bundleID: nil, dedicated: ["com.browser", "com.idle"], processes: processes, ownPID: 999, ownObjectID: 4)
        #expect(included.processIDs == [1, 2])
        #expect(excluded.processIDs == [1, 2, 4, 5])
        #expect(Set(included.processIDs).isSubset(of: Set(excluded.processIDs)))
        #expect(excluded.bundleIDs.contains("com.idle"))
        #expect(excluded.bundleIDs.contains("com.browser.audio"))
        #expect(!excluded.bundleIDs.contains("com.browser.unrelated"))
        let afterExit = tapProcessSelection(bundleID: "com.browser", dedicated: ["com.browser"], processes: [], ownPID: 999, ownObjectID: 4)
        #expect(afterExit.processIDs.isEmpty && afterExit.bundleIDs == ["com.browser"])
    }
    @Test func outerApplicationMetadataWinsOverNestedHelper() {
        let executable = URL(fileURLWithPath: "/Applications/Browser.app/Contents/Frameworks/Framework.framework/Helpers/Browser Helper.app/Contents/MacOS/Helper")
        let candidates = ApplicationIdentityResolver.bundleCandidates(for: executable)
        #expect(candidates.map(\.lastPathComponent) == ["Browser.app", "Browser Helper.app"])
        let owner = ApplicationIdentityResolver.resolve(executable: executable, fallback: .init(bundleID: "reported.helper", name: "Helper")) { url in
            .init(bundleID: url.lastPathComponent == "Browser.app" ? "com.browser" : "com.browser.helper", name: url.lastPathComponent)
        }
        #expect(owner.bundleID == "com.browser")
        let unresolved = ApplicationIdentityResolver.resolve(executable: URL(fileURLWithPath: "/usr/libexec/shared-service"), fallback: .init(bundleID: "shared.service", name: "Shared")) { _ in nil }
        #expect(unresolved.bundleID == "shared.service")
    }
}

@Suite struct NativeTapRestorationTests {
    @Test func optionalPublicSettersAreCheckedWithoutCreatingAnAudioTap() {
        // Constructing this value object does not create a tap or request consent.
        let description = CATapDescription(stereoMixdownOfProcesses: [123])
        let applied = configureNativeTapRestoration(description, bundleIDs: ["com.example.selected"])
        if #available(macOS 26.0, *) {
            if description.responds(to: NSSelectorFromString("setBundleIDs:")) && description.responds(to: NSSelectorFromString("setProcessRestoreEnabled:")) {
                #expect(applied)
                #expect(description.value(forKey: "bundleIDs") as? [String] == ["com.example.selected"])
                #expect(description.value(forKey: "processRestoreEnabled") as? Bool == true)
            } else { #expect(!applied) }
        } else { #expect(!applied) }
        #expect(description.processes == [123])
    }
}
