import Charts
import SwiftUI
import TokenStationCore

/// The sparkline under a provider: the meter window's history, 0–100.
///
/// It is an area under a line, tinted by the window's level — the shape the
/// Battery and Screen Time panels use for their own little histories.
struct UsageChart: View {
    var caption: String
    var points: [HistoryPoint]
    var level: Level

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(caption)
                .font(.system(size: PanelMetrics.secondarySize))
                .foregroundStyle(.secondary)
            chart
                .frame(height: PanelMetrics.chartHeight)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(accessibilityLabel)
    }

    @ViewBuilder
    private var chart: some View {
        if points.count < 2 {
            RoundedRectangle(cornerRadius: 4, style: .continuous)
                .fill(.quaternary)
                .overlay {
                    Text("Not enough history yet")
                        .font(.system(size: 10))
                        .foregroundStyle(.tertiary)
                }
        } else {
            Chart(points, id: \.timestamp) { point in
                AreaMark(
                    x: .value("Time", point.date),
                    y: .value("Used", point.percent)
                )
                .foregroundStyle(
                    .linearGradient(
                        colors: [level.tint.opacity(0.28), level.tint.opacity(0.02)],
                        startPoint: .top,
                        endPoint: .bottom
                    )
                )
                .interpolationMethod(.monotone)

                LineMark(
                    x: .value("Time", point.date),
                    y: .value("Used", point.percent)
                )
                .foregroundStyle(level.tint)
                .lineStyle(StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
                .interpolationMethod(.monotone)
            }
            .chartYScale(domain: 0...100)
            .chartXAxis(.hidden)
            .chartYAxis(.hidden)
            .chartLegend(.hidden)
        }
    }

    private var accessibilityLabel: String {
        guard let last = points.last else { return "\(caption), no history yet" }
        return "\(caption), now at \(Formatting.percent(last.percent))"
    }
}
