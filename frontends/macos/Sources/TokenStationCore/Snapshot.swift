import Foundation

/// The whole state the daemon publishes, as described by `data/snapshot.schema.json`.
public struct Snapshot: Codable, Hashable, Sendable {
    public var schemaVersion: Int
    public var revision: UInt64
    public var generatedAt: Int64
    public var meter: Meter
    public var providers: [ProviderSnapshot]

    public init(
        schemaVersion: Int,
        revision: UInt64,
        generatedAt: Int64,
        meter: Meter,
        providers: [ProviderSnapshot]
    ) {
        self.schemaVersion = schemaVersion
        self.revision = revision
        self.generatedAt = generatedAt
        self.meter = meter
        self.providers = providers
    }

    /// The moment the snapshot was assembled.
    public var generatedDate: Date { Date(timeIntervalSince1970: TimeInterval(generatedAt)) }
}

/// The tray mini-meter: one bar per enabled provider.
public struct Meter: Codable, Hashable, Sendable {
    public var bars: [MeterBar]
    public var level: Level

    public init(bars: [MeterBar], level: Level) {
        self.bars = bars
        self.level = level
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        bars = try container.decodeIfPresent([MeterBar].self, forKey: .bars) ?? []
        level = try container.decodeIfPresent(Level.self, forKey: .level) ?? .normal
    }
}

/// One bar of the tray mini-meter.
public struct MeterBar: Codable, Hashable, Sendable {
    public var provider: ProviderID
    /// 0–100, or `nil` when the provider has no data (draw an empty outline).
    public var percent: Double?
    public var level: Level
    /// Window the bar represents.
    public var windowId: String?

    public init(provider: ProviderID, percent: Double?, level: Level, windowId: String?) {
        self.provider = provider
        self.percent = percent
        self.level = level
        self.windowId = windowId
    }
}

/// One provider's data.
public struct ProviderSnapshot: Codable, Hashable, Sendable, Identifiable {
    public var id: ProviderID
    public var name: String
    public var state: ProviderState
    /// User-facing explanation for non-`ok` states.
    public var message: String?
    /// Plan label, e.g. "Max 20x", "Pro".
    public var plan: String?
    public var windows: [UsageWindow]
    public var credits: Credits?
    public var breakdown: [BreakdownRow]
    public var tokens: TokenReport?
    public var accountTokens: AccountTokens?
    /// Last successful plan-limit refresh.
    public var updatedAt: Int64?

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(ProviderID.self, forKey: .id)
        name = try container.decode(String.self, forKey: .name)
        state = try container.decode(ProviderState.self, forKey: .state)
        message = try container.decodeIfPresent(String.self, forKey: .message)
        plan = try container.decodeIfPresent(String.self, forKey: .plan)
        windows = try container.decodeIfPresent([UsageWindow].self, forKey: .windows) ?? []
        credits = try container.decodeIfPresent(Credits.self, forKey: .credits)
        breakdown = try container.decodeIfPresent([BreakdownRow].self, forKey: .breakdown) ?? []
        tokens = try container.decodeIfPresent(TokenReport.self, forKey: .tokens)
        accountTokens = try container.decodeIfPresent(AccountTokens.self, forKey: .accountTokens)
        updatedAt = try container.decodeIfPresent(Int64.self, forKey: .updatedAt)
    }
}

/// One rate-limit window, e.g. "Session · 34 % · resets 23:30".
public struct UsageWindow: Codable, Hashable, Sendable, Identifiable {
    public var id: String
    public var label: String
    public var kind: WindowKind
    /// 0–100.
    public var usedPercent: Double
    public var resetsAt: Int64?
    public var windowMinutes: UInt32?
    public var level: Level
    /// `oauth`, `statusline`, `app-server`, `http`, `rollout`.
    public var source: String
    public var observedAt: Int64

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        label = try container.decode(String.self, forKey: .label)
        kind = try container.decode(WindowKind.self, forKey: .kind)
        usedPercent = try container.decode(Double.self, forKey: .usedPercent)
        resetsAt = try container.decodeIfPresent(Int64.self, forKey: .resetsAt)
        windowMinutes = try container.decodeIfPresent(UInt32.self, forKey: .windowMinutes)
        level = try container.decodeIfPresent(Level.self, forKey: .level) ?? .normal
        source = try container.decodeIfPresent(String.self, forKey: .source) ?? ""
        observedAt = try container.decodeIfPresent(Int64.self, forKey: .observedAt) ?? 0
    }

    /// When the window rolls over, if the daemon knows.
    public var resetDate: Date? {
        resetsAt.map { Date(timeIntervalSince1970: TimeInterval($0)) }
    }
}

/// Paid overage / credit balance (Claude "extra usage", Codex credits).
public struct Credits: Codable, Hashable, Sendable {
    public var label: String
    public var enabled: Bool
    public var used: Double?
    public var limit: Double?
    public var currency: String?
    public var percent: Double?
    /// Free-form extra line, e.g. "1 reset credit available".
    public var detail: String?
}

/// Share of the weekly window per surface (Claude only: "Claude Code 97 %").
public struct BreakdownRow: Codable, Hashable, Sendable, Identifiable {
    public var key: String
    public var label: String
    public var percent: Double

    public var id: String { key }
}
