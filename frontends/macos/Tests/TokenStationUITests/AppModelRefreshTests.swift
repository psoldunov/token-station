import Testing
import TokenStationCore

@testable import TokenStationUI

@MainActor
@Suite("App model refresh", .timeLimit(.minutes(1)))
struct AppModelRefreshTests {
    private let harness: AppHarness

    init() throws {
        harness = try AppHarness()
    }

    private var model: AppModel { harness.model }

    @Test("a refresh shows as running until the session answers")
    func refreshInFlight() async {
        model.refresh()
        #expect(model.isRefreshing)
        let request = await harness.session.refreshes.next()
        #expect(model.isRefreshing)
        request.succeed()
        await settle { !model.isRefreshing }
        #expect(model.refreshFailure == nil)
    }

    @Test("pressing refresh while one is running does not start another")
    func secondPressIgnored() async {
        model.refresh()
        model.refresh()
        let request = await harness.session.refreshes.next()
        request.succeed()
        await settle { !model.isRefreshing }
        await flush()
        #expect(await harness.session.refreshes.pendingCount == 0)
    }

    @Test("a failed refresh is surfaced, and the next good one clears it")
    func failureThenSuccess() async {
        model.refresh()
        await harness.session.refreshes.next().fail(TransportDown())
        await settle { !model.isRefreshing }
        #expect(model.refreshFailure == "socket closed")

        model.refresh()
        #expect(model.isRefreshing)
        await harness.session.refreshes.next().succeed()
        await settle { !model.isRefreshing }
        #expect(model.refreshFailure == nil)
    }

    @Test("reconnecting to the daemon clears an earlier refresh failure")
    func connectedClearsFailure() async throws {
        await harness.start()
        model.refresh()
        await harness.session.refreshes.next().fail(TransportDown())
        await settle { model.refreshFailure != nil }

        let snapshot = try RepositoryFixtures.snapshot(named: "ok")
        harness.session.emit(.connected(snapshot: snapshot, daemonVersion: nil))
        await settle { model.refreshFailure == nil }
    }

    @Test("retry clears an earlier refresh failure")
    func retryClearsFailure() async {
        model.refresh()
        await harness.session.refreshes.next().fail(TransportDown())
        await settle { model.refreshFailure != nil }

        model.retry()
        #expect(model.refreshFailure == nil)
        await settle { harness.session.retryCount == 1 }
    }
}
