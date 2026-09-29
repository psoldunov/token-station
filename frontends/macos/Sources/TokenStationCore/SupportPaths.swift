import Foundation

/// Where the daemon's files live on macOS.
public enum SupportPaths {
    /// Bundle identifier, which is also the support directory's name.
    public static let bundleIdentifier = "dev.soldunov.TokenStation"

    /// `~/Library/Application Support/dev.soldunov.TokenStation`.
    public static func supportDirectory(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> URL {
        let home = homeDirectory(environment: environment)
        return home
            .appending(path: "Library/Application Support", directoryHint: .isDirectory)
            .appending(path: bundleIdentifier, directoryHint: .isDirectory)
    }

    /// The daemon socket, honouring `$TOKEN_STATION_SOCKET`.
    ///
    /// The override is what keeps a development build, and every test, off the
    /// socket a real daemon is already serving.
    public static func socketPath(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> String {
        if let override = environment["TOKEN_STATION_SOCKET"], !override.isEmpty {
            return override
        }
        return supportDirectory(environment: environment)
            .appending(path: "daemon.sock", directoryHint: .notDirectory)
            .path(percentEncoded: false)
    }

    /// The `config.toml` the daemon reads.
    ///
    /// One path, always: on macOS the daemon keeps its configuration, its state
    /// and its socket in the Application Support directory and ignores the XDG
    /// variables it honours on Linux. `$TOKEN_STATION_SOCKET` moves the socket
    /// and nothing else.
    public static func configPath(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> String {
        supportDirectory(environment: environment)
            .appending(path: "config.toml", directoryHint: .notDirectory)
            .path(percentEncoded: false)
    }

    /// `~/Library/Caches/dev.soldunov.TokenStation`, where the daemon caches.
    public static func cacheDirectory(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> URL {
        homeDirectory(environment: environment)
            .appending(path: "Library/Caches", directoryHint: .isDirectory)
            .appending(path: bundleIdentifier, directoryHint: .isDirectory)
    }

    /// The bundled helper, or the `$TOKEN_STATION_HELPER` override used in development.
    public static func helperOverride(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> String? {
        guard let path = environment["TOKEN_STATION_HELPER"], !path.isEmpty else { return nil }
        return path
    }

    private static func homeDirectory(environment: [String: String]) -> URL {
        if let home = environment["HOME"], !home.isEmpty {
            return URL(filePath: home, directoryHint: .isDirectory)
        }
        return FileManager.default.homeDirectoryForCurrentUser
    }
}
