import Foundation
import Testing

@testable import TokenStationCore

@Suite("Helper log lines")
struct HelperLogLineTests {
    @Test("the level tracing stamped on a line is read back")
    func readsTracingLevels() {
        let stamp = "2026-09-29T11:02:02.266113Z"
        #expect(
            HelperLogLine.severity(
                of: "\(stamp)  INFO token_station::ipc::server: listening socket=/tmp/a.sock")
                == .info)
        #expect(
            HelperLogLine.severity(of: "\(stamp)  WARN token_station::pricing: table is stale")
                == .warning)
        #expect(
            HelperLogLine.severity(of: "\(stamp) ERROR token_station::daemon: cannot bind")
                == .error)
        #expect(HelperLogLine.severity(of: "\(stamp) DEBUG token_station: polling") == .debug)
        #expect(HelperLogLine.severity(of: "\(stamp) TRACE token_station: frame") == .debug)
    }

    @Test("a line with no level of its own has no severity")
    func reportsNoSeverityWithoutALevel() {
        #expect(HelperLogLine.severity(of: "thread 'main' panicked at src/main.rs:12:5") == nil)
        #expect(HelperLogLine.severity(of: "") == nil)
    }

    @Test("a message body that says ERROR does not promote a routine line")
    func ignoresTheLevelWordInsideAMessage() {
        let line = "2026-09-29T11:02:02.266113Z  INFO token_station: retrying after ERROR 503"
        #expect(HelperLogLine.severity(of: line) == .info)
    }

    @Test("only a complaint is worth quoting when the helper dies")
    func picksDiagnosticLines() {
        let routine = "2026-09-29T11:02:02.266113Z  INFO token_station::ipc::server: listening"
        #expect(!HelperLogLine.isDiagnostic(routine, onStandardError: true))
        #expect(!HelperLogLine.isDiagnostic(routine, onStandardError: false))

        let warning = "2026-09-29T11:02:02.266113Z  WARN token_station: the lock is held"
        #expect(HelperLogLine.isDiagnostic(warning, onStandardError: false))

        let panic = "thread 'main' panicked at src/main.rs:12:5"
        #expect(HelperLogLine.isDiagnostic(panic, onStandardError: true))
        // The same text on standard output is the helper talking, not failing.
        #expect(!HelperLogLine.isDiagnostic(panic, onStandardError: false))
    }
}
