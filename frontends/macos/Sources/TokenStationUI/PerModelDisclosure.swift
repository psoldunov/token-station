import SwiftUI
import TokenStationCore

/// The "Per Model" list, opened by a row whose chevron turns.
///
/// This is how the Wi-Fi menu shows "Other Networks": a plain row, a trailing
/// chevron that rotates a quarter turn, and the contents sliding in underneath.
struct PerModelDisclosure: View {
    var rows: [ModelTotals]
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var isExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                // Reduce Motion turns the slide into a plain appearance, which
                // is what every system disclosure does under that setting.
                withAnimation(reduceMotion ? nil : .easeOut(duration: PanelMetrics.resizeDuration)) {
                    isExpanded.toggle()
                }
            } label: {
                HStack(spacing: 4) {
                    Text("Per Model")
                        .font(.system(size: PanelMetrics.rowSize))
                    Spacer(minLength: 4)
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(isExpanded ? 90 : 0))
                        .accessibilityHidden(true)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Per Model")
            .accessibilityValue(isExpanded ? "Expanded" : "Collapsed")
            .accessibilityAddTraits(.isButton)

            // The list stays in the layout and its height is what animates, from
            // nothing to all of it, clipped from the top. Inserting it with a
            // transition instead draws it at full height straight away, over the
            // rows below, while they are still sliding down to make room.
            list
                .padding(.top, PanelMetrics.rowSpacing)
                .frame(height: isExpanded ? nil : 0, alignment: .top)
                .clipped()
                .opacity(isExpanded ? 1 : 0)
                .accessibilityHidden(!isExpanded)
        }
    }

    private var list: some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(rows) { row in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(row.model)
                        .font(.system(size: PanelMetrics.secondarySize))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 4)
                    Text(PanelText.modelDetail(row))
                        .font(.system(size: PanelMetrics.secondarySize))
                        .monospacedDigit()
                        .foregroundStyle(.tertiary)
                }
                .accessibilityElement(children: .combine)
            }
        }
        .padding(.leading, 2)
    }
}
