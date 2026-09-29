import Foundation
import Testing

@testable import TokenStationCore

@Suite("Daemon connection", .timeLimit(.minutes(1)))
struct DaemonConnectionTests {
    /// Joined into one line: a newline inside a message would end it early.
    private static let snapshotJSON = [
        #"{"schemaVersion":1,"revision":7,"generatedAt":1790596800,"#,
        #""meter":{"bars":[{"provider":"claude","percent":34.0,"level":"normal","#,
        #""windowId":"session"}],"level":"normal"},"#,
        #""providers":[{"id":"claude","name":"Claude Code","state":"ok","windows":[]}]}"#,
    ].joined()

    private static let alertParams = [
        #"{"provider":"claude","providerName":"Claude Code","windowId":"session","#,
        #""windowLabel":"Session","kind":"warning","percent":82.4,"#,
        #""resetsAt":1790614800,"summary":"s","body":"b"}"#,
    ].joined()

    private static func notification(method: String, params: String) -> String {
        #"{"jsonrpc":"2.0","method":"\#(method)","params":\#(params)}"#
    }

    @Test("a method call gets its own answer")
    func answersAMethodCall() async throws {
        let daemon = try FakeDaemon { request, daemon in
            guard let id = request.id else { return }
            switch request.method {
            case "GetVersion":
                daemon.reply(to: id, result: #"{"version":"0.1.0","protocol":1}"#)
            case "GetSnapshot":
                daemon.reply(
                    to: id,
                    result: #"{"revision":7,"snapshot":\#(Self.snapshotJSON)}"#)
            default:
                daemon.fail(id, code: -32601, message: "unknown method")
            }
        }
        defer { daemon.stop() }

        let connection = try await DaemonConnection.open(socketPath: daemon.path)
        let version = try await connection.getVersion()
        #expect(version.version == "0.1.0")
        #expect(version.protocolVersion == 1)

        let envelope = try await connection.getSnapshot()
        #expect(envelope.revision == 7)
        #expect(envelope.snapshot.providers.first?.name == "Claude Code")
        connection.close()
    }

    @Test("answers that come back out of order still reach their caller")
    func matchesResponsesByID() async throws {
        let daemon = try FakeDaemon { request, daemon in
            guard let id = request.id else { return }
            // Everything is held back until the second request arrives, then
            // answered newest first.
            if request.method == "GetHistory" {
                daemon.reply(to: id, result: "[[1790596740,34.0],[1790597040,35.5]]")
            } else {
                Thread.sleep(forTimeInterval: 0.15)
                daemon.reply(to: id, result: "null")
            }
        }
        defer { daemon.stop() }

        let connection = try await DaemonConnection.open(socketPath: daemon.path)
        async let refresh: Void = connection.refresh()
        async let history = connection.getHistory(
            provider: .claude, windowId: "session", since: 0)

        let points = try await history
        try await refresh
        #expect(points.count == 2)
        #expect(points[0].timestamp == 1_790_596_740)
        #expect(points[1].percent == 35.5)
        connection.close()
    }

    @Test("a subscription's pushes arrive in order")
    func deliversNotifications() async throws {
        let daemon = try FakeDaemon { request, daemon in
            guard let id = request.id, request.method == "Subscribe" else { return }
            daemon.reply(to: id, result: #"{"revision":7,"snapshot":\#(Self.snapshotJSON)}"#)
            daemon.send(Self.notification(
                method: "SnapshotChanged",
                params: #"{"revision":8,"snapshot":\#(Self.snapshotJSON)}"#))
            daemon.send(Self.notification(method: "Alert", params: Self.alertParams))
        }
        defer { daemon.stop() }

        let connection = try await DaemonConnection.open(socketPath: daemon.path)
        let envelope = try await connection.subscribe()
        #expect(envelope.revision == 7)

        var seen: [String] = []
        for await notification in connection.notifications {
            switch notification {
            case .snapshotChanged(let revision, _):
                seen.append("snapshot:\(revision)")
            case .alert(let alert):
                seen.append("alert:\(alert.kind.rawValue)")
            case .closed:
                seen.append("closed")
            }
            if seen.count == 2 { break }
        }
        #expect(seen == ["snapshot:8", "alert:warning"])
        connection.close()
    }

    @Test("a refused SetSettings reports the daemon's own problems")
    func surfacesErrorProblems() async throws {
        let daemon = try FakeDaemon { request, daemon in
            guard let id = request.id else { return }
            daemon.fail(
                id,
                code: -32602,
                message: "invalid settings",
                data: #"{"problems":["config.toml is read-only","warning_percent > critical_percent"]}"#)
        }
        defer { daemon.stop() }

        let connection = try await DaemonConnection.open(socketPath: daemon.path)
        await #expect(throws: DaemonError.self) {
            try await connection.setSettings(.object(["alerts": .object([:])]))
        }
        do {
            try await connection.setSettings(.object([:]))
        } catch let error as DaemonError {
            #expect(error.isInvalidParams)
            #expect(error.problems.count == 2)
            #expect(error.problems[0] == "config.toml is read-only")
        }
        connection.close()
    }

    @Test("a daemon that goes away fails the calls waiting on it")
    func failsPendingCallsOnClose() async throws {
        let daemon = try FakeDaemon { request, daemon in
            guard request.id != nil else { return }
            daemon.stop()
        }
        let connection = try await DaemonConnection.open(socketPath: daemon.path)
        await #expect(throws: DaemonTransportError.self) {
            try await connection.refresh()
        }
    }

    @Test("connecting to nothing fails rather than hanging")
    func failsOnAMissingSocket() async {
        await #expect(throws: (any Error).self) {
            _ = try await DaemonConnection.open(socketPath: "/tmp/ts-not-there-\(UUID()).sock")
        }
    }
}
