import Testing
import TokenStationCore

@testable import TokenStationUI

@MainActor
@Suite("Settings model loading", .timeLimit(.minutes(1)))
struct SettingsModelLoadTests {
    private let harness: SettingsHarness

    init() throws {
        harness = try SettingsHarness()
    }

    private var settings: SettingsModel { harness.settings }

    @Test("nothing is sent for an edit made before the settings were read")
    func refusesToSaveBeforeLoad() async {
        settings.edit { $0.setting("claude.enabled", false) }
        #expect(!settings.isSaving)
        #expect(!settings.isLoaded)
        #expect(settings.document == SettingsDocument())

        await harness.load()
        settings.edit { $0.setting("codex.enabled", false) }
        // Had the early edit been queued, it would be the first write out.
        let sent = await harness.acceptedWrite()
        #expect(!sent.codexEnabled)
        #expect(sent.claudeEnabled)
        #expect(await harness.pendingWrites == 0)
    }

    @Test("a read that fails reports why and still refuses edits")
    func failedLoad() async {
        let loading = Task { await settings.load() }
        await harness.session.settingsReads.next().fail(TransportDown())
        await loading.value
        #expect(!settings.isLoaded)
        #expect(settings.errorMessage == "socket closed")

        settings.edit { $0.setting("claude.enabled", false) }
        await flush()
        #expect(!settings.isSaving)
        #expect(await harness.pendingWrites == 0)
    }

    @Test("a good read shows the daemon's document and clears an earlier error")
    func goodLoad() async {
        let failing = Task { await settings.load() }
        await harness.session.settingsReads.next().fail(TransportDown())
        await failing.value
        #expect(settings.errorMessage != nil)

        await harness.load()
        #expect(settings.isLoaded)
        #expect(settings.errorMessage == nil)
        #expect(settings.document.json == SettingsHarness.stored)
        #expect(settings.document.warningPercent == 80)
    }

    @Test("a read that returns while an edit is being written does not undo the edit")
    func staleReloadIgnored() async {
        await harness.load()
        settings.edit { $0.setting("alerts.notify_on_reset", true) }
        #expect(settings.isSaving)

        // The reload was answered with what the daemon had before the edit.
        await harness.load()
        #expect(settings.document.notifyOnReset)

        _ = await harness.acceptedWrite()
        await settle { !settings.isSaving }

        let updated = SettingsDocument(json: SettingsHarness.stored).setting("claude.enabled", false)
        await harness.load(updated.json)
        #expect(!settings.document.claudeEnabled)
    }
}
