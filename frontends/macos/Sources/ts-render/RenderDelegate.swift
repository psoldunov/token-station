import AppKit
import TokenStationCore
import TokenStationUI

/// Runs every render once the application has a run loop, then stops.
@MainActor
final class RenderDelegate: NSObject, NSApplicationDelegate {
    private let fixtures: URL
    private let output: URL
    private var written: [String] = []

    private let appearances: [(name: String, appearance: NSAppearance)] = [
        ("light", NSAppearance(named: .aqua) ?? NSAppearance.currentDrawing()),
        ("dark", NSAppearance(named: .darkAqua) ?? NSAppearance.currentDrawing()),
    ]

    init(fixtures: URL, output: URL) {
        self.fixtures = fixtures
        self.output = output
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        do {
            try FileManager.default.createDirectory(
                at: output, withIntermediateDirectories: true)
            try renderPanels()
            renderStates()
            renderSettings()
            renderIcons()
        } catch {
            FileHandle.standardError.write(Data("ts-render: \(error)\n".utf8))
            exit(1)
        }
        for name in written { print(name) }
        exit(0)
    }

    // MARK: - Panels

    /// Reads one fixture and moves its clock to now.
    private func loadFixture(named name: String) throws -> Snapshot {
        let url = fixtures.appending(path: name, directoryHint: .notDirectory)
        return SnapshotRebasing.toNow(
            try JSONDecoder().decode(Snapshot.self, from: try Data(contentsOf: url)))
    }

    private func renderPanels() throws {
        let files = try FileManager.default
            .contentsOfDirectory(at: fixtures, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "json" }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }

        for file in files {
            let snapshot = try loadFixture(named: file.lastPathComponent)
            let name = file.deletingPathExtension().lastPathComponent
                .replacingOccurrences(of: "snapshot-", with: "")
            let model = makeModel(
                session: FixtureSession(
                    snapshot: snapshot,
                    history: FixtureSession.syntheticHistory(for: snapshot)))
            renderPanel(model: model, named: "panel-\(name)")
        }
    }

    private func renderStates() {
        renderPanel(
            model: makeModel(session: FixtureSession(snapshot: nil)),
            named: "panel-connecting")
        renderPanel(
            model: makeModel(session: FixtureSession(
                snapshot: nil,
                failure: "The Token Station service stopped: no usable provider was found.")),
            named: "panel-helper-failed")
        renderPanel(
            model: makeModel(session: FixtureSession(
                snapshot: nil,
                failure: "The Token Station service stopped: exit status 101.",
                failureKind: .restarting)),
            named: "panel-restarting")
        renderPanel(
            model: makeModel(session: FixtureSession(snapshot: Snapshot(
                schemaVersion: 1,
                revision: 1,
                generatedAt: Int64(Date.now.timeIntervalSince1970),
                meter: Meter(bars: [], level: .normal),
                providers: []
            ))),
            named: "panel-no-providers")
    }

    /// The Settings window, which the panel's own renders never reach.
    private func renderSettings() {
        let json = [
            #"{"general":{"limits_interval_secs":300},"#,
            #""meter":{"window":"most_constrained"},"#,
            #""alerts":{"warning_percent":80,"critical_percent":95,"#,
            #""notify":true,"notify_on_reset":false},"#,
            #""pricing":{"auto_update":true},"#,
            #""claude":{"enabled":true},"codex":{"enabled":true}}"#,
        ].joined()
        guard let settings = try? JSONDecoder().decode(
            JSONValue.self, from: Data(json.utf8)),
            let snapshot = try? loadFixture(named: "snapshot-ok.json")
        else { return }
        // A connected session, because the form only unlocks once the daemon has
        // answered and an empty form is not what this render is for.
        let model = makeModel(session: FixtureSession(snapshot: snapshot, settings: settings))
        for (suffix, appearance) in appearances {
            let view = SettingsView(model: model)
            guard let data = OffscreenRenderer.png(of: view, appearance: appearance) else {
                continue
            }
            write(data, named: "settings-\(suffix).png")
        }
    }

    private func renderPanel(model: AppModel, named name: String) {
        let panel = PanelView(
            model: model,
            isScrollable: false,
            onOpenSettings: {},
            onQuit: {}
        )
        for (suffix, appearance) in appearances {
            let scene = RenderScene(content: panel)
            guard let data = OffscreenRenderer.png(of: scene, appearance: appearance) else {
                continue
            }
            write(data, named: "\(name)-\(suffix).png")
        }
    }

    /// Starts a fixture session and lets its first events land.
    private func makeModel(session: FixtureSession) -> AppModel {
        let defaults = UserDefaults(suiteName: "dev.soldunov.TokenStation.render")
            ?? .standard
        let model = AppModel(session: session, defaults: defaults)
        model.start()
        RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        return model
    }

    // MARK: - Icons

    /// One menu bar state worth a render.
    private struct IconState {
        var name: String
        var meter: Meter
        var showsPercentage = false
    }

    private func renderIcons() {
        for state in Self.iconStates() {
            for (suffix, appearance) in appearances {
                guard let data = IconRenderer.png(
                    meter: state.meter,
                    percentageText: state.showsPercentage
                        ? MeterIcon.percentageText(for: state.meter) : nil,
                    appearance: appearance
                ) else { continue }
                write(data, named: "icon-\(state.name)-\(suffix).png")
            }
        }
    }

    /// Every menu bar state worth a render.
    private static func iconStates() -> [IconState] {
        let empty = Meter(
            bars: [
                MeterBar(provider: .claude, percent: nil, level: .normal, windowId: nil),
                MeterBar(provider: .codex, percent: nil, level: .normal, windowId: nil),
            ],
            level: .normal)
        let normal = Meter(
            bars: [
                MeterBar(provider: .claude, percent: 34, level: .normal, windowId: "session"),
                MeterBar(provider: .codex, percent: 12, level: .normal, windowId: "codex:primary"),
            ],
            level: .normal)
        let warning = Meter(
            bars: [
                MeterBar(provider: .claude, percent: 84, level: .warning, windowId: "session"),
                MeterBar(provider: .codex, percent: 41, level: .normal, windowId: "codex:primary"),
            ],
            level: .warning)
        let critical = Meter(
            bars: [
                MeterBar(provider: .claude, percent: 96, level: .critical, windowId: "session"),
                MeterBar(provider: .codex, percent: 88, level: .warning, windowId: "codex:primary"),
            ],
            level: .critical)
        return [
            IconState(name: "empty", meter: empty),
            IconState(name: "normal", meter: normal),
            IconState(name: "warning", meter: warning),
            IconState(name: "critical", meter: critical),
            IconState(name: "percentage", meter: normal, showsPercentage: true),
        ]
    }

    private func write(_ data: Data, named name: String) {
        let url = output.appending(path: name, directoryHint: .notDirectory)
        do {
            try data.write(to: url)
            written.append(url.path(percentEncoded: false))
        } catch {
            FileHandle.standardError.write(Data("ts-render: \(name): \(error)\n".utf8))
        }
    }
}
