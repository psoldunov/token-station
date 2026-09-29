import SwiftUI
import TokenStationCore

/// One plan window: its name, its reading, and the bar underneath.
///
/// The reading is inside a `TimelineView`, so the "resets in …" countdown keeps
/// running for as long as the panel is open without the whole panel redrawing.
struct UsageWindowRow: View {
    var window: UsageWindow

    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(window.label)
                        .font(.system(size: PanelMetrics.rowSize))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    reading(now: context.date)
                        .font(.system(size: PanelMetrics.secondarySize))
                        .monospacedDigit()
                        .lineLimit(1)
                }
                UsageBar(percent: window.usedPercent, level: window.level)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(
                PanelText.windowAccessibilityLabel(window, now: context.date))
        }
    }

    /// The percentage carries the level's colour; the countdown stays secondary.
    private func reading(now: Date) -> Text {
        let percent = Text(PanelText.windowPercent(window))
            .foregroundStyle(window.level.isElevated
                ? AnyShapeStyle(window.level.tint)
                : AnyShapeStyle(.secondary))
        guard let reset = PanelText.windowReset(window, now: now) else { return percent }
        return percent + Text(reset).foregroundStyle(.secondary)
    }
}
