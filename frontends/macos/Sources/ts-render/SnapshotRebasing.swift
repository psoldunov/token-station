import Foundation
import TokenStationCore

/// Moves a fixture's clock to now.
///
/// The fixtures were written on the day they were captured, so drawing them
/// unchanged gives a panel full of "resetting now" and "Updated 21 hr ago".
/// Shifting every timestamp by the same amount keeps the fixture's own
/// relationships and makes the render show what a user would actually read.
enum SnapshotRebasing {
    static func toNow(_ snapshot: Snapshot, now: Date = .now) -> Snapshot {
        let offset = Int64(now.timeIntervalSince1970) - snapshot.generatedAt
        var rebased = snapshot
        rebased.generatedAt += offset
        rebased.providers = snapshot.providers.map { provider in
            var copy = provider
            copy.updatedAt = provider.updatedAt.map { $0 + offset }
            copy.windows = provider.windows.map { window in
                var window = window
                window.resetsAt = window.resetsAt.map { $0 + offset }
                window.observedAt += offset
                return window
            }
            copy.tokens = provider.tokens.map { tokens in
                var tokens = tokens
                tokens.updatedAt += offset
                return tokens
            }
            copy.accountTokens = provider.accountTokens.map { account in
                var account = account
                account.updatedAt += offset
                return account
            }
            return copy
        }
        return rebased
    }
}
