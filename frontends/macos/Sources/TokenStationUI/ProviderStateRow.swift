import SwiftUI
import TokenStationCore

/// The symbol-and-message row a provider shows when its state is not `ok`.
struct ProviderStateRow: View {
    var state: ProviderState
    var message: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: state.symbolName)
                .font(.system(size: PanelMetrics.secondarySize))
                .foregroundStyle(state.symbolTint)
                .accessibilityHidden(true)
            Text(message)
                .font(.system(size: PanelMetrics.secondarySize))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .combine)
    }
}
