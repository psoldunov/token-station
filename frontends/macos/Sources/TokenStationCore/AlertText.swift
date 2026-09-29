import Foundation

/// How an `Alert` is worded in Notification Center.
///
/// The daemon sends its own `summary`/`body` for clients that do not word
/// alerts themselves; macOS writes them the way the system's own notifications
/// read — a title in title case, a body that is one short sentence.
public enum AlertText {
    /// `Claude Code Session at 82%`, or `Claude Code Session Reset`.
    public static func title(_ alert: Alert) -> String {
        let subject = "\(alert.providerName) \(alert.windowLabel)"
            .trimmingCharacters(in: .whitespaces)
        switch alert.kind {
        case .reset:
            return "\(subject) Reset"
        case .warning, .critical, .unknown:
            return "\(subject) at \(Formatting.percent(alert.percent))"
        }
    }

    /// `Resets in 2 hr 14 min.` or `Usage is back to 1%.`
    public static func body(_ alert: Alert, now: Date = .now) -> String {
        switch alert.kind {
        case .reset:
            return "Usage is back to \(Formatting.percent(alert.percent))."
        case .warning, .critical, .unknown:
            let reset = alert.resetsAt.map { Date(timeIntervalSince1970: TimeInterval($0)) }
            guard let sentence = Formatting.resetsIn(reset, now: now) else {
                return "\(alert.windowLabel) usage is at \(Formatting.percent(alert.percent))."
            }
            return "\(sentence.prefix(1).uppercased())\(sentence.dropFirst())."
        }
    }

    /// Notifications for one provider stack together, as Apple's own do.
    public static func threadIdentifier(_ alert: Alert) -> String {
        "provider.\(alert.provider.rawValue)"
    }
}
