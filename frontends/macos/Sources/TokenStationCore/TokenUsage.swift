/// Token totals in disjoint buckets.
///
/// `total = input + cacheRead + cacheWrite + output`; `reasoning` is a subset of
/// `output`, shown for information only.
public struct TokenTotals: Codable, Hashable, Sendable {
    public var input: UInt64
    public var output: UInt64
    public var cacheRead: UInt64
    public var cacheWrite: UInt64
    public var reasoning: UInt64
    public var total: UInt64
    /// API-equivalent cost of the priced part, if any model was priced.
    public var costUsd: Double?
    /// Tokens whose model had no known price.
    public var unpricedTokens: UInt64
    public var requests: UInt64
}

/// One model's slice of the last seven days.
public struct ModelTotals: Codable, Hashable, Sendable, Identifiable {
    public var model: String
    public var input: UInt64
    public var output: UInt64
    public var cacheRead: UInt64
    public var cacheWrite: UInt64
    public var reasoning: UInt64
    public var total: UInt64
    public var costUsd: Double?
    public var unpricedTokens: UInt64
    public var requests: UInt64

    public var id: String { model }
}

/// Token usage recorded in this machine's local CLI logs.
public struct TokenReport: Codable, Hashable, Sendable {
    public var today: TokenTotals
    public var last7Days: TokenTotals
    /// Last 7 days, most expensive first.
    public var byModel: [ModelTotals]
    public var updatedAt: Int64

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        today = try container.decode(TokenTotals.self, forKey: .today)
        last7Days = try container.decode(TokenTotals.self, forKey: .last7Days)
        byModel = try container.decodeIfPresent([ModelTotals].self, forKey: .byModel) ?? []
        updatedAt = try container.decodeIfPresent(Int64.self, forKey: .updatedAt) ?? 0
    }
}

/// One day of account-wide usage.
public struct DailyTokens: Codable, Hashable, Sendable, Identifiable {
    /// Local calendar date, `YYYY-MM-DD`.
    public var date: String
    public var tokens: UInt64

    public var id: String { date }
}

/// Account-wide token usage reported by the provider backend (all devices).
public struct AccountTokens: Codable, Hashable, Sendable {
    public var today: UInt64
    public var last7Days: UInt64
    public var lifetime: UInt64?
    /// Oldest first, at most 30 entries.
    public var daily: [DailyTokens]
    public var updatedAt: Int64

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        today = try container.decode(UInt64.self, forKey: .today)
        last7Days = try container.decode(UInt64.self, forKey: .last7Days)
        lifetime = try container.decodeIfPresent(UInt64.self, forKey: .lifetime)
        daily = try container.decodeIfPresent([DailyTokens].self, forKey: .daily) ?? []
        updatedAt = try container.decodeIfPresent(Int64.self, forKey: .updatedAt) ?? 0
    }
}
