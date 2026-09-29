import Foundation
import Observation
import TokenStationCore

/// The settings window's copy of the config, and the writes back to the daemon.
///
/// A control changes the document straight away so the window stays responsive,
/// and the write is debounced. Two rules keep that from losing an edit:
///
///   * Nothing is written before `GetSettings` has answered. `SetSettings`
///     replaces the whole config, so saving a document this window never read
///     would reset every key it does not know about to its default.
///   * Writes run in the order they were made, one at a time, and each carries
///     the edit generation it came from. A rejection only puts the controls back
///     when nothing newer is waiting, so the daemon refusing an old value cannot
///     wipe out what the user typed while it was thinking.
@Observable
@MainActor
final class SettingsModel {
    /// How long a stepper is left alone before its value is written out.
    private static let saveDebounce: Duration = .milliseconds(400)

    private(set) var document = SettingsDocument()
    private(set) var isLoaded = false
    /// Why the settings could not be read or written.
    private(set) var errorMessage: String?
    /// `error.data.problems` from a `SetSettings` the daemon refused.
    private(set) var problems: [String] = []
    private(set) var isSaving = false

    private let model: AppModel
    /// The last document the daemon accepted, which a rejection reverts to.
    private var committed = SettingsDocument()
    /// Bumped by every edit; a save carries the generation it was made at.
    private var editGeneration = 0
    /// The tail of the write chain, so saves cannot overtake one another.
    private var saveChain: Task<Void, Never>?
    /// How many scheduled writes have yet to finish.
    private var outstanding = 0

    init(model: AppModel) {
        self.model = model
    }

    /// Reads the config. Safe to call again when the daemon comes back.
    func load() async {
        do {
            let loaded = try await model.loadSettings()
            // An edit made while the read was in flight wins: it is newer than
            // what the daemon just handed back.
            guard outstanding == 0 else { return }
            document = loaded
            committed = loaded
            errorMessage = nil
            problems = []
            isLoaded = true
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    /// Applies an edit and schedules the write.
    func edit(
        debounced: Bool = false,
        _ transform: (SettingsDocument) -> SettingsDocument
    ) {
        guard isLoaded else { return }
        let wasNotifying = document.notify
        document = transform(document)
        problems = []
        errorMessage = nil
        // Turning alerts on is the moment to ask for permission, not launch.
        if !wasNotifying, document.notify { model.notificationsWanted() }
        scheduleSave(debounced: debounced)
    }

    private func scheduleSave(debounced: Bool) {
        editGeneration += 1
        let generation = editGeneration
        let pending = document
        outstanding += 1
        isSaving = true

        let previous = saveChain
        saveChain = Task { [self] in
            defer { writeFinished() }
            if debounced {
                try? await Task.sleep(for: Self.saveDebounce)
                // A newer edit is already on its way; it carries this value too.
                if generation != editGeneration { return }
            }
            // Wait for the write in front, so the daemon sees the edits in the
            // order the user made them.
            await previous?.value
            await save(pending, generation: generation)
        }
    }

    private func writeFinished() {
        outstanding -= 1
        isSaving = outstanding > 0
    }

    private func save(_ pending: SettingsDocument, generation: Int) async {
        do {
            try await model.saveSettings(pending)
            committed = pending
            // A write going through answers for the ones it overtook: a refusal
            // of a value this one replaced is no longer about anything on
            // screen, and leaving it up says the controls are wrong when they
            // are exactly what the daemon just accepted.
            problems = []
            errorMessage = nil
        } catch let error as DaemonError {
            problems = error.problems.isEmpty ? [error.message] : error.problems
            revert(generation: generation)
        } catch {
            errorMessage = error.localizedDescription
            revert(generation: generation)
        }
    }

    /// Puts the controls back, unless a newer edit is already waiting.
    private func revert(generation: Int) {
        guard generation == editGeneration else { return }
        document = committed
    }

    // MARK: - Facts for the footer

    /// The `config.toml` the daemon reads.
    var configPath: String { SupportPaths.configPath() }

    /// `1.2 (34)`, or just the short version when there is no build number.
    var appVersion: String {
        let info = Bundle.main.infoDictionary
        let short = info?["CFBundleShortVersionString"] as? String
        let build = info?["CFBundleVersion"] as? String
        switch (short, build) {
        case (let short?, let build?) where short != build: return "\(short) (\(build))"
        case (let short?, _): return short
        default: return "—"
        }
    }

    var daemonVersion: String { model.daemonVersion ?? "—" }
}
