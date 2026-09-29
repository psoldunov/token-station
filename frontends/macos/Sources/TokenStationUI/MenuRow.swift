import SwiftUI

/// A menu-style command row at the foot of the panel.
///
/// `NSMenu` is not available inside a window-style menu bar extra, so the rows
/// are rebuilt: the same 13 pt text, the same trailing key equivalent, and the
/// same rounded highlight that follows the pointer.
struct MenuRow: View {
    var title: String
    var shortcut: String?
    var action: () -> Void

    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Text(title)
                    .font(.system(size: PanelMetrics.menuRowSize))
                Spacer(minLength: 8)
                if let shortcut {
                    Text(shortcut)
                        .font(.system(size: PanelMetrics.menuRowSize))
                        .foregroundStyle(.tertiary)
                }
            }
            .padding(.vertical, PanelMetrics.menuRowVerticalPadding)
            .padding(.horizontal, PanelMetrics.menuRowHorizontalPadding)
            .contentShape(Rectangle())
            .background {
                RoundedRectangle(
                    cornerRadius: PanelMetrics.menuRowCornerRadius, style: .continuous
                )
                .fill(Color.primary.opacity(isHovered ? 0.09 : 0))
            }
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .accessibilityLabel(title)
    }
}
