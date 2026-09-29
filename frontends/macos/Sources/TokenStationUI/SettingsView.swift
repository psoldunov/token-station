import AppKit
import SwiftUI
import TokenStationCore

/// The Settings window: one grouped form, the shape a System Settings pane has.
public struct SettingsView: View {
    private let model: AppModel
    @State private var settings: SettingsModel
    @State private var loginItem = LoginItemController()

    public init(model: AppModel) {
        self.model = model
        _settings = State(initialValue: SettingsModel(model: model))
    }

    public var body: some View {
        Form {
            statusSection
            Group {
                generalSection
                providersSection
                notificationsSection
                pricingSection
            }
            // Nothing is editable before `GetSettings` answers: `SetSettings`
            // replaces the whole document, so writing one this window never read
            // would reset every key it does not know about.
            .disabled(!settings.isLoaded)
            aboutSection
        }
        .formStyle(.grouped)
        .frame(width: 460)
        .frame(minHeight: 520)
        // Reloads when the daemon comes up, and again after a reconnect, rather
        // than only the first time the window is put on screen.
        .task(id: model.phase) {
            guard model.phase == .running else { return }
            await settings.load()
        }
        .onAppear { loginItem.refresh() }
        .onReceive(
            NotificationCenter.default.publisher(
                for: NSApplication.didBecomeActiveNotification)
        ) { _ in
            // Approval is granted in System Settings, so the answer only changes
            // while this app is in the background.
            loginItem.refresh()
        }
    }

    /// What the form cannot do yet, and why — above everything, so a problem is
    /// read before the control that caused it rather than after a scroll.
    @ViewBuilder
    private var statusSection: some View {
        if !settings.isLoaded || !settings.problems.isEmpty || settings.errorMessage != nil {
            Section {
                if !settings.problems.isEmpty {
                    InlineNotice(text: settings.problems.joined(separator: "\n"))
                }
                if let message = settings.errorMessage {
                    InlineNotice(text: message)
                } else if !settings.isLoaded {
                    // Not a problem yet, so not a warning triangle.
                    InlineNotice(
                        symbolName: "arrow.triangle.2.circlepath",
                        tint: AnyShapeStyle(.secondary),
                        text: model.phase == .running
                            ? "Reading the current settings…"
                            : "Waiting for the Token Station service…")
                }
            }
        }
    }

    // MARK: - Sections

    private var generalSection: some View {
        Section("General") {
            VStack(alignment: .leading, spacing: 4) {
                Toggle("Open at Login", isOn: Binding(
                    get: { loginItem.isEnabled },
                    set: { loginItem.setEnabled($0) }
                ))
                if loginItem.needsApproval {
                    InlineNotice(
                        text: "macOS is waiting for your approval in Login Items.",
                        actionTitle: "Open Login Items…",
                        action: { loginItem.openLoginItemsSettings() }
                    )
                }
                if let message = loginItem.errorMessage {
                    InlineNotice(text: message)
                }
            }

            Toggle("Show Percentage in Menu Bar", isOn: Binding(
                get: { model.showPercentageInMenuBar },
                set: { model.showPercentageInMenuBar = $0 }
            ))

            Picker("Menu Bar Meter", selection: Binding(
                get: { settings.document.meterWindow },
                set: { value in
                    settings.edit { $0.setting("meter.window", value.rawValue) }
                }
            )) {
                ForEach(MeterWindowSetting.allCases, id: \.self) { window in
                    Text(window.title).tag(window)
                }
            }
        }
    }

    private var providersSection: some View {
        Section("Providers") {
            Toggle("Claude Code", isOn: binding(path: "claude.enabled", default: true))
            Toggle("Codex", isOn: binding(path: "codex.enabled", default: true))

            Picker("Check Plan Limits", selection: Binding(
                get: { settings.document.limitsIntervalSeconds },
                set: { value in
                    settings.edit { $0.setting("general.limits_interval_secs", value) }
                }
            )) {
                ForEach(
                    LimitsInterval.choices(including: settings.document.limitsIntervalSeconds),
                    id: \.self
                ) { seconds in
                    Text(LimitsInterval.title(seconds)).tag(seconds)
                }
            }
        }
    }

    private var notificationsSection: some View {
        Section("Notifications") {
            Toggle("Notify Near Limits", isOn: binding(path: "alerts.notify", default: true))

            Group {
                ThresholdStepper(title: "Warning At", value: Binding(
                    get: { settings.document.warningPercent },
                    set: { value in
                        settings.edit(debounced: true) { $0.settingWarningPercent(value) }
                    }
                ))
                ThresholdStepper(title: "Critical At", value: Binding(
                    get: { settings.document.criticalPercent },
                    set: { value in
                        settings.edit(debounced: true) { $0.settingCriticalPercent(value) }
                    }
                ))
            }
            .disabled(!settings.document.notify)

            Toggle(
                "Notify When a Limit Resets",
                isOn: binding(path: "alerts.notify_on_reset", default: false))
        }
    }

    private var pricingSection: some View {
        Section("Pricing") {
            Toggle("Update Prices Daily", isOn: binding(path: "pricing.auto_update", default: true))
        }
    }

    private var aboutSection: some View {
        Section {
            LabeledContent("Configuration File") {
                HStack(spacing: 8) {
                    Text(settings.configPath)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .help(settings.configPath)
                    Button("Show in Finder") {
                        NSWorkspace.shared.activateFileViewerSelecting(
                            [URL(filePath: settings.configPath)])
                    }
                    .controlSize(.small)
                }
            }
            LabeledContent("Token Station", value: settings.appVersion)
            LabeledContent("Service", value: settings.daemonVersion)
        }
    }

    // MARK: - Helpers

    private func binding(path: String, default fallback: Bool) -> Binding<Bool> {
        Binding(
            get: { settings.document.bool(path, default: fallback) },
            set: { value in settings.edit { $0.setting(path, value) } }
        )
    }
}

/// A 1–100 threshold, stepped in fives, with its value beside the stepper.
private struct ThresholdStepper: View {
    var title: String
    @Binding var value: Double

    var body: some View {
        LabeledContent(title) {
            HStack(spacing: 6) {
                Text(Formatting.percent(value))
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
                Stepper(
                    title,
                    value: $value,
                    in: SettingsDocument.Bounds.thresholdPercent,
                    step: SettingsDocument.Bounds.thresholdStep
                )
                .labelsHidden()
            }
        }
        .accessibilityValue(Formatting.percent(value))
    }
}

/// A short explanation under a control, with an optional button.
private struct InlineNotice: View {
    var symbolName = "exclamationmark.triangle.fill"
    var tint = AnyShapeStyle(.orange)
    var text: String
    var actionTitle: String?
    var action: (() -> Void)?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: symbolName)
                .foregroundStyle(tint)
                .accessibilityHidden(true)
            Text(text)
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if let actionTitle, let action {
                Button(actionTitle, action: action)
                    .controlSize(.small)
            }
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .combine)
    }
}
