import AppKit
import TokenStationCore
import TokenStationUI

/// Owns the daemon supervisor and the model, and handles launch, wake and quit.
///
/// SwiftUI makes the delegate itself, so the app's one model is built here and
/// read back out by the scenes rather than the other way round.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let supervisor: DaemonSupervisor
    let model: AppModel

    private let notifications = NotificationPresenter()
    private var hasAskedAboutNotifications = false

    override init() {
        let supervisor = DaemonSupervisor()
        self.supervisor = supervisor
        self.model = AppModel(session: supervisor)
        super.init()
    }

    func applicationWillFinishLaunching(_ notification: Notification) {
        // Claimed before the system can hand over a notification the app was
        // launched from, which is earlier than `didFinishLaunching`.
        notifications.attach()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        model.onAlert = { [notifications] alert in notifications.post(alert) }
        model.onConnected = { [weak self] in self?.askAboutNotificationsIfWanted() }
        model.onNotificationsWanted = { [weak self] in self?.askAboutNotifications() }

        // The daemon flushes the alerts it kept the moment the app subscribes,
        // so the authorization status has to be settled before that happens.
        Task { [notifications, model] in
            await notifications.resolveAuthorization()
            model.start()
        }

        NSWorkspace.shared.notificationCenter.addObserver(
            self,
            selector: #selector(systemDidWake),
            name: NSWorkspace.didWakeNotification,
            object: nil
        )
    }

    func applicationWillTerminate(_ notification: Notification) {
        NSWorkspace.shared.notificationCenter.removeObserver(self)
        // Synchronously, here and now: a task scheduled from this method is not
        // guaranteed to run before the process goes away, and the helper would
        // then outlive the app until its stdin pipe is collected.
        supervisor.shutdownNow()
    }

    @objc
    private func systemDidWake(_: Notification) {
        // Everything on screen is as old as the sleep was long.
        model.refresh()
    }

    /// Asks for notification permission once, and only while alerts are on.
    private func askAboutNotificationsIfWanted() {
        guard !hasAskedAboutNotifications else { return }
        hasAskedAboutNotifications = true
        Task { [notifications, model] in
            guard let document = try? await model.loadSettings(), document.notify else { return }
            await notifications.requestAuthorization()
        }
    }

    /// The user just turned alerts on, so now is the moment to ask.
    private func askAboutNotifications() {
        hasAskedAboutNotifications = true
        Task { [notifications] in await notifications.requestAuthorization() }
    }
}
