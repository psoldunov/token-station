import Foundation
import Testing

@testable import TokenStationCore

@Suite("Panel text")
struct PanelTextTests {
    private func fixture(_ name: String) throws -> Snapshot {
        let url = RepositoryFixtures.directory.appending(path: name)
        return try JSONDecoder().decode(Snapshot.self, from: try Data(contentsOf: url))
    }

    @Test("a window reads as its percentage and its countdown")
    func writesWindowDetail() throws {
        let snapshot = try fixture("snapshot-ok.json")
        let window = snapshot.providers[0].windows[0]
        let now = Date(timeIntervalSince1970: 1_790_605_560)
        #expect(PanelText.windowPercent(window) == "34%")
        #expect(PanelText.windowReset(window, now: now) == " · resets in 2 hr 14 min")
        #expect(PanelText.windowAccessibilityLabel(window, now: now)
            == "Session, 34% used, resets in 2 hours, 14 minutes")
    }

    @Test("credits say so when extra usage is off")
    func writesCreditsLine() throws {
        let snapshot = try fixture("snapshot-ok.json")
        #expect(PanelText.creditsLine(snapshot.providers[0].credits) == "Extra usage: Turned off")
        #expect(PanelText.creditsLine(snapshot.providers[1].credits)
            == "Credits: 1 reset credit available")
        #expect(PanelText.creditsLine(nil) == nil)
    }

    @Test("the breakdown leaves out the rows at zero")
    func writesBreakdownLine() throws {
        let snapshot = try fixture("snapshot-ok.json")
        #expect(PanelText.breakdownLine(snapshot.providers[0].breakdown)
            == "Claude Code 97% · Cowork 3%")
        #expect(PanelText.breakdownLine([]) == nil)
    }

    @Test("token lines name the device they came from")
    func writesTokenLines() throws {
        let snapshot = try fixture("snapshot-ok.json")
        let claude = PanelText.tokenLines(snapshot.providers[0])
        #expect(claude.count == 2)
        #expect(claude[0].hasPrefix("This device · Today 43.3M tokens · "))
        #expect(claude[1].hasPrefix("Last 7 Days 317.2M tokens · "))

        let codex = PanelText.tokenLines(snapshot.providers[1])
        #expect(codex.count == 3)
        #expect(codex[2] == "All devices · Today 1.2M · Last 7 Days 9.9M tokens")
    }

    @Test("a non-ok provider has something to say even with no message")
    func writesStateMessage() throws {
        let snapshot = try fixture("snapshot-loading.json")
        #expect(PanelText.stateMessage(snapshot.providers[0]) == "Loading…")

        let stale = try fixture("snapshot-stale-auth.json")
        #expect(PanelText.stateMessage(stale.providers[0])
            == "Sign-in expired. Open Claude Code to refresh it.")
        #expect(PanelText.stateMessage(try fixture("snapshot-ok.json").providers[0]) == nil)
    }

    @Test("the chart follows the window the meter shows")
    func picksTheMeterWindow() throws {
        let snapshot = try fixture("snapshot-ok.json")
        let claudeWindow = try #require(
            PanelText.meterWindow(for: snapshot.providers[0], in: snapshot))
        #expect(claudeWindow.id == "session")
        #expect(PanelText.chartCaption(claudeWindow) == "Session · Last 24 Hours")
        #expect(PanelText.historySpan(for: claudeWindow) == 86_400)

        let codexWindow = try #require(
            PanelText.meterWindow(for: snapshot.providers[1], in: snapshot))
        #expect(PanelText.chartCaption(codexWindow) == "Weekly · Last 7 Days")
        #expect(PanelText.historySpan(for: codexWindow) == 604_800)
    }

    @Test("a window the meter does not name falls back to the first one")
    func fallsBackToTheFirstWindow() throws {
        let snapshot = try fixture("snapshot-stale-auth.json")
        let codex = try #require(snapshot.providers.first { $0.id == .codex })
        #expect(PanelText.meterWindow(for: codex, in: snapshot) == nil)
    }

    @Test("a per-model row shows its total and its cost")
    func writesModelDetail() throws {
        let snapshot = try fixture("snapshot-ok.json")
        let row = try #require(snapshot.providers[0].tokens?.byModel.first)
        #expect(PanelText.modelDetail(row).hasPrefix("272.7M · "))
    }
}
