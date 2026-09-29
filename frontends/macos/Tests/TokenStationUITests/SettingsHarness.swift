import TokenStationCore

@testable import TokenStationUI

/// A settings model on a fake session, with helpers to load it and to read what
/// it wrote.
@MainActor
final class SettingsHarness {
    let app: AppHarness
    let settings: SettingsModel

    /// What the daemon hands back from `GetSettings` unless a test says otherwise.
    static let stored = SettingsDocument()
        .setting("alerts.notify", true)
        .setting("alerts.notify_on_reset", false)
        .setting("alerts.warning_percent", 80.0)
        .setting("alerts.critical_percent", 95.0)
        .setting("future.invented_later", 7)
        .json

    init() throws {
        app = try AppHarness()
        settings = SettingsModel(model: app.model)
    }

    var session: FakeSession { app.session }

    /// Runs `load()` against the fake daemon, which answers with `json`.
    func load(_ json: JSONValue = SettingsHarness.stored) async {
        let loading = Task { await settings.load() }
        await session.settingsReads.next().succeed(json)
        await loading.value
    }

    /// The next write the model sent, answered as accepted.
    func acceptedWrite() async -> SettingsDocument {
        let write = await session.settingsWrites.next()
        write.succeed()
        return SettingsDocument(json: write.input)
    }

    var pendingWrites: Int {
        get async { await session.settingsWrites.pendingCount }
    }
}
