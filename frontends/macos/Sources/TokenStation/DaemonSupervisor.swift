import Foundation
import OSLog
import TokenStationCore
import TokenStationUI

/// Keeps one connection to the daemon alive, starting the daemon when nobody else has.
///
/// Connecting comes first: a daemon started by launchd, by a terminal, or by a
/// second copy of the app is one to talk to, not one to replace. Only when the
/// socket answers nothing does the app start the bundled helper, and a helper
/// that exits with `EX_TEMPFAIL` means somebody else won the race — which is
/// also a reason to connect rather than to report a failure.
///
/// Every attempt carries the generation it was started at. A loop that `retry()`
/// cancelled can still be somewhere inside an `await` when the next one begins;
/// tagging its work is what stops it adopting a connection or reporting a
/// failure beside the loop that replaced it.
public actor DaemonSupervisor: DaemonSession {
    private static let firstBackoff: Duration = .seconds(1)
    private static let maximumBackoff: Duration = .seconds(60)
    /// How long the helper is given to open its socket.
    private static let startupAttempts = 50
    private static let startupPollInterval: Duration = .milliseconds(200)

    nonisolated public let events: AsyncStream<SessionEvent>
    private let continuation: AsyncStream<SessionEvent>.Continuation
    private let logger = Logger(subsystem: SupportPaths.bundleIdentifier, category: "daemon")
    private let socketPath: String
    private let helperURL: URL?
    /// What `applicationWillTerminate` can reach without waiting for the actor.
    private let live = LiveResources()

    private var connection: DaemonConnection?
    private var helper: HelperProcess?
    private var loop: Task<Void, Never>?
    private var generation = 0
    private var version: String?
    /// Whether the daemon has ever answered in this run of the app.
    private var hasConnected = false

    public init(
        socketPath: String = SupportPaths.socketPath(),
        helperURL: URL? = DaemonSupervisor.bundledHelper()
    ) {
        self.socketPath = socketPath
        self.helperURL = helperURL
        (events, continuation) = AsyncStream<SessionEvent>.makeStream()
    }

    /// The helper inside the app bundle, or the development override.
    public static func bundledHelper() -> URL? {
        if let override = SupportPaths.helperOverride() {
            return URL(filePath: override)
        }
        return Bundle.main.url(forAuxiliaryExecutable: "token-station")
    }

    // MARK: - DaemonSession

    public func start() {
        guard loop == nil else { return }
        generation += 1
        let current = generation
        loop = Task { await self.run(generation: current) }
    }

    /// Starts over at once rather than waiting out the backoff.
    public func retry() {
        loop?.cancel()
        loop = nil
        connection?.close()
        connection = nil
        live.setConnection(nil)
        start()
    }

    public func refresh() async throws {
        try await requireConnection().refresh()
    }

    public func history(
        provider: ProviderID,
        windowId: String,
        since: Int64
    ) async throws -> [HistoryPoint] {
        try await requireConnection().getHistory(
            provider: provider, windowId: windowId, since: since)
    }

    public func settings() async throws -> JSONValue {
        try await requireConnection().getSettings()
    }

    public func applySettings(_ settings: JSONValue) async throws {
        try await requireConnection().setSettings(settings)
    }

    /// Stops the loop and the helper, from whatever thread is quitting.
    ///
    /// A task scheduled out of `applicationWillTerminate` is not guaranteed to
    /// run before the process goes away, so this does its work at once and
    /// leaves the actor to notice afterwards.
    nonisolated public func shutdownNow() {
        live.stop()
        continuation.finish()
    }

    // MARK: - The loop

    private func requireConnection() throws -> DaemonConnection {
        guard let connection else { throw DaemonTransportError.connectionClosed }
        return connection
    }

    private func run(generation: Int) async {
        var backoff = Self.firstBackoff
        while !Task.isCancelled, generation == self.generation, !live.isStopped {
            continuation.yield(.connecting)
            do {
                try await attempt(generation: generation)
                backoff = Self.firstBackoff
            } catch is CancellationError {
                return
            } catch {
                clearConnection(ifGeneration: generation)
                guard generation == self.generation, !Task.isCancelled else { return }
                logger.error(
                    "cannot reach the daemon: \(error.localizedDescription, privacy: .public)")
                continuation.yield(.failed(
                    kind: hasConnected ? .restarting : .neverStarted,
                    message: error.localizedDescription))
            }
            guard !Task.isCancelled, generation == self.generation else { return }
            do {
                try await Task.sleep(for: backoff)
            } catch {
                return  // Cancelled: a newer loop is already running.
            }
            backoff = min(backoff * 2, Self.maximumBackoff)
        }
    }

    /// One connect-subscribe-drain round.
    private func attempt(generation: Int) async throws {
        let connection = try await connect()
        // `retry()` replaced this loop while it was connecting, so the
        // connection belongs to nobody: close it rather than publish it beside
        // the one the newer loop is making.
        guard generation == self.generation, !live.isStopped else {
            connection.close()
            throw CancellationError()
        }
        self.connection = connection
        live.setConnection(connection)

        let envelope = try await connection.subscribe()
        version = try? await connection.getVersion().version
        guard generation == self.generation else {
            connection.close()
            throw CancellationError()
        }
        hasConnected = true
        continuation.yield(.connected(snapshot: envelope.snapshot, daemonVersion: version))

        await drain(connection, generation: generation)
        clearConnection(ifGeneration: generation)
    }

    private func clearConnection(ifGeneration generation: Int) {
        guard generation == self.generation else { return }
        connection?.close()
        connection = nil
        live.setConnection(nil)
    }

    private func drain(_ connection: DaemonConnection, generation: Int) async {
        // Whatever ends this loop — a close, a cancellation, a replaced
        // generation — the socket goes with it rather than leaking its
        // descriptor until the app quits.
        defer { connection.close() }
        for await notification in connection.notifications {
            guard generation == self.generation else { return }
            switch notification {
            case .snapshotChanged(_, let snapshot):
                continuation.yield(.snapshot(snapshot))
            case .alert(let alert):
                continuation.yield(.alert(alert))
            case .closed(let reason):
                if let reason {
                    logger.notice("the daemon connection closed: \(reason, privacy: .public)")
                }
                return
            }
        }
    }

    private func connect() async throws -> DaemonConnection {
        if let connection = try? await DaemonConnection.open(socketPath: socketPath) {
            return connection
        }
        try startHelper()
        for _ in 0..<Self.startupAttempts {
            try Task.checkCancellation()
            try await Task.sleep(for: Self.startupPollInterval)
            if let connection = try? await DaemonConnection.open(socketPath: socketPath) {
                return connection
            }
            if let status = helper?.terminationStatus,
               status != HelperProcess.alreadyRunningStatus {
                let detail = helper?.lastProblemLine ?? "exit status \(status)"
                discardHelper()
                throw SupervisorError.helperExited(detail)
            }
        }
        // It is up and it is not listening. Keeping it would mean every later
        // attempt sees a running helper, skips the spawn, and waits on a socket
        // that is never going to appear.
        discardHelper()
        throw SupervisorError.helperNeverListened(socketPath)
    }

    private func startHelper() throws {
        if let helper, helper.isRunning { return }
        guard !live.isStopped else { throw CancellationError() }
        guard let helperURL else { throw SupervisorError.helperMissing }
        let process = HelperProcess(
            executable: helperURL, socketPath: socketPath, logger: logger)
        do {
            try process.run()
        } catch {
            throw SupervisorError.helperCannotStart(error.localizedDescription)
        }
        helper = process
        live.setHelper(process)
        logger.notice("started the daemon at \(helperURL.path, privacy: .public)")
    }

    private func discardHelper() {
        helper?.terminate()
        helper = nil
        live.setHelper(nil)
    }
}

/// The handles a synchronous shutdown needs, behind a lock rather than an actor.
private final class LiveResources: @unchecked Sendable {
    private let lock = NSLock()
    private var helper: HelperProcess?
    private var connection: DaemonConnection?
    private var stopped = false

    var isStopped: Bool {
        lock.lock()
        defer { lock.unlock() }
        return stopped
    }

    func setHelper(_ process: HelperProcess?) {
        lock.lock()
        let previous = helper
        let isStopped = stopped
        helper = isStopped ? nil : process
        lock.unlock()
        if previous !== process { previous?.terminate() }
        if isStopped { process?.terminate() }
    }

    func setConnection(_ value: DaemonConnection?) {
        lock.lock()
        let isStopped = stopped
        connection = isStopped ? nil : value
        lock.unlock()
        if isStopped { value?.close() }
    }

    func stop() {
        lock.lock()
        stopped = true
        let openConnection = connection
        let runningHelper = helper
        connection = nil
        helper = nil
        lock.unlock()
        openConnection?.close()
        runningHelper?.terminate()
    }
}

/// Why the daemon could not be brought up.
enum SupervisorError: Error, LocalizedError {
    case helperMissing
    case helperCannotStart(String)
    case helperExited(String)
    case helperNeverListened(String)

    var errorDescription: String? {
        switch self {
        case .helperMissing:
            "The Token Station service is missing from the app."
        case .helperCannotStart(let detail):
            "The Token Station service would not start: \(detail)"
        case .helperExited(let detail):
            "The Token Station service stopped: \(detail)"
        case .helperNeverListened(let path):
            "The Token Station service did not start listening at \(path)."
        }
    }
}
