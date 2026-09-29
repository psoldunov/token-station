import AppKit
import SwiftUI
import TokenStationCore
import TokenStationUI

@main
struct TokenStationApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        MenuBarExtra {
            PanelContainer(model: delegate.model)
        } label: {
            MeterLabel(
                meter: delegate.model.snapshot?.meter,
                percentageText: delegate.model.showPercentageInMenuBar
                    ? MeterIcon.percentageText(for: delegate.model.snapshot?.meter)
                    : nil
            )
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsView(model: delegate.model)
        }
    }
}

/// The menu bar image, redrawn whenever the meter changes.
private struct MeterLabel: View {
    var meter: Meter?
    var percentageText: String?

    var body: some View {
        let image = MeterIcon.image(for: meter, percentageText: percentageText)
        // The normal icon stays a template so the system fills it with the menu
        // bar's own label colour; only the orange and red states are drawn as
        // they are, and those carry their outline colour themselves.
        return Image(nsImage: image)
            .renderingMode(image.isTemplate ? .template : .original)
            .accessibilityLabel("Token Station")
            .accessibilityValue(MeterIcon.accessibilityDescription(for: meter?.bars ?? []))
    }
}

/// The panel, plus the two commands that need a scene to act on.
private struct PanelContainer: View {
    var model: AppModel
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        PanelView(
            model: model,
            onOpenSettings: {
                // Opening Settings makes its window key, which is what closes
                // the panel: a menu bar extra has no dismiss of its own.
                NSApp.activate(ignoringOtherApps: true)
                openSettings()
            },
            onQuit: { NSApp.terminate(nil) }
        )
        .sizesWindowToFit()
    }
}
