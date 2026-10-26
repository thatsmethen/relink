// swift-tools-version: 6.0
import PackageDescription
let package = Package(
    name: "RaveLink",
    platforms: [.macOS(.v15)],
    products: [
        .executable(name: "RaveLink", targets: ["RaveLinkApp"]),
        .executable(name: "RaveLinkEngine", targets: ["RaveLinkEngine"]),
        .executable(name: "RaveLinkPluginHost", targets: ["RaveLinkPluginHost"]),
        .library(name: "RaveLinkCore", targets: ["RaveLinkCore"]),
        .library(name: "RaveLinkAudio", targets: ["RaveLinkAudio"])
    ],
    targets: [
        .target(name: "RaveLinkCore", path: "Sources/ReLinkCore"),
        .executableTarget(name: "RaveLinkPluginHost", path: "Sources/ReLinkPluginHost"),
        .executableTarget(name: "RaveLinkEngine", dependencies: ["RaveLinkAudio", "RaveLinkCore"], path: "Sources/ReLinkEngine"),
        .target(name: "RaveLinkRealtime", path: "Sources/ReLinkRealtime", publicHeadersPath: "include"),
        .target(name: "RaveLinkAudioUnitBridge", path: "Sources/ReLinkAudioUnitBridge", publicHeadersPath: "include", cSettings: [.unsafeFlags(["-fno-objc-arc"])], linkerSettings: [.linkedFramework("AVFoundation"), .linkedFramework("AudioToolbox")]),
        .target(name: "RaveLinkAudio", dependencies: ["RaveLinkCore", "RaveLinkRealtime", "RaveLinkAudioUnitBridge"], path: "Sources/ReLinkAudio"),
        .executableTarget(name: "RaveLinkApp", dependencies: ["RaveLinkCore", "RaveLinkAudio"], path: "Sources/ReLinkApp"),
        .testTarget(name: "RaveLinkCoreTests", dependencies: ["RaveLinkCore"], path: "Tests/ReLinkCoreTests"),
        .testTarget(name: "RaveLinkAppTests", dependencies: ["RaveLinkApp", "RaveLinkCore", "RaveLinkAudio"], path: "Tests/ReLinkAppTests"),
        .testTarget(name: "RaveLinkAudioTests", dependencies: ["RaveLinkAudio", "RaveLinkCore", "RaveLinkRealtime"], path: "Tests/ReLinkAudioTests")
    ],
    swiftLanguageModes: [.v5]
)
