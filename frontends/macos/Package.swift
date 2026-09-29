// swift-tools-version: 6.0
import PackageDescription

/// Strict concurrency plus the Swift 6 language mode everywhere; the app is
/// built by `scripts/build-app.sh`, never by Xcode, so there is no project file
/// to carry these settings instead.
let strict: [SwiftSetting] = [
    .swiftLanguageMode(.v6),
]

let package = Package(
    name: "TokenStation",
    platforms: [.macOS(.v14)],
    // The two libraries are deliberately not products: nothing outside this
    // package consumes them, and vending them would make every `public` symbol
    // look reachable to a dead-code scan.
    products: [
        .executable(name: "TokenStation", targets: ["TokenStation"]),
        .executable(name: "ts-render", targets: ["ts-render"]),
    ],
    targets: [
        .target(
            name: "TokenStationCore",
            swiftSettings: strict
        ),
        .target(
            name: "TokenStationUI",
            dependencies: ["TokenStationCore"],
            swiftSettings: strict
        ),
        .executableTarget(
            name: "TokenStation",
            dependencies: ["TokenStationCore", "TokenStationUI"],
            swiftSettings: strict
        ),
        .executableTarget(
            name: "ts-render",
            dependencies: ["TokenStationCore", "TokenStationUI"],
            swiftSettings: strict
        ),
        .testTarget(
            name: "TokenStationCoreTests",
            dependencies: ["TokenStationCore"],
            swiftSettings: strict
        ),
        .testTarget(
            name: "TokenStationUITests",
            dependencies: ["TokenStationCore", "TokenStationUI"],
            swiftSettings: strict
        ),
    ]
)
