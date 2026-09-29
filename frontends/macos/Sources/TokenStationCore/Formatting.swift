import Foundation

/// Every number, duration and money string the panel shows.
///
/// All of it goes through Foundation's format styles, so the user's own locale
/// decides the digits, the decimal mark, the grouping, the unit names and the
/// currency symbol. Nothing here writes a separator or a suffix by hand.
public enum Formatting {
    /// The middle dot macOS menus use between parts of one line.
    public static let separator = " · "

    /// `34%`, rounded to whole percent like the menu bar extras do.
    public static func percent(_ value: Double) -> String {
        guard value.isFinite else { return "—" }
        return (value / 100).formatted(.percent.precision(.fractionLength(0)))
    }

    /// `43.3M`, one fraction digit while it still carries information.
    public static func compactCount(_ value: UInt64) -> String {
        Double(value).formatted(.number.notation(.compactName).precision(.fractionLength(0...1)))
    }

    /// Money in the daemon's currency; an unknown code falls back to two digits.
    public static func currency(_ value: Double, code: String?) -> String {
        guard value.isFinite else { return "—" }
        let normalized = (code ?? "USD").uppercased()
        guard normalized.count == 3, normalized.allSatisfy(\.isLetter) else {
            return value.formatted(.number.precision(.fractionLength(2)))
        }
        return value.formatted(.currency(code: normalized))
    }

    /// `2 hr 14 min` — two units at most, and only ones its size warrants.
    public static func duration(_ seconds: TimeInterval, width: DurationWidth = .short) -> String {
        let clamped = max(0, seconds)
        return Duration.seconds(clamped).formatted(
            .units(
                allowed: allowedUnits(for: clamped),
                width: width.styleWidth,
                maximumUnitCount: 2
            ))
    }

    /// Days go with hours, hours with minutes, and nothing smaller is offered.
    ///
    /// Handing the style all three units and a count of two makes it pick the
    /// two largest that are not zero, which is not the same thing: six days and
    /// twenty-three minutes came out as `6 days 23 min`, where the minutes are
    /// noise at that distance and read like hours beside `3 days 14 hr`. Picking
    /// the pair by magnitude drops the tail instead of skipping the gap.
    private static func allowedUnits(
        for seconds: TimeInterval
    ) -> Set<Duration.UnitsFormatStyle.Unit> {
        if seconds >= 86_400 { return [.days, .hours] }
        if seconds >= 3_600 { return [.hours, .minutes] }
        return [.minutes]
    }

    /// How a duration is spelled: short for the panel, wide for a screen reader.
    public enum DurationWidth: Sendable {
        case short
        case wide

        var styleWidth: Duration.UnitsFormatStyle.UnitWidth {
            switch self {
            case .short: .condensedAbbreviated
            case .wide: .wide
            }
        }
    }

    /// `resets in 2 hr 14 min`, or `nil` when the daemon has no reset time.
    public static func resetsIn(
        _ date: Date?,
        now: Date = .now,
        width: DurationWidth = .short
    ) -> String? {
        guard let date else { return nil }
        let remaining = date.timeIntervalSince(now)
        if remaining <= 0 { return "resetting now" }
        if remaining < 60 { return "resets in under a minute" }
        return "resets in \(duration(remaining, width: width))"
    }

    /// `Updated 2 min ago`, the way a menu bar extra dates its own contents.
    public static func updatedAgo(_ date: Date, now: Date = .now) -> String {
        let elapsed = now.timeIntervalSince(date)
        if elapsed < 60 { return "Updated just now" }
        return "Updated \(duration(elapsed)) ago"
    }

    /// Joins the parts that are there with the menu separator.
    public static func join(_ parts: [String?]) -> String {
        parts.compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: separator)
    }

    /// Joins the parts the way a screen reader reads a list.
    public static func joinSpoken(_ parts: [String?]) -> String {
        parts.compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: ", ")
    }
}
