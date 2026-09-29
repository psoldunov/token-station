import SwiftUI

/// The panel's title row, with the refresh button Control Center puts on the right.
struct PanelHeader: View {
    var isRefreshing: Bool
    var action: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Text("Token Station")
                .font(.system(size: PanelMetrics.titleSize, weight: .bold))
            Spacer(minLength: 8)
            Button(action: action) {
                ZStack {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 12, weight: .medium))
                        .opacity(isRefreshing ? 0 : 1)
                    ProgressView()
                        .controlSize(.small)
                        .opacity(isRefreshing ? 1 : 0)
                }
                .frame(width: 18, height: 18)
                .contentShape(Rectangle())
            }
            .buttonStyle(.borderless)
            .disabled(isRefreshing)
            // The one refresh control in the panel, so it carries the shortcut
            // a "Refresh Now" menu row would have had.
            .keyboardShortcut("r", modifiers: .command)
            .accessibilityLabel("Refresh Now")
            .help("Refresh Now")
        }
    }
}
