import Foundation
import Testing

@testable import TokenStationCore

@Suite("Snapshot decoding")
struct SnapshotDecodingTests {
    @Test("every fixture in data/fixtures decodes")
    func decodesEveryFixture() throws {
        let urls = try RepositoryFixtures.urls()
        #expect(!urls.isEmpty)
        for url in urls {
            let snapshot = try JSONDecoder().decode(
                Snapshot.self, from: try Data(contentsOf: url))
            #expect(snapshot.schemaVersion == 1, "\(url.lastPathComponent)")
            #expect(snapshot.providers.count == 2, "\(url.lastPathComponent)")
        }
    }

    @Test("the healthy fixture keeps every field")
    func decodesTheHealthyFixture() throws {
        let url = RepositoryFixtures.directory.appending(path: "snapshot-ok.json")
        let snapshot = try JSONDecoder().decode(Snapshot.self, from: try Data(contentsOf: url))

        let claude = try #require(snapshot.providers.first { $0.id == .claude })
        #expect(claude.name == "Claude Code")
        #expect(claude.state == .ok)
        #expect(claude.plan == "Max 20x")
        #expect(claude.windows.count == 3)
        #expect(claude.windows[0].id == "session")
        #expect(claude.windows[0].usedPercent == 34)
        #expect(claude.windows[0].windowMinutes == 300)
        #expect(claude.breakdown.count == 4)
        #expect(claude.credits?.enabled == false)
        #expect(claude.tokens?.today.total == 43_302_460)
        #expect(claude.tokens?.byModel.count == 3)
        #expect(claude.accountTokens == nil)

        let codex = try #require(snapshot.providers.first { $0.id == .codex })
        #expect(codex.accountTokens?.lifetime == 4_743_603_614)
        #expect(codex.accountTokens?.daily.count == 7)
        #expect(snapshot.meter.bars.count == 2)
        #expect(snapshot.meter.level == .normal)
    }

    @Test("a provider with no data decodes its nulls")
    func decodesTheLoadingFixture() throws {
        let url = RepositoryFixtures.directory.appending(path: "snapshot-loading.json")
        let snapshot = try JSONDecoder().decode(Snapshot.self, from: try Data(contentsOf: url))
        for provider in snapshot.providers {
            #expect(provider.state == .loading)
            #expect(provider.windows.isEmpty)
            #expect(provider.tokens == nil)
            #expect(provider.updatedAt == nil)
        }
        #expect(snapshot.meter.bars.allSatisfy { $0.percent == nil })
    }

    @Test("the snake_case state from the daemon maps to its case")
    func decodesNotInstalled() throws {
        let url = RepositoryFixtures.directory.appending(path: "snapshot-not-installed.json")
        let snapshot = try JSONDecoder().decode(Snapshot.self, from: try Data(contentsOf: url))
        let codex = try #require(snapshot.providers.first { $0.id == .codex })
        #expect(codex.state == .notInstalled)
        #expect(codex.message == "Codex CLI not found.")
    }

    @Test("a value a newer daemon invents decodes instead of failing")
    func keepsUnknownEnumValues() throws {
        let json = """
        {"schemaVersion": 2, "revision": 9, "generatedAt": 1790596800,
         "meter": {"bars": [{"provider": "gemini", "percent": 5.0, "level": "elevated"}],
                   "level": "elevated"},
         "providers": [{"id": "gemini", "name": "Gemini", "state": "throttled",
                        "windows": [{"id": "w", "label": "W", "kind": "monthly",
                                     "usedPercent": 5.0, "source": "http",
                                     "observedAt": 1790596800}]}]}
        """
        let snapshot = try JSONDecoder().decode(Snapshot.self, from: Data(json.utf8))
        #expect(snapshot.meter.level == .unknown("elevated"))
        #expect(snapshot.providers[0].id == .unknown("gemini"))
        #expect(snapshot.providers[0].state == .unknown("throttled"))
        #expect(snapshot.providers[0].windows[0].kind == .unknown("monthly"))
    }

    @Test("a missing optional array is empty rather than an error")
    func toleratesMissingKeys() throws {
        let json = """
        {"schemaVersion": 1, "revision": 1, "generatedAt": 0,
         "meter": {"bars": []},
         "providers": [{"id": "claude", "name": "Claude Code", "state": "ok"}]}
        """
        let snapshot = try JSONDecoder().decode(Snapshot.self, from: Data(json.utf8))
        #expect(snapshot.meter.level == .normal)
        #expect(snapshot.providers[0].windows.isEmpty)
        #expect(snapshot.providers[0].breakdown.isEmpty)
    }

    @Test("an alert decodes with every field the daemon sends")
    func decodesAnAlert() throws {
        let json = """
        {"provider": "claude", "providerName": "Claude Code", "windowId": "session",
         "windowLabel": "Session", "kind": "warning", "percent": 82.4,
         "resetsAt": 1790614800, "summary": "s", "body": "b"}
        """
        let alert = try JSONDecoder().decode(Alert.self, from: Data(json.utf8))
        #expect(alert.kind == .warning)
        #expect(alert.percent == 82.4)
        #expect(alert.resetsAt == 1_790_614_800)
    }
}
