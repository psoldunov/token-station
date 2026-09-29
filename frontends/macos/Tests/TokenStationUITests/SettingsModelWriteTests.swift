import Foundation
import Testing
import TokenStationCore

@testable import TokenStationUI

@MainActor
@Suite("Settings model writes", .timeLimit(.minutes(1)))
struct SettingsModelWriteTests {
    private let harness: SettingsHarness

    init() throws {
        harness = try SettingsHarness()
    }

    private var settings: SettingsModel { harness.settings }
    private var writes: Requests<JSONValue, Void> { harness.session.settingsWrites }

    /// An error object as the daemon sends it, so the model sees a real `DaemonError`.
    private func rejection(problems: [String], message: String = "invalid params") throws -> DaemonError {
        var error: [String: JSONValue] = ["code": .number(-32_602), "message": .string(message)]
        if !problems.isEmpty {
            error["data"] = .object(["problems": .array(problems.map(JSONValue.string))])
        }
        let json = try JSONEncoder().encode(JSONValue.object(error))
        return try JSONDecoder().decode(DaemonError.self, from: json)
    }

    // MARK: - Order

    @Test("edits reach the daemon one at a time, in the order they were made")
    func writesInOrder() async {
        await harness.load()
        settings.edit { $0.setting("alerts.notify_on_reset", true) }
        settings.edit { $0.setting("claude.enabled", false) }
        settings.edit { $0.setting("codex.enabled", false) }

        // The first write is held back by the session; the others must wait for it.
        let first = await writes.next()
        await flush()
        #expect(await harness.pendingWrites == 0)
        #expect(SettingsDocument(json: first.input).notifyOnReset)
        #expect(SettingsDocument(json: first.input).claudeEnabled)
        first.succeed()

        let second = await writes.next()
        #expect(!SettingsDocument(json: second.input).claudeEnabled)
        #expect(SettingsDocument(json: second.input).codexEnabled)
        #expect(await harness.pendingWrites == 0)
        second.succeed()

        let third = await writes.next()
        #expect(!SettingsDocument(json: third.input).codexEnabled)
        #expect(!SettingsDocument(json: third.input).claudeEnabled)
        third.succeed()
        await settle { !settings.isSaving }
    }

    @Test("saving stays on until the last outstanding write has finished")
    func isSavingSpansTheQueue() async {
        await harness.load()
        #expect(!settings.isSaving)
        settings.edit { $0.setting("claude.enabled", false) }
        settings.edit { $0.setting("codex.enabled", false) }
        #expect(settings.isSaving)

        await writes.next().succeed()
        await flush()
        #expect(settings.isSaving)

        await writes.next().succeed()
        await settle { !settings.isSaving }
    }

    @Test("a debounced edit that a newer edit overtakes is not written on its own")
    func debouncedEditIsSuperseded() async {
        await harness.load()
        settings.edit(debounced: true) { $0.setting("alerts.warning_percent", 60.0) }
        settings.edit { $0.setting("claude.enabled", false) }

        let write = await writes.next()
        let sent = SettingsDocument(json: write.input)
        #expect(sent.warningPercent == 60)
        #expect(!sent.claudeEnabled)
        write.succeed()
        await settle { !settings.isSaving }
        #expect(await harness.pendingWrites == 0)
    }

    // MARK: - Rejection

    @Test("a refused write puts the controls back and shows the daemon's problems")
    func rejectionReverts() async throws {
        await harness.load()
        settings.edit { $0.setting("alerts.notify_on_reset", true) }
        try await writes.next().fail(rejection(problems: ["warning too low", "critical too high"]))
        await settle { !settings.isSaving }

        #expect(settings.document.json == SettingsHarness.stored)
        #expect(settings.problems == ["warning too low", "critical too high"])
        #expect(settings.errorMessage == nil)
    }

    @Test("a refusal with no problem list shows the daemon's message instead")
    func rejectionWithoutProblems() async throws {
        await harness.load()
        settings.edit { $0.setting("alerts.notify_on_reset", true) }
        try await writes.next().fail(rejection(problems: [], message: "config is read-only"))
        await settle { !settings.isSaving }

        #expect(settings.problems == ["config is read-only"])
        #expect(settings.document.json == SettingsHarness.stored)
    }

    @Test("a write that fails for another reason reverts and reports an error message")
    func transportFailure() async throws {
        await harness.load()
        settings.edit { $0.setting("alerts.notify_on_reset", true) }
        await writes.next().fail(TransportDown())
        await settle { !settings.isSaving }

        #expect(settings.errorMessage == "socket closed")
        #expect(settings.problems.isEmpty)
        #expect(settings.document.json == SettingsHarness.stored)
    }

    @Test("an edit made after a refusal clears the problems it showed")
    func editClearsProblems() async throws {
        await harness.load()
        settings.edit { $0.setting("alerts.notify_on_reset", true) }
        try await writes.next().fail(rejection(problems: ["nope"]))
        await settle { !settings.isSaving }
        #expect(settings.problems == ["nope"])

        settings.edit { $0.setting("claude.enabled", false) }
        #expect(settings.problems.isEmpty)
        await writes.next().succeed()
    }

    @Test("a refused old write does not undo an edit made since")
    func staleRejectionKeepsNewerEdit() async throws {
        await harness.load()
        settings.edit { $0.setting("alerts.notify_on_reset", true) }
        settings.edit { $0.setting("claude.enabled", false) }

        try await writes.next().fail(rejection(problems: ["old value refused"]))
        await settle { !settings.problems.isEmpty }
        #expect(settings.problems == ["old value refused"])
        // The controls still show both edits, and the newer write goes out.
        #expect(settings.document.notifyOnReset)
        #expect(!settings.document.claudeEnabled)
        let newer = await writes.next()
        #expect(!SettingsDocument(json: newer.input).claudeEnabled)
        newer.succeed()
        await settle { !settings.isSaving }
        #expect(!settings.document.claudeEnabled)
    }

    @Test("a stale refusal stops being shown once the write that overtook it is accepted")
    func staleProblemsClearOnTheNextSuccess() async throws {
        await harness.load()
        settings.edit { $0.setting("alerts.notify_on_reset", true) }
        settings.edit { $0.setting("claude.enabled", false) }

        try await writes.next().fail(rejection(problems: ["old value refused"]))
        await settle { !settings.problems.isEmpty }

        await writes.next().succeed()
        await settle { !settings.isSaving }
        #expect(settings.problems.isEmpty)
        #expect(settings.errorMessage == nil)
        // The accepted edit is still what the controls show.
        #expect(!settings.document.claudeEnabled)
    }

    @Test("a stale transport failure stops being shown once a later write is accepted")
    func staleErrorMessageClearsOnTheNextSuccess() async {
        await harness.load()
        settings.edit { $0.setting("alerts.notify_on_reset", true) }
        settings.edit { $0.setting("claude.enabled", false) }

        await writes.next().fail(TransportDown())
        await settle { settings.errorMessage != nil }

        await writes.next().succeed()
        await settle { !settings.isSaving }
        #expect(settings.errorMessage == nil)
        #expect(settings.problems.isEmpty)
    }

    @Test("when every outstanding write is refused the controls go back to the last accepted state")
    func revertGoesToLastAccepted() async throws {
        await harness.load()
        settings.edit { $0.setting("alerts.notify_on_reset", true) }
        let accepted = await harness.acceptedWrite()
        await settle { !settings.isSaving }
        #expect(accepted.notifyOnReset)

        settings.edit { $0.setting("claude.enabled", false) }
        try await writes.next().fail(rejection(problems: ["no"]))
        await settle { !settings.isSaving }
        #expect(settings.document == accepted)
    }

    @Test("a stale refusal followed by a refusal of the newest write reverts to the accepted state")
    func bothRefused() async throws {
        await harness.load()
        settings.edit { $0.setting("alerts.notify_on_reset", true) }
        settings.edit { $0.setting("claude.enabled", false) }

        try await writes.next().fail(rejection(problems: ["first"]))
        try await writes.next().fail(rejection(problems: ["second"]))
        await settle { !settings.isSaving }
        #expect(settings.document.json == SettingsHarness.stored)
        #expect(settings.problems == ["second"])
    }
}
