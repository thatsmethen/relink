import Foundation

/// Exact bundle-ID membership used by the current tap implementation. Keep
/// empty entries: a previously idle excluded app becoming active is a change.
func applicationMemberships(bundleIDs: Set<String>, processes: [AudioProcessIdentity], excludingPID: Int32) -> [String: [UInt32]] {
    var result = Dictionary(uniqueKeysWithValues: bundleIDs.map { ($0, Set<UInt32>()) })
    for process in processes where process.pid != excludingPID {
        if bundleIDs.contains(process.applicationBundleID) { result[process.applicationBundleID, default: []].insert(process.id) }
        if bundleIDs.contains(process.bundle) { result[process.bundle, default: []].insert(process.id) }
    }
    return result.mapValues { $0.sorted() }
}

func applicationTapBundleIDs(selected: Set<String>, processes: [AudioProcessIdentity]) -> [String] {
    let matching = processes.filter { selected.contains($0.applicationBundleID) || selected.contains($0.bundle) }
    return Set(selected).union(matching.map(\.bundle).filter { !$0.isEmpty }).sorted()
}

struct TapProcessSelection: Equatable {
    let processIDs: [UInt32]
    let bundleIDs: [String]
}

/// A nil bundle denotes the global remainder. Both sides consume the identical
/// inventory and selected-app set; no separately timed inclusion/exclusion read.
func tapProcessSelection(bundleID: String?, dedicated: Set<String>, processes: [AudioProcessIdentity], ownPID: Int32, ownObjectID: UInt32) -> TapProcessSelection {
    if let bundleID {
        return .init(processIDs: Set(processes.filter { $0.pid != ownPID && $0.belongs(to: bundleID) }.map(\.id)).sorted(),
                     bundleIDs: applicationTapBundleIDs(selected: [bundleID], processes: processes))
    }
    let excluded = dedicated.union(["au.textual.ravelink"])
    let matching = processes.filter { $0.pid == ownPID || excluded.contains($0.applicationBundleID) || excluded.contains($0.bundle) }
    return .init(processIDs: Set(matching.map(\.id) + [ownObjectID]).sorted(),
                 bundleIDs: Set(applicationTapBundleIDs(selected: excluded, processes: processes)).union(matching.map(\.bundle).filter { !$0.isEmpty }).sorted())
}
