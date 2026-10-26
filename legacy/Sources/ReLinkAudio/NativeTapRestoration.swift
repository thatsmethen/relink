import CoreAudio
import Foundation

/// These documented properties were added in the macOS 26 SDK. Dynamic public
/// setters keep the macOS 15-compatible source buildable with older Xcode SDKs;
/// an OS-version and selector check guards every KVC call. Process IDs remain
/// authoritative on systems that do not implement both optional setters.
@discardableResult
func configureNativeTapRestoration(_ description: CATapDescription, bundleIDs: [String]) -> Bool {
    guard #available(macOS 26.0, *),
          description.responds(to: NSSelectorFromString("setBundleIDs:")),
          description.responds(to: NSSelectorFromString("setProcessRestoreEnabled:")) else { return false }
    description.setValue(bundleIDs, forKey: "bundleIDs")
    description.setValue(true, forKey: "processRestoreEnabled")
    return true
}
