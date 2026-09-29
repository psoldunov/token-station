import SwiftUI
import TokenStationCore

/// The Control Center-style panel the menu bar icon opens.
public struct PanelView: View {
    private let model: AppModel
    private let isScrollable: Bool
    private let onOpenSettings: () -> Void
    private let onQuit: () -> Void

    @State private var contentHeight: CGFloat = 0

    /// - Parameter isScrollable: `false` draws the whole panel at its natural
    ///   height, which is what `ts-render` needs and what a preview wants.
    public init(
        model: AppModel,
        isScrollable: Bool = true,
        onOpenSettings: @escaping () -> Void,
        onQuit: @escaping () -> Void
    ) {
        self.model = model
        self.isScrollable = isScrollable
        self.onOpenSettings = onOpenSettings
        self.onQuit = onQuit
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            PanelHeader(isRefreshing: model.isRefreshing) { model.refresh() }
                .padding(.horizontal, PanelMetrics.horizontalPadding)
                .padding(.top, PanelMetrics.topPadding)
                .padding(.bottom, PanelMetrics.sectionSpacing)

            body(for: model.phase)

            Divider()
                .padding(.horizontal, PanelMetrics.horizontalPadding)
                .padding(.vertical, 5)

            commands
                .padding(.horizontal, PanelMetrics.horizontalPadding
                    - PanelMetrics.menuRowHorizontalPadding)

            // Only a snapshot can be dated. While the daemon is starting, or
            // after it failed, the state above already says where things stand
            // and a footer could only contradict it.
            if let generatedAt = model.snapshot?.generatedDate {
                PanelFooter(generatedAt: generatedAt, refreshFailure: model.refreshFailure)
                    .padding(.horizontal, PanelMetrics.horizontalPadding)
                    .padding(.top, 6)
                    .padding(.bottom, PanelMetrics.bottomPadding)
            } else {
                Color.clear.frame(height: PanelMetrics.bottomPadding)
            }
        }
        .frame(width: PanelMetrics.width)
        .onAppear { model.panelDidOpen() }
    }

    @ViewBuilder
    private func body(for phase: AppModel.Phase) -> some View {
        switch phase {
        case .connecting:
            PanelPlaceholder(
                symbolName: "arrow.triangle.2.circlepath",
                title: "Starting Token Station…",
                message: "Reading your Claude Code and Codex plan usage."
            )
            .padding(.horizontal, PanelMetrics.horizontalPadding)
        case .failed(let kind, let message):
            // A helper that came up once and went away is being restarted, and
            // saying it "could not start" would be both wrong and alarming.
            PanelPlaceholder(
                symbolName: kind == .restarting
                    ? "arrow.triangle.2.circlepath" : "exclamationmark.triangle.fill",
                symbolTint: kind == .restarting
                    ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.orange),
                title: kind == .restarting
                    ? "Restarting Token Station…" : "Token Station could not start",
                message: message,
                actionTitle: "Try Again",
                action: { model.retry() }
            )
            .padding(.horizontal, PanelMetrics.horizontalPadding)
        case .running:
            if model.hasNoProviders {
                PanelPlaceholder(
                    symbolName: "switch.2",
                    title: "No providers are turned on",
                    message: "Turn on Claude Code or Codex to see plan usage here.",
                    actionTitle: "Open Settings…",
                    action: onOpenSettings
                )
                .padding(.horizontal, PanelMetrics.horizontalPadding)
            } else {
                providers
            }
        }
    }

    private var providers: some View {
        let sections = VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(providerList.enumerated()), id: \.element.id) { index, provider in
                if index > 0 {
                    Divider().padding(.vertical, PanelMetrics.sectionSpacing)
                }
                ProviderSection(
                    provider: provider,
                    meterWindow: model.snapshot.flatMap {
                        PanelText.meterWindow(for: provider, in: $0)
                    },
                    historyPoints: model.historyPoints(for: provider)
                )
            }
        }
        .padding(.horizontal, PanelMetrics.horizontalPadding)
        .padding(.bottom, 2)

        return Group {
            if isScrollable {
                ScrollView(.vertical) {
                    sections
                        .background {
                            GeometryReader { geometry in
                                Color.clear.preference(
                                    key: ContentHeightKey.self, value: geometry.size.height)
                            }
                        }
                }
                .scrollIndicators(.automatic)
                .frame(height: min(
                    max(contentHeight, 1), PanelMetrics.maximumContentHeight))
                .onPreferenceChange(ContentHeightKey.self) { contentHeight = $0 }
            } else {
                sections
            }
        }
    }

    private var providerList: [ProviderSnapshot] {
        model.snapshot?.providers ?? []
    }

    private var commands: some View {
        VStack(alignment: .leading, spacing: 0) {
            MenuRow(title: "Token Station Settings…", shortcut: "⌘,", action: onOpenSettings)
                .keyboardShortcut(",", modifiers: .command)
            MenuRow(title: "Quit Token Station", shortcut: "⌘Q", action: onQuit)
                .keyboardShortcut("q", modifiers: .command)
        }
    }
}

/// Measures the scrolling part so the panel is only as tall as it needs to be.
private struct ContentHeightKey: PreferenceKey {
    static let defaultValue: CGFloat = 0

    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = max(value, nextValue())
    }
}
