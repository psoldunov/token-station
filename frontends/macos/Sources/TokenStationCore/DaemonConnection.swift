import Foundation

/// One JSON-RPC conversation with the daemon over its Unix socket.
///
/// Requests on one connection may be answered out of order, so every call waits
/// on its own id. Notifications arrive on `notifications`, in the order the
/// daemon sent them.
public actor DaemonConnection {
    private let channel: UnixSocketChannel
    private let lines: AsyncStream<LineEvent>
    private var pending: [Int: CheckedContinuation<JSONValue, any Error>] = [:]
    private var nextID = 1
    private var pump: Task<Void, Never>?
    private var isClosed = false

    private let notificationContinuation: AsyncStream<DaemonNotification>.Continuation
    /// Everything the daemon pushes after `Subscribe`, plus a final `.closed`.
    nonisolated public let notifications: AsyncStream<DaemonNotification>

    /// Connects to `socketPath` and starts reading.
    public static func open(socketPath: String) async throws -> DaemonConnection {
        let connection = try DaemonConnection(socketPath: socketPath)
        await connection.startPump()
        return connection
    }

    private init(socketPath: String) throws {
        channel = try UnixSocketChannel(path: socketPath)

        let (notificationStream, notificationContinuation) =
            AsyncStream<DaemonNotification>.makeStream()
        self.notifications = notificationStream
        self.notificationContinuation = notificationContinuation

        let (lineStream, lineContinuation) = AsyncStream<LineEvent>.makeStream()
        lines = lineStream

        // The framer lives outside the actor so that lines keep the order the
        // socket delivered them; one task drains the stream in that same order.
        let framer = FramerBox()
        channel.start(
            onBytes: { bytes in
                switch framer.append(bytes) {
                case .success(let lines):
                    for line in lines { lineContinuation.yield(.line(line)) }
                case .failure(let error):
                    lineContinuation.yield(.closed(reason: error.localizedDescription))
                    lineContinuation.finish()
                }
            },
            onClose: { reason in
                lineContinuation.yield(.closed(reason: reason))
                lineContinuation.finish()
            }
        )
    }

    private func startPump() {
        pump = Task { [lines] in
            for await event in lines {
                switch event {
                case .line(let data):
                    self.receive(line: data)
                case .closed(let reason):
                    self.handleClose(reason: reason)
                }
            }
        }
    }

    // MARK: - Methods

    public func getVersion() async throws -> DaemonVersion {
        try decode(DaemonVersion.self, from: try await call("GetVersion"))
    }

    public func getSnapshot() async throws -> SnapshotEnvelope {
        try decode(SnapshotEnvelope.self, from: try await call("GetSnapshot"))
    }

    /// Asks for the current snapshot and for every later change on this connection.
    public func subscribe() async throws -> SnapshotEnvelope {
        try decode(SnapshotEnvelope.self, from: try await call("Subscribe"))
    }

    public func refresh() async throws {
        _ = try await call("Refresh")
    }

    public func getHistory(
        provider: ProviderID,
        windowId: String,
        since: Int64
    ) async throws -> [HistoryPoint] {
        let params = JSONValue.object([
            "provider": .string(provider.rawValue),
            "windowId": .string(windowId),
            "since": .number(Double(since)),
        ])
        let rows = try decode([[Double]].self, from: try await call("GetHistory", params: params))
        return rows.compactMap { row in
            guard row.count >= 2 else { return nil }
            return HistoryPoint(timestamp: Int64(row[0]), percent: row[1])
        }
    }

    public func getSettings() async throws -> JSONValue {
        try await call("GetSettings")
    }

    public func setSettings(_ settings: JSONValue) async throws {
        _ = try await call("SetSettings", params: .object(["settings": settings]))
    }

    /// Closes the socket; every waiting call fails with `.connectionClosed`.
    ///
    /// Nonisolated so that a supervisor can hang up without waiting for a
    /// request already queued on this actor to finish.
    nonisolated public func close() {
        channel.close()
    }

    // MARK: - Plumbing

    private func call(_ method: String, params: JSONValue? = nil) async throws -> JSONValue {
        if isClosed { throw DaemonTransportError.connectionClosed }
        let id = nextID
        nextID += 1

        let request = JSONRPC.Request(id: id, method: method, params: params)
        var line = try JSONEncoder().encode(request)
        line.append(UInt8(ascii: "\n"))

        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                // Cancelled before the request even went out: do not put a
                // request on the wire whose answer nobody is waiting for.
                guard !Task.isCancelled else {
                    continuation.resume(throwing: CancellationError())
                    return
                }
                pending[id] = continuation
                channel.write(line)
            }
        } onCancel: {
            Task { await self.abandon(id: id) }
        }
    }

    /// Fails one waiting call because its task was cancelled.
    private func abandon(id: Int) {
        guard let continuation = pending.removeValue(forKey: id) else { return }
        continuation.resume(throwing: CancellationError())
    }

    private func receive(line: Data) {
        let message: JSONRPC.Message
        do {
            message = try JSONDecoder().decode(JSONRPC.Message.self, from: line)
        } catch {
            // A line that is not a JSON-RPC message means the stream is no
            // longer trustworthy. Hanging up is what stops every call waiting
            // on an answer that is never going to parse.
            let reason = DaemonTransportError.malformedMessage(
                error.localizedDescription).localizedDescription
            handleClose(reason: reason)
            return
        }

        if let method = message.method, message.id == nil {
            deliverNotification(method: method, params: message.params)
            return
        }
        guard let id = message.id, let continuation = pending.removeValue(forKey: id) else {
            return
        }
        if let error = message.error {
            continuation.resume(throwing: error)
        } else {
            continuation.resume(returning: message.result ?? .null)
        }
    }

    private func deliverNotification(method: String, params: JSONValue?) {
        guard let params else { return }
        switch method {
        case "SnapshotChanged":
            guard let envelope = try? decode(SnapshotEnvelope.self, from: params) else { return }
            notificationContinuation.yield(
                .snapshotChanged(revision: envelope.revision, snapshot: envelope.snapshot))
        case "Alert":
            guard let alert = try? decode(Alert.self, from: params) else { return }
            notificationContinuation.yield(.alert(alert))
        default:
            // A newer daemon may push something this build does not know; the
            // panel keeps working on the notifications it does understand.
            break
        }
    }

    private func handleClose(reason: String?) {
        guard !isClosed else { return }
        isClosed = true
        let waiting = pending
        pending.removeAll()
        for continuation in waiting.values {
            continuation.resume(throwing: DaemonTransportError.connectionClosed)
        }
        notificationContinuation.yield(.closed(reason: reason))
        notificationContinuation.finish()
        pump?.cancel()
        // Every path into here ends the conversation — end of file, a framing
        // overflow, a line that would not parse — and the descriptor has to go
        // with it rather than wait for the process to exit.
        channel.close()
    }

    /// Re-decodes a JSON value into a concrete type.
    ///
    /// The value is wrapped in an array first, because a top-level fragment is
    /// not something every `JSONEncoder` will write.
    private func decode<T: Decodable>(_ type: T.Type, from value: JSONValue) throws -> T {
        let data = try JSONEncoder().encode([value])
        guard let decoded = try JSONDecoder().decode([T].self, from: data).first else {
            throw DaemonTransportError.malformedMessage("empty result for \(T.self)")
        }
        return decoded
    }
}

/// What the read loop hands to the actor.
private enum LineEvent: Sendable {
    case line(Data)
    case closed(reason: String?)
}

/// A `LineFramer` usable from the socket queue.
private final class FramerBox: @unchecked Sendable {
    private let lock = NSLock()
    private var framer = LineFramer()

    func append(_ bytes: Data) -> Result<[Data], LineFramer.LineTooLongError> {
        lock.lock()
        defer { lock.unlock() }
        do {
            return .success(try framer.append(bytes))
        } catch let error as LineFramer.LineTooLongError {
            return .failure(error)
        } catch {
            return .success([])
        }
    }
}
