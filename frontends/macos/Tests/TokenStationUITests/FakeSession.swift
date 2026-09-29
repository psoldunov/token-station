import Foundation
import TokenStationCore
import TokenStationUI

/// One `GetHistory` call, as the session saw it.
struct HistoryRequest: Equatable, Sendable {
    let provider: ProviderID
    let windowId: String
    let since: Int64
}

/// A `DaemonSession` the test drives by hand.
///
/// Events go in through `emit`. Refreshes, settings reads and settings writes
/// wait in a `Requests` queue for the test to answer. History answers with an
/// empty series straight away, so the tests that do not care about charts need
/// not answer anything; `holdHistory()` puts it in a queue too.
final class FakeSession: DaemonSession, @unchecked Sendable {
    let events: AsyncStream<SessionEvent>
    let refreshes = Requests<Void, Void>()
    let histories = Requests<HistoryRequest, [HistoryPoint]>()
    let settingsReads = Requests<Void, JSONValue>()
    let settingsWrites = Requests<JSONValue, Void>()

    private let continuation: AsyncStream<SessionEvent>.Continuation
    private let lock = NSLock()
    private var starts = 0
    private var retries = 0
    private var holdsHistory = false

    init() {
        (events, continuation) = AsyncStream<SessionEvent>.makeStream()
    }

    func emit(_ event: SessionEvent) {
        continuation.yield(event)
    }

    func holdHistory() {
        lock.withLock { holdsHistory = true }
    }

    var startCount: Int { lock.withLock { starts } }
    var retryCount: Int { lock.withLock { retries } }

    func start() async {
        lock.withLock { starts += 1 }
    }

    func retry() async {
        lock.withLock { retries += 1 }
    }

    func refresh() async throws {
        try await refreshes.call(())
    }

    func history(
        provider: ProviderID,
        windowId: String,
        since: Int64
    ) async throws -> [HistoryPoint] {
        guard lock.withLock({ holdsHistory }) else { return [] }
        let request = HistoryRequest(provider: provider, windowId: windowId, since: since)
        return try await histories.call(request)
    }

    func settings() async throws -> JSONValue {
        try await settingsReads.call(())
    }

    func applySettings(_ settings: JSONValue) async throws {
        try await settingsWrites.call(settings)
    }
}
