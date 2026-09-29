import Testing
import TokenStationCore

@testable import TokenStationUI

@MainActor
@Suite("App model connection", .timeLimit(.minutes(1)))
struct AppModelConnectionTests {
    private let harness: AppHarness
    private let snapshot: Snapshot

    init() throws {
        harness = try AppHarness()
        snapshot = try RepositoryFixtures.snapshot(named: "ok")
    }

    private var model: AppModel { harness.model }

    @Test("the panel starts out connecting and start is only honoured once")
    func startsConnecting() async {
        #expect(model.phase == .connecting)
        #expect(model.snapshot == nil)
        await harness.start()
        model.start()
        await harness.drain()
        #expect(harness.session.startCount == 1)
    }

    @Test("the first answer from the daemon puts the panel in the running phase")
    func connectedRuns() async {
        await harness.start()
        harness.session.emit(.connecting)
        await harness.drain()
        #expect(model.phase == .connecting)

        harness.session.emit(.connected(snapshot: snapshot, daemonVersion: "0.1.0"))
        await settle { model.phase == .running }
        #expect(model.snapshot == snapshot)
        #expect(model.daemonVersion == "0.1.0")
        #expect(harness.connectedCount == 1)
    }

    @Test("a cold start that fails shows why, and connecting again clears it")
    func coldFailure() async {
        await harness.start()
        harness.session.emit(.failed(kind: .neverStarted, message: "helper missing"))
        await settle { model.phase == .failed(.neverStarted, "helper missing") }
        #expect(model.snapshot == nil)

        harness.session.emit(.connecting)
        await settle { model.phase == .connecting }
    }

    @Test("a drop after the first snapshot keeps the contents and says it is restarting")
    func restartingKeepsContents() async {
        await harness.start()
        harness.session.emit(.connected(snapshot: snapshot, daemonVersion: nil))
        await settle { model.phase == .running }

        harness.session.emit(.connecting)
        await harness.drain()
        #expect(model.phase == .running)
        #expect(model.snapshot == snapshot)

        harness.session.emit(.failed(kind: .restarting, message: "daemon exited"))
        await settle { model.phase == .failed(.restarting, "daemon exited") }
        #expect(model.snapshot == snapshot)
    }

    @Test("a snapshot after a failure runs the panel again and replaces the old one")
    func snapshotReplacesAndRecovers() async {
        await harness.start()
        harness.session.emit(.connected(snapshot: snapshot, daemonVersion: "0.1.0"))
        await settle { model.phase == .running }
        harness.session.emit(.failed(kind: .restarting, message: "daemon exited"))
        await settle { model.phase != .running }

        var newer = snapshot
        newer.revision += 1
        newer.generatedAt += 60
        harness.session.emit(.snapshot(newer))
        await settle { model.phase == .running }
        #expect(model.snapshot == newer)
        #expect(model.daemonVersion == "0.1.0")
        #expect(harness.connectedCount == 1)
    }

    @Test("a running panel with nothing turned on reports no providers")
    func noProviders() async {
        #expect(!model.hasNoProviders)
        await harness.start()
        var empty = snapshot
        empty.providers = []
        empty.meter.bars = []
        harness.session.emit(.connected(snapshot: empty, daemonVersion: nil))
        await settle { model.hasNoProviders }

        harness.session.emit(.snapshot(snapshot))
        await settle { !model.hasNoProviders }
    }

    @Test("an alert push reaches the app's alert handler untouched")
    func forwardsAlerts() async {
        await harness.start()
        let alert = Alert(
            provider: .codex,
            providerName: "Codex",
            windowId: "codex:primary",
            windowLabel: "Weekly",
            kind: .critical,
            percent: 97)
        harness.session.emit(.alert(alert))
        await harness.drain()
        #expect(harness.forwardedAlerts == [alert])
    }

    @Test("retry goes back to connecting, clears the failure note and asks the session again")
    func retry() async {
        await harness.start()
        harness.session.emit(.failed(kind: .neverStarted, message: "helper missing"))
        await settle { model.phase != .connecting }

        model.retry()
        #expect(model.phase == .connecting)
        await settle { harness.session.retryCount == 1 }
    }

    @Test("the menu bar percentage preference is stored in the defaults it was given")
    func percentagePreference() throws {
        let defaults = try isolatedDefaults()
        let stored = try AppHarness(defaults: defaults)
        #expect(!stored.model.showPercentageInMenuBar)

        stored.model.showPercentageInMenuBar = true
        #expect(stored.defaults.store.bool(forKey: AppModel.showPercentageDefaultsKey))
        #expect(try AppHarness(defaults: defaults).model.showPercentageInMenuBar)
    }

    @Test("notificationsWanted reaches the app's handler")
    func notificationsWanted() {
        model.notificationsWanted()
        #expect(harness.notificationsWantedCount == 1)
    }
}
