import RaveLinkCore

/// First-use ownership is stable in the expanded source order. Identical capture
/// selectors share one IOProc and one resampling cursor per output clock; every
/// logical input still owns its group effects, sends, and meters. Tone generators
/// remain independent and do not contribute to native capture diagnostics.
struct CaptureSourcePlan {
    let roots: [Int]
    let uniqueCaptureCount: Int
    init(sources: [InputSource]) {
        var owners: [InputSource: Int] = [:]
        var roots: [Int] = []
        roots.reserveCapacity(sources.count)
        for (index, source) in sources.enumerated() {
            if source.kind == .tone { roots.append(index); continue }
            let identity = source.canonicalized
            if let owner = owners[identity] { roots.append(owner) }
            else { owners[identity] = index; roots.append(index) }
        }
        self.roots = roots; uniqueCaptureCount = owners.count
    }
}
