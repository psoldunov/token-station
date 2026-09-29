import Foundation
import Observation
import TokenStationCore

/// One history series, keyed by the provider and window it belongs to.
public struct HistoryKey: Hashable, Sendable {
    public var provider: ProviderID
    public var windowId: String

    public init(provider: ProviderID, windowId: String) {
        self.provider = provider
        self.windowId = windowId
    }
}

/// Everything the menu bar icon, the panel and the settings window read.
@Observable
@MainActor
public final class AppModel {
    /// What the panel is showing.
    public enum Phase: Equatable, Sendable {
        case connecting
        case running
        case failed(SessionFailure, String)
    }

    /// The `Show Percentage in Menu Bar` preference, stored in user defaults.
    public static let showPercentageDefaultsKey = "ShowPercentageInMenuBar"
    /// Floor between two rounds of `GetHistory` while the panel stays open.
    private static let historyMinimumInterval: TimeInterval = 60

    public private(set) var phase: Phase = .connecting
    public private(set) var snapshot: Snapshot?
    public private(set) var history: [HistoryKey: [HistoryPoint]] = [:]
    public private(set) var isRefreshing = false
    public private(set) var daemonVersion: String?

    /// Why the last refresh did not go through, if it did not.
    public private(set) var refreshFailure: String?

    /// Set by the app so that an `Alert` reaches Notification Center.
    public var onAlert: (@MainActor (Alert) -> Void)?
    /// Set by the app; called the first time the daemon answers.
    public var onConnected: (@MainActor () -> Void)?
    /// Set by the app; called when something turns alerts on.
    public var onNotificationsWanted: (@MainActor () -> Void)?

    public var showPercentageInMenuBar: Bool {
        didSet {
            guard showPercentageInMenuBar != oldValue else { return }
            defaults.set(showPercentageInMenuBar, forKey: Self.showPercentageDefaultsKey)
        }
    }

    private let session: any DaemonSession
    private let defaults: UserDefaults
    private var pump: Task<Void, Never>?
    private var historyTask: Task<Void, Never>?
    private var lastHistoryFetch: Date?

    public init(session: any DaemonSession, defaults: UserDefaults = .standard) {
        self.session = session
        self.defaults = defaults
        self.showPercentageInMenuBar = defaults.bool(forKey: Self.showPercentageDefaultsKey)
    }

    /// Starts listening to the session. Call once, at launch.
    public func start() {
        guard pump == nil else { return }
        pump = Task { [session] in
            await session.start()
            for await event in session.events {
                self.apply(event)
            }
        }
    }

    private func apply(_ event: SessionEvent) {
        switch event {
        case .connecting:
            // A drop after the first snapshot keeps the panel's contents on
            // screen; only a cold start has nothing to show.
            if snapshot == nil { phase = .connecting }
        case .connected(let snapshot, let version):
            self.snapshot = snapshot
            daemonVersion = version
            phase = .running
            refreshFailure = nil
            fetchHistory(force: true)
            onConnected?()
        case .snapshot(let snapshot):
            self.snapshot = snapshot
            phase = .running
            fetchHistory(force: false)
        case .alert(let alert):
            onAlert?(alert)
        case .failed(let kind, let message):
            phase = .failed(kind, message)
        }
    }

    /// `Refresh Now`, and the header's refresh button.
    public func refresh() {
        guard !isRefreshing else { return }
        isRefreshing = true
        Task { [session] in
            defer { self.isRefreshing = false }
            do {
                try await session.refresh()
                // The snapshot that follows says the rest; this only clears the
                // footer's note about the attempt before it.
                self.refreshFailure = nil
            } catch {
                self.refreshFailure = error.localizedDescription
            }
        }
    }

    /// Tries the daemon again after a failure.
    public func retry() {
        phase = .connecting
        refreshFailure = nil
        Task { [session] in await session.retry() }
    }

    /// Something turned alerts on, so the app should ask for permission now.
    public func notificationsWanted() {
        onNotificationsWanted?()
    }

    /// The panel opened: start the countdowns and pull the charts.
    public func panelDidOpen() {
        fetchHistory(force: true)
    }

    /// Pulls one series per provider, for the window its meter bar shows.
    private func fetchHistory(force: Bool) {
        guard let snapshot else { return }
        if !force, let last = lastHistoryFetch,
           Date.now.timeIntervalSince(last) < Self.historyMinimumInterval {
            return
        }
        lastHistoryFetch = .now

        let requests: [(HistoryKey, Int64)] = snapshot.providers.compactMap { provider in
            guard let window = PanelText.meterWindow(for: provider, in: snapshot) else {
                return nil
            }
            let since = Int64(Date.now.timeIntervalSince1970)
                - PanelText.historySpan(for: window)
            return (HistoryKey(provider: provider.id, windowId: window.id), since)
        }
        guard !requests.isEmpty else { return }

        historyTask?.cancel()
        historyTask = Task { [session] in
            for (key, since) in requests {
                guard !Task.isCancelled else { return }
                guard let points = try? await session.history(
                    provider: key.provider, windowId: key.windowId, since: since)
                else { continue }
                self.history[key] = points
            }
        }
    }

    /// The series drawn under one provider, if it has been fetched.
    public func historyPoints(for provider: ProviderSnapshot) -> [HistoryPoint] {
        guard let snapshot,
              let window = PanelText.meterWindow(for: provider, in: snapshot)
        else { return [] }
        return history[HistoryKey(provider: provider.id, windowId: window.id)] ?? []
    }

    /// True once the daemon answered but nothing is turned on.
    public var hasNoProviders: Bool {
        guard case .running = phase, let snapshot else { return false }
        return snapshot.providers.isEmpty
    }

    // MARK: - Settings

    public func loadSettings() async throws -> SettingsDocument {
        SettingsDocument(json: try await session.settings())
    }

    public func saveSettings(_ document: SettingsDocument) async throws {
        try await session.applySettings(document.json)
    }
}
