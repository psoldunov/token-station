import SwiftUI

/// What the panel shows instead of providers: starting up, broken, or empty.
struct PanelPlaceholder: View {
    var symbolName: String
    var symbolTint = AnyShapeStyle(.tertiary)
    var title: String
    var message: String?
    var actionTitle: String?
    var action: (() -> Void)?

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: symbolName)
                .font(.system(size: 22, weight: .light))
                .foregroundStyle(symbolTint)
                .accessibilityHidden(true)
            Text(title)
                .font(.system(size: PanelMetrics.rowSize, weight: .medium))
                .multilineTextAlignment(.center)
            if let message {
                Text(message)
                    .font(.system(size: PanelMetrics.secondarySize))
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let actionTitle, let action {
                Button(actionTitle, action: action)
                    .controlSize(.small)
                    .padding(.top, 2)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 18)
        .accessibilityElement(children: .contain)
    }
}
