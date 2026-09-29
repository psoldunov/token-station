import TokenStationCore

/// Why the daemon is not there.
public enum SessionFailure: Sendable, Equatable {
    /// It has never answered in this run of the app.
    case neverStarted
    /// It was answering and went away; another attempt is already on its way.
    case restarting
}

/// What the app's daemon supervisor tells the panel.
public enum SessionEvent: Sendable {
    /// Looking for the daemon, or starting it.
    case connecting
    /// Talking to the daemon; the snapshot is the one `Subscribe` answered with.
    case connected(snapshot: Snapshot, daemonVersion: String?)
    /// A later `SnapshotChanged`.
    case snapshot(Snapshot)
    /// An `Alert` push.
    case alert(Alert)
    /// The daemon could not be reached or would not stay up.
    case failed(kind: SessionFailure, message: String)
}

/// The panel's view of the daemon.
///
/// The app fulfils this with a supervised socket connection; `ts-render` fulfils
/// it with a fixture, which is what lets the panel be drawn without a daemon.
public protocol DaemonSession: Sendable {
    /// Everything the session reports, oldest first.
    var events: AsyncStream<SessionEvent> { get }
    /// Begins connecting. Call once.
    func start() async
    /// Tries again after `.failed`.
    func retry() async
    /// `Refresh`, returning once the refresh has finished.
    func refresh() async throws
    func history(
        provider: ProviderID,
        windowId: String,
        since: Int64
    ) async throws -> [HistoryPoint]
    func settings() async throws -> JSONValue
    func applySettings(_ settings: JSONValue) async throws
}
