import SwiftUI
import TokenStationCore

/// Everything the panel shows for one provider.
struct ProviderSection: View {
    var provider: ProviderSnapshot
    var meterWindow: UsageWindow?
    var historyPoints: [HistoryPoint]

    var body: some View {
        VStack(alignment: .leading, spacing: PanelMetrics.rowSpacing) {
            header

            if let message = PanelText.stateMessage(provider) {
                ProviderStateRow(state: provider.state, message: message)
            }

            ForEach(provider.windows) { window in
                UsageWindowRow(window: window)
            }

            if let credits = PanelText.creditsLine(provider.credits) {
                SecondaryLine(text: credits)
            }
            if let breakdown = PanelText.breakdownLine(provider.breakdown) {
                SecondaryLine(text: breakdown)
            }
            ForEach(Array(PanelText.tokenLines(provider).enumerated()), id: \.offset) { _, line in
                SecondaryLine(text: line)
            }

            if let byModel = provider.tokens?.byModel, !byModel.isEmpty {
                PerModelDisclosure(rows: byModel)
            }

            if let window = meterWindow {
                UsageChart(
                    caption: PanelText.chartCaption(window),
                    points: historyPoints,
                    level: window.level
                )
            }
        }
    }

    /// The small secondary section header macOS uses above a group of rows.
    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(provider.name)
                .font(.system(size: PanelMetrics.sectionHeaderSize, weight: .semibold))
                .foregroundStyle(.secondary)
            Spacer(minLength: 4)
            if let plan = provider.plan, !plan.isEmpty {
                Text(plan)
                    .font(.system(size: PanelMetrics.sectionHeaderSize))
                    .foregroundStyle(.tertiary)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }
}
