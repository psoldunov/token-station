import Foundation
import Testing
import TokenStationCore
import TokenStationUI

/// Yields until `condition` holds. A test that waits for something that never
/// happens is stopped by its `.timeLimit`, not by a timer in here.
@MainActor
func settle(_ condition: @MainActor () -> Bool) async {
    while !condition() { await Task.yield() }
}

/// Gives every task that is ready to run a chance to. Only for asserting that
/// something did *not* happen: it can hide a bug, never invent a failure.
@MainActor
func flush() async {
    for _ in 0..<50 { await Task.yield() }
}

/// A failure that is not a `DaemonError`, the way a dropped socket is not.
struct TransportDown: LocalizedError {
    var errorDescription: String? { "socket closed" }
}

/// User defaults that belong to one test, so the suite never touches the real
/// preferences.
///
/// A named suite is a file in `~/Library/Preferences`, so the domain is removed
/// again once the test is done with it: without that, every run of the suite
/// leaves one plist per test behind on whoever's machine ran it.
final class IsolatedDefaults {
    /// Every suite this file makes is named for the test target, so the sweep
    /// below can tell its own leftovers from anybody else's preferences.
    private static let prefix = "TokenStationUITests."

    let store: UserDefaults
    private let suiteName: String

    init() throws {
        _ = Self.sweepAbandonedSuites
        let name = "\(Self.prefix)\(UUID().uuidString)"
        suiteName = name
        store = try #require(UserDefaults(suiteName: name))
    }

    deinit {
        store.removePersistentDomain(forName: suiteName)
    }

    /// Removes the suites an earlier run could not.
    ///
    /// The test process can exit while the framework still holds the last test's
    /// harness, and that one's `deinit` never runs — one plist per run, for ever.
    /// It runs once, before this run has made a suite of its own, and only takes
    /// files that already existed by then.
    private static let sweepAbandonedSuites: Void = {
        let started = Date.now
        guard let directory = FileManager.default.urls(
            for: .libraryDirectory, in: .userDomainMask).first?
            .appending(path: "Preferences"),
            let entries = try? FileManager.default.contentsOfDirectory(
                at: directory, includingPropertiesForKeys: [.contentModificationDateKey])
        else { return }

        for entry in entries where entry.lastPathComponent.hasPrefix(prefix)
            && entry.pathExtension == "plist" {
            let modified = try? entry.resourceValues(forKeys: [.contentModificationDateKey])
                .contentModificationDate
            guard let modified, modified < started else { continue }
            let name = entry.deletingPathExtension().lastPathComponent
            // Through the suite's own defaults, the way a live one is dropped:
            // `standard` will not remove a domain this process never opened.
            UserDefaults(suiteName: name)?.removePersistentDomain(forName: name)
            // Emptying a domain leaves its plist behind as an empty dictionary,
            // so the file itself goes too. Nothing holds it: the run that made
            // it is over.
            try? FileManager.default.removeItem(at: entry)
        }
    }()
}

/// User defaults that belong to one test. See ``IsolatedDefaults``.
func isolatedDefaults() throws -> IsolatedDefaults {
    try IsolatedDefaults()
}

/// The snapshots in `data/fixtures`, which the daemon and every front end share.
enum RepositoryFixtures {
    static func snapshot(named name: String) throws -> Snapshot {
        let url = URL(filePath: #filePath)
            .deletingLastPathComponent()  // TokenStationUITests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // macos
            .deletingLastPathComponent()  // frontends
            .deletingLastPathComponent()  // repository root
            .appending(path: "data/fixtures/snapshot-\(name).json")
        return try JSONDecoder().decode(Snapshot.self, from: Data(contentsOf: url))
    }
}

/// The app model wired to a fake session, with every callback counted.
@MainActor
final class AppHarness {
    let session = FakeSession()
    let model: AppModel
    /// Held so the suite outlives the model that reads it.
    let defaults: IsolatedDefaults
    private(set) var alerts: [Alert] = []
    private(set) var connectedCount = 0
    private(set) var notificationsWantedCount = 0
    private var drains = 0

    init(defaults: IsolatedDefaults? = nil) throws {
        let owned = try defaults ?? isolatedDefaults()
        self.defaults = owned
        model = AppModel(session: session, defaults: owned.store)
        model.onAlert = { [weak self] in self?.alerts.append($0) }
        model.onConnected = { [weak self] in self?.connectedCount += 1 }
        model.onNotificationsWanted = { [weak self] in self?.notificationsWantedCount += 1 }
    }

    /// Starts the model and waits until it has asked the session to begin.
    func start() async {
        model.start()
        await settle { session.startCount == 1 }
    }

    /// Returns once every event emitted so far has been applied, by riding a
    /// marker alert through the same ordered stream.
    func drain() async {
        drains += 1
        let marker = "drain-\(drains)"
        session.emit(.alert(Alert(
            provider: .claude,
            providerName: "Claude Code",
            windowId: marker,
            windowLabel: marker,
            kind: .warning,
            percent: 0)))
        await settle { alerts.contains { $0.windowId == marker } }
    }

    /// The alerts the panel forwarded, without the markers `drain` sends.
    var forwardedAlerts: [Alert] {
        alerts.filter { !$0.windowId.hasPrefix("drain-") }
    }
}
