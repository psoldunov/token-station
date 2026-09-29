import SwiftUI
import TokenStationCore

/// The capsule usage bar under a window row.
///
/// A `ProgressView` would carry a system slider's inset and its own animation;
/// what the Control Center panels draw for a level is a plain capsule track with
/// a capsule fill, which is what this is.
struct UsageBar: View {
    var percent: Double
    var level: Level

    private var fraction: Double {
        guard percent.isFinite else { return 0 }
        return min(max(percent, 0), 100) / 100
    }

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule(style: .continuous)
                    .fill(.quaternary)
                Capsule(style: .continuous)
                    .fill(level.tint)
                    .frame(width: max(fillWidth(in: geometry.size.width), 0))
            }
        }
        .frame(height: PanelMetrics.barHeight)
        .accessibilityHidden(true)
    }

    /// A non-zero reading always shows at least a round cap, so that 1 % reads
    /// as a sliver rather than as nothing at all.
    private func fillWidth(in available: CGFloat) -> CGFloat {
        guard fraction > 0 else { return 0 }
        return max(available * fraction, PanelMetrics.barHeight)
    }
}
