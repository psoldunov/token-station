import Foundation

/// The snapshots in `data/fixtures`, which the daemon and every front end share.
///
/// They are read from the repository rather than copied into a resource bundle:
/// the app ships without one, because a bundle at the root of a `.app` breaks
/// its signature.
enum RepositoryFixtures {
    static var directory: URL {
        URL(filePath: #filePath)
            .deletingLastPathComponent()  // TokenStationCoreTests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // macos
            .deletingLastPathComponent()  // frontends
            .deletingLastPathComponent()  // repository root
            .appending(path: "data/fixtures", directoryHint: .isDirectory)
    }

    static func urls() throws -> [URL] {
        try FileManager.default
            .contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "json" }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }
    }
}
