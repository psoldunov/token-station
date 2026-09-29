/// Which window the menu bar meter shows.
public enum MeterWindowSetting: String, CaseIterable, Sendable {
    case mostConstrained = "most_constrained"
    case session
    case weekly

    /// Title-case label, the way a macOS pop-up button writes its options.
    public var title: String {
        switch self {
        case .mostConstrained: "Most Constrained"
        case .session: "Session"
        case .weekly: "Weekly"
        }
    }
}

/// The daemon's config document, edited one path at a time.
///
/// `SetSettings` replaces the whole config, so the document is kept exactly as
/// the daemon sent it and only the edited path is replaced. A key a newer daemon
/// added, or one hand-written into `config.toml`, therefore survives a save from
/// this settings window.
public struct SettingsDocument: Hashable, Sendable {
    /// Bounds the daemon enforces, repeated here so a control cannot offer a
    /// value the daemon would refuse.
    public enum Bounds {
        public static let thresholdPercent: ClosedRange<Double> = 1...100
        public static let thresholdStep: Double = 5
    }

    public private(set) var json: JSONValue

    public init(json: JSONValue = .object([:])) {
        self.json = json
    }

    // MARK: - Reading

    public func bool(_ path: String, default fallback: Bool) -> Bool {
        json[path: path]?.boolValue ?? fallback
    }

    public func int(_ path: String, default fallback: Int) -> Int {
        json[path: path]?.intValue ?? fallback
    }

    public func double(_ path: String, default fallback: Double) -> Double {
        json[path: path]?.doubleValue ?? fallback
    }

    public func string(_ path: String, default fallback: String) -> String {
        json[path: path]?.stringValue ?? fallback
    }

    // MARK: - Writing

    public func setting(_ path: String, _ value: Bool) -> Self {
        Self(json: json.setting(path: path, to: .bool(value)))
    }

    public func setting(_ path: String, _ value: Int) -> Self {
        Self(json: json.setting(path: path, to: .number(Double(value))))
    }

    public func setting(_ path: String, _ value: Double) -> Self {
        Self(json: json.setting(path: path, to: .number(value)))
    }

    public func setting(_ path: String, _ value: String) -> Self {
        Self(json: json.setting(path: path, to: .string(value)))
    }

    // MARK: - The keys the settings window edits

    public var claudeEnabled: Bool { bool("claude.enabled", default: true) }
    public var codexEnabled: Bool { bool("codex.enabled", default: true) }
    public var notify: Bool { bool("alerts.notify", default: true) }
    public var notifyOnReset: Bool { bool("alerts.notify_on_reset", default: false) }
    public var pricingAutoUpdate: Bool { bool("pricing.auto_update", default: true) }

    public var warningPercent: Double {
        clampThreshold(double("alerts.warning_percent", default: 80))
    }

    public var criticalPercent: Double {
        clampThreshold(double("alerts.critical_percent", default: 95))
    }

    public var limitsIntervalSeconds: Int {
        int("general.limits_interval_secs", default: 600)
    }

    public var meterWindow: MeterWindowSetting {
        MeterWindowSetting(rawValue: string("meter.window", default: "most_constrained"))
            ?? .mostConstrained
    }

    /// Moves the warning threshold, pushing the critical one up when it would pass it.
    public func settingWarningPercent(_ value: Double) -> Self {
        let warning = clampThreshold(value)
        var next = setting("alerts.warning_percent", warning)
        if warning > next.criticalPercent {
            next = next.setting("alerts.critical_percent", warning)
        }
        return next
    }

    /// Moves the critical threshold, pulling the warning one down when it would pass it.
    public func settingCriticalPercent(_ value: Double) -> Self {
        let critical = clampThreshold(value)
        var next = setting("alerts.critical_percent", critical)
        if critical < next.warningPercent {
            next = next.setting("alerts.warning_percent", critical)
        }
        return next
    }

    private func clampThreshold(_ value: Double) -> Double {
        let bounds = Bounds.thresholdPercent
        guard value.isFinite else { return bounds.lowerBound }
        return min(max(value.rounded(), bounds.lowerBound), bounds.upperBound)
    }
}
