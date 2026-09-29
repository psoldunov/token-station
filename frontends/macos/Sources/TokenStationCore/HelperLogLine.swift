import Foundation

/// One line of the Rust helper's output, read for the level `tracing` stamped on it.
///
/// The helper writes every level to standard error, routine or not:
///
/// ```text
/// 2026-09-29T11:02:02.266113Z  INFO token_station::ipc::server: listening socket=…
/// ```
///
/// Logging all of it as an error fills the unified log's error filter with
/// startup chatter, and — worse — makes the last line the helper printed a
/// useless thing to quote back when it dies, because that line is almost always
/// a routine one. Reading the level out of the line fixes both.
public enum HelperLogLine {
    /// What a line is, as far as the app cares.
    public enum Severity: Sendable, Equatable {
        case debug
        case info
        case warning
        case error
    }

    /// How many whitespace-separated fields at the front may hold the level: the
    /// timestamp and the level itself. Looking further would let a message body
    /// that happens to say `ERROR` promote a routine line.
    private static let levelFieldLimit = 2

    /// The level `tracing` printed, or `nil` when the line carries none.
    public static func severity(of line: String) -> Severity? {
        for field in line.split(separator: " ", omittingEmptySubsequences: true)
            .prefix(levelFieldLimit) {
            switch field {
            case "TRACE", "DEBUG": return .debug
            case "INFO": return .info
            case "WARN", "WARNING": return .warning
            case "ERROR": return .error
            default: continue
            }
        }
        return nil
    }

    /// Whether the line is worth quoting back when the helper fails.
    ///
    /// A line with no level of its own counts only on standard error, which is
    /// where a panic, a usage error and a linker failure all come out.
    public static func isDiagnostic(_ line: String, onStandardError: Bool) -> Bool {
        switch severity(of: line) {
        case .warning, .error: true
        case .debug, .info: false
        case nil: onStandardError
        }
    }
}
