import Foundation

/// Every line of text the panel shows for a provider.
///
/// The wording lives here rather than in the views so that it can be tested
/// without a window, and so that one provider's rows read the same whichever
/// view draws them.
public enum PanelText {
    /// What a non-`ok` provider says for itself.
    public static func stateMessage(_ provider: ProviderSnapshot) -> String? {
        guard provider.state != .ok else { return nil }
        if let message = provider.message, !message.isEmpty { return message }
        switch provider.state {
        case .loading: return "Loading…"
        case .disabled: return "Turned off"
        case .error: return "Something went wrong"
        default: return nil
        }
    }

    /// `34%` — the only part of a window's reading that takes the level colour.
    public static func windowPercent(_ window: UsageWindow) -> String {
        Formatting.percent(window.usedPercent)
    }

    /// ` · resets in 2 hr 14 min`, separator included, or `nil` when there is
    /// no reset time. The bar already says how full the window is, so this half
    /// of the reading stays secondary however close the limit gets.
    public static func windowReset(_ window: UsageWindow, now: Date = .now) -> String? {
        guard let reset = Formatting.resetsIn(window.resetDate, now: now) else { return nil }
        return Formatting.separator + reset
    }

    /// `Session, 34% used, resets in 2 hours 14 minutes`.
    public static func windowAccessibilityLabel(_ window: UsageWindow, now: Date = .now) -> String {
        Formatting.joinSpoken([
            window.label,
            "\(Formatting.percent(window.usedPercent)) used",
            Formatting.resetsIn(window.resetDate, now: now, width: .wide),
        ])
    }

    /// `Extra usage: Turned off`, `Credits: 1 reset credit available`.
    public static func creditsLine(_ credits: Credits?) -> String? {
        guard let credits else { return nil }
        var parts: [String?] = []
        if credits.enabled, let used = credits.used, used.isFinite {
            let spent = Formatting.currency(used, code: credits.currency)
            if let limit = credits.limit, limit.isFinite {
                parts.append("\(spent) of \(Formatting.currency(limit, code: credits.currency))")
            } else {
                parts.append(spent)
            }
        }
        if let detail = credits.detail, !detail.isEmpty { parts.append(detail) }
        if parts.isEmpty {
            // A balance is only meaningful while extra usage is switched on;
            // with it off, say so rather than show nothing spent.
            guard !credits.enabled else { return nil }
            parts.append("Turned off")
        }
        return "\(credits.label): \(Formatting.join(parts))"
    }

    /// `Claude Code 97% · Cowork 3%`, with the zero rows left out.
    public static func breakdownLine(_ rows: [BreakdownRow]) -> String? {
        let parts = rows
            .filter { $0.percent > 0 }
            .map { "\($0.label) \(Formatting.percent($0.percent))" }
        return parts.isEmpty ? nil : parts.joined(separator: Formatting.separator)
    }

    /// The token lines, in the order they are shown.
    public static func tokenLines(_ provider: ProviderSnapshot) -> [String] {
        var lines: [String] = []
        if let tokens = provider.tokens {
            if let today = totalsLine(prefix: "Today", totals: tokens.today) {
                lines.append(Formatting.join(["This device", today]))
            }
            if let week = totalsLine(prefix: "Last 7 Days", totals: tokens.last7Days) {
                lines.append(week)
            }
        }
        if let account = provider.accountTokens {
            // "tokens" is said once, at the end: with both counts spelled out
            // the line wraps at the panel's 320 pt in the ordinary case.
            let parts = [
                "Today \(Formatting.compactCount(account.today))",
                "Last 7 Days \(Formatting.compactCount(account.last7Days)) tokens",
            ]
            lines.append(Formatting.join(["All devices"] + parts))
        }
        return lines
    }

    /// `Today 43.3M tokens · $31.42`.
    public static func totalsLine(prefix: String, totals: TokenTotals) -> String? {
        guard totals.total > 0 else { return nil }
        var parts = ["\(prefix) \(Formatting.compactCount(totals.total)) tokens"]
        if let cost = totals.costUsd, cost > 0 {
            parts.append(Formatting.currency(cost, code: "USD"))
        }
        return parts.joined(separator: Formatting.separator)
    }

    /// `272.7M · $201.33` for one per-model row.
    public static func modelDetail(_ row: ModelTotals) -> String {
        Formatting.join([
            Formatting.compactCount(row.total),
            (row.costUsd ?? 0) > 0 ? Formatting.currency(row.costUsd ?? 0, code: "USD") : nil,
        ])
    }

    /// `Session · Last 24 Hours`.
    public static func chartCaption(_ window: UsageWindow) -> String {
        let span = window.kind == .weekly ? "Last 7 Days" : "Last 24 Hours"
        return Formatting.join([window.label, span])
    }

    /// The window the meter shows for this provider, else the first one.
    public static func meterWindow(
        for provider: ProviderSnapshot,
        in snapshot: Snapshot
    ) -> UsageWindow? {
        guard !provider.windows.isEmpty else { return nil }
        let bar = snapshot.meter.bars.first { $0.provider == provider.id }
        if let id = bar?.windowId, let match = provider.windows.first(where: { $0.id == id }) {
            return match
        }
        return provider.windows.first
    }

    /// How far back the chart looks, in seconds.
    public static func historySpan(for window: UsageWindow) -> Int64 {
        window.kind == .weekly ? 7 * 24 * 60 * 60 : 24 * 60 * 60
    }
}
