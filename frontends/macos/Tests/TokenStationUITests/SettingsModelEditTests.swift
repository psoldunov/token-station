import Testing
import TokenStationCore

@testable import TokenStationUI

@MainActor
@Suite("Settings model edits", .timeLimit(.minutes(1)))
struct SettingsModelEditTests {
    private let harness: SettingsHarness

    init() throws {
        harness = try SettingsHarness()
    }

    private var settings: SettingsModel { harness.settings }

    @Test("a key this build does not know survives a save")
    func unknownKeysSurvive() async {
        await harness.load()
        settings.edit { $0.setting("alerts.notify", false) }
        let sent = await harness.acceptedWrite()
        #expect(!sent.notify)
        #expect(sent.json[path: "future.invented_later"]?.intValue == 7)
        #expect(sent.json[path: "alerts.warning_percent"]?.intValue == 80)
    }

    @Test("the controls change straight away, before the daemon has answered")
    func editIsImmediate() async {
        await harness.load()
        settings.edit { $0.setting("claude.enabled", false) }
        #expect(!settings.document.claudeEnabled)
        _ = await harness.acceptedWrite()
        await settle { !settings.isSaving }
        #expect(!settings.document.claudeEnabled)
    }

    // MARK: - Thresholds

    @Test("raising warning past critical pulls critical up with it")
    func warningPushesCritical() async {
        await harness.load()
        settings.edit { $0.settingWarningPercent(99) }
        let sent = await harness.acceptedWrite()
        #expect(sent.warningPercent == 99)
        #expect(sent.criticalPercent == 99)
        #expect(settings.document.criticalPercent == 99)
    }

    @Test("lowering critical below warning pulls warning down with it")
    func criticalPullsWarning() async {
        await harness.load()
        settings.edit { $0.settingCriticalPercent(50) }
        let sent = await harness.acceptedWrite()
        #expect(sent.criticalPercent == 50)
        #expect(sent.warningPercent == 50)
        #expect(settings.document.warningPercent == 50)
    }

    @Test("a threshold that already fits leaves the other one alone")
    func thresholdsLeftAlone() async {
        await harness.load()
        settings.edit { $0.settingWarningPercent(60) }
        let warning = await harness.acceptedWrite()
        #expect(warning.warningPercent == 60)
        #expect(warning.criticalPercent == 95)

        settings.edit { $0.settingCriticalPercent(70) }
        let critical = await harness.acceptedWrite()
        #expect(critical.warningPercent == 60)
        #expect(critical.criticalPercent == 70)
    }

    // MARK: - Notifications

    @Test("turning alerts on asks for notification permission, once per change")
    func notificationsWantedOnEnable() async {
        await harness.load(SettingsDocument(json: SettingsHarness.stored).setting("alerts.notify", false).json)
        #expect(harness.app.notificationsWantedCount == 0)

        settings.edit { $0.setting("alerts.notify", true) }
        #expect(harness.app.notificationsWantedCount == 1)

        // Already on: an unrelated edit, or saving the same value, asks for nothing.
        settings.edit { $0.setting("alerts.notify", true) }
        settings.edit { $0.setting("claude.enabled", false) }
        #expect(harness.app.notificationsWantedCount == 1)

        settings.edit { $0.setting("alerts.notify", false) }
        #expect(harness.app.notificationsWantedCount == 1)
        settings.edit { $0.setting("alerts.notify", true) }
        #expect(harness.app.notificationsWantedCount == 2)

        for _ in 0..<5 { _ = await harness.acceptedWrite() }
        await settle { !settings.isSaving }
    }

    @Test("alerts that were already on at load never ask")
    func alreadyOnNeverAsks() async {
        await harness.load()
        settings.edit { $0.setting("alerts.notify_on_reset", true) }
        #expect(harness.app.notificationsWantedCount == 0)
        _ = await harness.acceptedWrite()
        await settle { !settings.isSaving }
    }

    @Test("an edit refused before the settings loaded does not ask for permission")
    func noAskBeforeLoad() async {
        settings.edit { $0.setting("alerts.notify", true) }
        #expect(harness.app.notificationsWantedCount == 0)
    }
}
