import Foundation
import Testing

@testable import TokenStationCore

@Suite("Alert wording")
struct AlertTextTests {
    private func alert(
        kind: AlertKind,
        percent: Double,
        resetsAt: Int64? = nil
    ) -> Alert {
        Alert(
            provider: .claude,
            providerName: "Claude Code",
            windowId: "session",
            windowLabel: "Session",
            kind: kind,
            percent: percent,
            resetsAt: resetsAt
        )
    }

    @Test("a threshold crossing names the window and the reading")
    func wordsAWarning() {
        let now = Date(timeIntervalSince1970: 1_790_596_800)
        let warning = alert(kind: .warning, percent: 82.4, resetsAt: 1_790_604_840)
        #expect(AlertText.title(warning) == "Claude Code Session at 82%")
        #expect(AlertText.body(warning, now: now) == "Resets in 2 hr 14 min.")
    }

    @Test("a reset says the usage is back down")
    func wordsAReset() {
        let reset = alert(kind: .reset, percent: 1)
        #expect(AlertText.title(reset) == "Claude Code Session Reset")
        #expect(AlertText.body(reset) == "Usage is back to 1%.")
    }

    @Test("a warning with no reset time still says something")
    func wordsAWarningWithoutAReset() {
        let warning = alert(kind: .warning, percent: 90)
        #expect(AlertText.body(warning) == "Session usage is at 90%.")
    }

    @Test("alerts about one provider share a thread")
    func groupsByProvider() {
        #expect(AlertText.threadIdentifier(alert(kind: .warning, percent: 1))
            == "provider.claude")
    }
}
