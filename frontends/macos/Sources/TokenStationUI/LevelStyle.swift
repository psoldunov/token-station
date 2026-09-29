import SwiftUI
import TokenStationCore

extension Level {
    /// The colour a bar, a chart or a meter capsule takes at this level.
    ///
    /// Normal usage takes the accent colour the user chose, the way every
    /// system slider does; warning and critical take the system orange and red,
    /// which is what the battery icon does when it runs low.
    public var tint: Color {
        switch self {
        case .warning: .orange
        case .critical: .red
        case .normal, .unknown: .accentColor
        }
    }

    /// True while the level is worth colouring away from the accent colour.
    public var isElevated: Bool {
        switch self {
        case .warning, .critical: true
        case .normal, .unknown: false
        }
    }
}

extension ProviderState {
    /// The SF Symbol shown beside a non-`ok` provider's message.
    public var symbolName: String {
        switch self {
        case .loading: "arrow.triangle.2.circlepath"
        case .stale: "exclamationmark.triangle.fill"
        case .unauthenticated: "person.crop.circle.badge.exclamationmark"
        case .notInstalled: "questionmark.circle"
        case .disabled: "circle.slash"
        case .error: "xmark.octagon.fill"
        case .ok, .unknown: "info.circle"
        }
    }

    /// How that symbol is tinted.
    public var symbolTint: Color {
        switch self {
        case .stale: .orange
        case .error: .red
        default: .secondary
        }
    }
}
