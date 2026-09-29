import Foundation

/// The choices the "Check Plan Limits" pop-up offers.
public enum LimitsInterval {
    /// Every preset the pop-up lists, shortest first.
    public static let presets: [Int] = [120, 300, 600, 900, 1800, 3600]

    /// `Every 5 Minutes`, `Every Hour`, or a spelled-out custom value.
    public static func title(_ seconds: Int) -> String {
        switch seconds {
        case 120: "Every 2 Minutes"
        case 300: "Every 5 Minutes"
        case 600: "Every 10 Minutes"
        case 900: "Every 15 Minutes"
        case 1800: "Every 30 Minutes"
        case 3600: "Every Hour"
        default: "Every \(Formatting.duration(TimeInterval(seconds), width: .wide))"
        }
    }

    /// The presets plus the value in the config, so a hand-written interval the
    /// pop-up has no preset for is shown rather than silently replaced.
    public static func choices(including current: Int) -> [Int] {
        presets.contains(current) ? presets : (presets + [current]).sorted()
    }
}
