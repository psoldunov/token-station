import Foundation
import Testing
import TokenStationCore

@testable import TokenStationUI

@MainActor
@Suite("App model history", .timeLimit(.minutes(1)))
struct AppModelHistoryTests {
    private static let day: Int64 = 86_400
    private static let week: Int64 = 7 * 86_400

    private let harness: AppHarness
    private let snapshot: Snapshot
    private let claudeKey = HistoryKey(provider: .claude, windowId: "session")
    private let codexKey = HistoryKey(provider: .codex, windowId: "codex:primary")

    init() throws {
        harness = try AppHarness()
        harness.session.holdHistory()
        snapshot = try RepositoryFixtures.snapshot(named: "ok")
    }

    private var model: AppModel { harness.model }

    private func connect(_ snapshot: Snapshot) async {
        await harness.start()
        harness.session.emit(.connected(snapshot: snapshot, daemonVersion: nil))
        await settle { model.phase == .running }
    }

    /// Answers every history call the connect just made, empty.
    private func answerRound(count: Int) async {
        for _ in 0..<count {
            await harness.session.histories.next().succeed([])
        }
    }

    private func expectSince(_ request: HistoryRequest, span: Int64) {
        let expected = Int64(Date.now.timeIntervalSince1970) - span
        #expect(abs(request.since - expected) < 120)
    }

    private func points(_ base: Int64) -> [HistoryPoint] {
        [HistoryPoint(timestamp: base, percent: 10), HistoryPoint(timestamp: base + 60, percent: 20)]
    }

    @Test("connecting asks for one series per provider, over the span of its meter window")
    func requestsPerProvider() async {
        await connect(snapshot)
        let claude = await harness.session.histories.next()
        #expect(claude.input.provider == .claude)
        #expect(claude.input.windowId == "session")
        expectSince(claude.input, span: Self.day)
        claude.succeed(points(1_000))

        let codex = await harness.session.histories.next()
        #expect(codex.input.provider == .codex)
        #expect(codex.input.windowId == "codex:primary")
        expectSince(codex.input, span: Self.week)
        codex.succeed(points(2_000))

        await settle { model.history[codexKey] != nil }
        #expect(model.history[claudeKey] == points(1_000))
        #expect(model.history[codexKey] == points(2_000))
        let claudeSnapshot = snapshot.providers.first { $0.id == .claude }
        #expect(claudeSnapshot.map(model.historyPoints(for:)) == points(1_000))
    }

    struct WindowCase: Sendable, CustomTestStringConvertible {
        let barWindow: String?
        let expectedWindow: String
        let expectedSpan: Int64
        var testDescription: String { "bar names \(barWindow ?? "nothing")" }
    }

    @Test("the window comes from the meter bar, else the provider's first window", arguments: [
        WindowCase(barWindow: "weekly_all", expectedWindow: "weekly_all", expectedSpan: 7 * 86_400),
        WindowCase(barWindow: "weekly_scoped:fable", expectedWindow: "weekly_scoped:fable", expectedSpan: 86_400),
        WindowCase(barWindow: "vanished", expectedWindow: "session", expectedSpan: 86_400),
        WindowCase(barWindow: nil, expectedWindow: "session", expectedSpan: 86_400),
    ])
    func meterWindowChoice(_ testCase: WindowCase) async {
        var edited = snapshot
        edited.providers = edited.providers.filter { $0.id == .claude }
        edited.meter.bars = [
            MeterBar(provider: .claude, percent: 40, level: .normal, windowId: testCase.barWindow),
        ]
        await connect(edited)

        let request = await harness.session.histories.next()
        #expect(request.input.provider == .claude)
        #expect(request.input.windowId == testCase.expectedWindow)
        expectSince(request.input, span: testCase.expectedSpan)
        request.succeed([])
    }

    @Test("a provider with no windows is skipped")
    func skipsProviderWithoutWindows() async {
        var edited = snapshot
        edited.providers = edited.providers.map { provider in
            guard provider.id == .codex else { return provider }
            var bare = provider
            bare.windows = []
            return bare
        }
        await connect(edited)

        let claude = await harness.session.histories.next()
        #expect(claude.input.provider == .claude)
        claude.succeed([])
        await flush()
        #expect(await harness.session.histories.pendingCount == 0)
        let codexProvider = edited.providers.first { $0.id == .codex }
        #expect(codexProvider.map(model.historyPoints(for:))?.isEmpty == true)
    }

    @Test("a series that fails to load leaves the others alone")
    func failureIsolated() async {
        await connect(snapshot)
        await harness.session.histories.next().fail(TransportDown())
        await harness.session.histories.next().succeed(points(2_000))

        await settle { model.history[codexKey] != nil }
        #expect(model.history[claudeKey] == nil)
        #expect(model.history[codexKey] == points(2_000))
    }

    @Test("a later snapshot does not refetch within a minute, but opening the panel does")
    func throttle() async {
        await connect(snapshot)
        await answerRound(count: 2)
        await harness.drain()

        var newer = snapshot
        newer.revision += 1
        harness.session.emit(.snapshot(newer))
        await settle { model.snapshot == newer }
        await flush()
        #expect(await harness.session.histories.pendingCount == 0)

        model.panelDidOpen()
        let request = await harness.session.histories.next()
        #expect(request.input.provider == .claude)
        request.succeed([])
        await harness.session.histories.next().succeed([])
    }

    @Test("opening the panel before the daemon has answered asks for nothing")
    func openBeforeConnected() async {
        model.panelDidOpen()
        await flush()
        #expect(await harness.session.histories.pendingCount == 0)
        #expect(model.history.isEmpty)
    }

    @Test("a provider's series is empty until it has been fetched")
    func emptyUntilFetched() async {
        await connect(snapshot)
        let claudeSnapshot = snapshot.providers.first { $0.id == .claude }
        #expect(claudeSnapshot.map(model.historyPoints(for:))?.isEmpty == true)
        await answerRound(count: 2)
    }
}
