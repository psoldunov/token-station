import OSLog
import TokenStationCore
import UserNotifications

/// Shows the daemon's alerts in Notification Center.
///
/// The text is written here rather than taken from the daemon's own `summary`
/// and `body`, so a notification reads the way the system's own do: a title in
/// title case, a body that is one sentence. Alerts about one provider carry the
/// same thread identifier, so Notification Center groups them.
///
/// Alerts raised while nobody was subscribed are delivered in a burst the moment
/// the app subscribes, which is sooner than the authorization status comes back.
/// Anything that arrives before the status is known is held rather than dropped.
@MainActor
final class NotificationPresenter: NSObject, UNUserNotificationCenterDelegate {
    /// How many alerts are held while the authorization status is unknown. The
    /// daemon keeps at most sixteen of its own, so there is no point in more.
    private static let bufferLimit = 16

    private enum Authorization {
        /// Not asked the system yet.
        case unknown
        /// The system has never asked the user.
        case notDetermined
        case allowed
        case denied
    }

    private let logger = Logger(
        subsystem: SupportPaths.bundleIdentifier, category: "notifications")
    private var authorization: Authorization = .unknown
    private var buffered: [Alert] = []
    private var hasAsked = false

    /// True only when the app runs from a bundle, which is what
    /// `UNUserNotificationCenter` needs; a bare `swift run` has no bundle id.
    private var isAvailable: Bool { Bundle.main.bundleIdentifier != nil }

    /// Claims the delegate. Call from `applicationWillFinishLaunching`, before
    /// the system can deliver a notification the app was launched from.
    func attach() {
        guard isAvailable else { return }
        UNUserNotificationCenter.current().delegate = self
    }

    /// Reads the status the user already gave, without prompting.
    func resolveAuthorization() async {
        guard isAvailable else {
            authorization = .denied
            buffered.removeAll()
            return
        }
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        apply(settings.authorizationStatus)
    }

    /// Prompts, once, and only when something wants notifications on.
    func requestAuthorization() async {
        guard isAvailable, !hasAsked else { return }
        guard authorization == .notDetermined || authorization == .unknown else { return }
        hasAsked = true
        do {
            let granted = try await UNUserNotificationCenter.current()
                .requestAuthorization(options: [.alert, .sound])
            authorization = granted ? .allowed : .denied
        } catch {
            authorization = .denied
            logger.error(
                "notifications were refused: \(error.localizedDescription, privacy: .public)")
        }
        flush()
    }

    private func apply(_ status: UNAuthorizationStatus) {
        switch status {
        case .authorized, .provisional, .ephemeral:
            authorization = .allowed
        case .denied:
            authorization = .denied
        case .notDetermined:
            authorization = .notDetermined
        @unknown default:
            authorization = .denied
        }
        flush()
    }

    func post(_ alert: Alert) {
        switch authorization {
        case .allowed:
            deliver(alert)
        case .denied:
            break
        case .unknown, .notDetermined:
            // Held: the status is about to arrive, or the user is about to be
            // asked, and an alert is worth more late than not at all.
            buffered.append(alert)
            if buffered.count > Self.bufferLimit { buffered.removeFirst() }
        }
    }

    /// Delivers what was held, or throws it away once the answer is no.
    private func flush() {
        let held = buffered
        buffered.removeAll()
        guard authorization == .allowed else { return }
        for alert in held { deliver(alert) }
    }

    private func deliver(_ alert: Alert) {
        let content = UNMutableNotificationContent()
        content.title = AlertText.title(alert)
        content.body = AlertText.body(alert)
        content.threadIdentifier = AlertText.threadIdentifier(alert)
        // A limit running out is worth a sound; a limit coming back is not.
        // `.timeSensitive` would need an entitlement this app does not carry.
        if alert.kind != .reset { content.sound = .default }

        let request = UNNotificationRequest(
            identifier: UUID().uuidString, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request) { [logger] error in
            guard let error else { return }
            logger.error(
                "could not show a notification: \(error.localizedDescription, privacy: .public)")
        }
    }

    // MARK: - UNUserNotificationCenterDelegate

    /// A menu bar app is often the active one; its notifications should still show.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    /// Nothing is attached to a notification, so opening one just opens the app.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {}
}
