import SwiftUI

/// One 11 pt secondary line — credits, the breakdown, a token total.
struct SecondaryLine: View {
    var text: String

    var body: some View {
        Text(text)
            .font(.system(size: PanelMetrics.secondarySize))
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}
