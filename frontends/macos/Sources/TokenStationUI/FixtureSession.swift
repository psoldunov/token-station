import Foundation
import TokenStationCore

/// A session that answers from a fixture instead of from a daemon.
///
/// It is what `ts-render` and the previews draw against, and it is the only way
/// to put the panel in a state — a failed helper, an empty provider list — that
/// a healthy machine would never reach.
public final class FixtureSession: DaemonSession, @unchecked Sendable {
    public let events: AsyncStream<SessionEvent>
    private let continuation: AsyncStream<SessionEvent>.Continuation
    private let opening: SessionEvent
    private let series: [HistoryKey: [HistoryPoint]]
    private let settingsDocument: JSONValue

    public init(
        snapshot: Snapshot?,
        failure: String? = nil,
        failureKind: SessionFailure = .neverStarted,
        history: [HistoryKey: [HistoryPoint]] = [:],
        settings: JSONValue = .object([:])
    ) {
        (events, continuation) = AsyncStream<SessionEvent>.makeStream()
        series = history
        settingsDocument = settings
        switch (snapshot, failure) {
        case (_, let failure?):
            opening = .failed(kind: failureKind, message: failure)
        case (let snapshot?, _):
            opening = .connected(snapshot: snapshot, daemonVersion: "0.1.0")
        default:
            opening = .connecting
        }
    }

    public func start() async {
        continuation.yield(opening)
    }

    public func retry() async {
        continuation.yield(opening)
    }

    public func refresh() async throws {}

    public func history(
        provider: ProviderID,
        windowId: String,
        since: Int64
    ) async throws -> [HistoryPoint] {
        series[HistoryKey(provider: provider, windowId: windowId)] ?? []
    }

    public func settings() async throws -> JSONValue { settingsDocument }

    public func applySettings(_ settings: JSONValue) async throws {}
}

extension FixtureSession {
    /// A plausible history for every provider in a snapshot, so the charts in a
    /// render are the shape a real week of data would be.
    public static func syntheticHistory(for snapshot: Snapshot) -> [HistoryKey: [HistoryPoint]] {
        var series: [HistoryKey: [HistoryPoint]] = [:]
        for provider in snapshot.providers {
            guard let window = PanelText.meterWindow(for: provider, in: snapshot) else { continue }
            let span = PanelText.historySpan(for: window)
            let count = 60
            let end = snapshot.generatedAt
            let points = (0..<count).map { step -> HistoryPoint in
                let progress = Double(step) / Double(count - 1)
                // A window fills as it runs out, with a little noise on top.
                let wobble = sin(progress * 9) * 2.5
                let value = max(0, min(100, window.usedPercent * progress + wobble))
                return HistoryPoint(
                    timestamp: end - span + Int64(progress * Double(span)),
                    percent: value)
            }
            series[HistoryKey(provider: provider.id, windowId: window.id)] = points
        }
        return series
    }
}
