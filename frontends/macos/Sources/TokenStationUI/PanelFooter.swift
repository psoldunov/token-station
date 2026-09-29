import SwiftUI
import TokenStationCore

/// `Updated 2 min ago`, kept current for as long as the panel is open.
///
/// A refresh that did not go through is said here too, quietly: the snapshot on
/// screen is still the last good one, so this is a footnote about its age rather
/// than an error about its contents.
struct PanelFooter: View {
    var generatedAt: Date
    var refreshFailure: String?

    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                if refreshFailure != nil {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.system(size: 9))
                        .foregroundStyle(.orange)
                        .accessibilityHidden(true)
                }
                Text(text(now: context.date))
                    .font(.system(size: PanelMetrics.secondarySize))
                    .foregroundStyle(.tertiary)
                    .lineLimit(2)
                    .truncationMode(.tail)
                Spacer(minLength: 0)
            }
            .accessibilityElement(children: .combine)
        }
    }

    private func text(now: Date) -> String {
        guard let refreshFailure else { return Formatting.updatedAgo(generatedAt, now: now) }
        return "Couldn't refresh — \(refreshFailure)"
    }
}
