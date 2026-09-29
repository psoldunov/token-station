import Foundation
import Testing

@testable import TokenStationCore

@Suite("Settings document")
struct SettingsDocumentTests {
    private let document: SettingsDocument = {
        let json = """
        {"general": {"limits_interval_secs": 300, "tokens_interval_secs": 60},
         "meter": {"window": "session"},
         "alerts": {"warning_percent": 80, "critical_percent": 95,
                    "notify": true, "notify_on_reset": false},
         "pricing": {"auto_update": true, "url": "https://example.invalid/prices.json"},
         "claude": {"enabled": true, "binary": ""},
         "codex": {"enabled": false, "homes": ["/a", "/b"]},
         "future": {"invented_later": 7}}
        """
        let value = try? JSONDecoder().decode(JSONValue.self, from: Data(json.utf8))
        return SettingsDocument(json: value ?? .object([:]))
    }()

    @Test("the keys the settings window edits are read back")
    func readsKnownKeys() {
        #expect(document.limitsIntervalSeconds == 300)
        #expect(document.meterWindow == .session)
        #expect(document.warningPercent == 80)
        #expect(document.criticalPercent == 95)
        #expect(document.notify)
        #expect(!document.notifyOnReset)
        #expect(document.pricingAutoUpdate)
        #expect(document.claudeEnabled)
        #expect(!document.codexEnabled)
    }

    @Test("a key this build knows nothing about survives an edit")
    func keepsUnknownKeys() {
        let edited = document.setting("claude.enabled", false)
        #expect(edited.json[path: "future.invented_later"]?.intValue == 7)
        #expect(edited.json[path: "codex.homes"] != nil)
        #expect(edited.json[path: "pricing.url"]?.stringValue
            == "https://example.invalid/prices.json")
        #expect(!edited.claudeEnabled)
    }

    @Test("a missing section is created rather than dropped")
    func createsMissingSections() {
        let empty = SettingsDocument()
        let edited = empty.setting("alerts.notify", false)
        #expect(edited.json[path: "alerts.notify"]?.boolValue == false)
    }

    @Test("the warning threshold cannot pass the critical one")
    func keepsThresholdsOrdered() {
        let raised = document.settingWarningPercent(98)
        #expect(raised.warningPercent == 98)
        #expect(raised.criticalPercent == 98)

        let lowered = document.settingCriticalPercent(40)
        #expect(lowered.criticalPercent == 40)
        #expect(lowered.warningPercent == 40)
    }

    @Test("a threshold outside 1–100 is pulled back in")
    func clampsThresholds() {
        #expect(document.settingWarningPercent(-10).warningPercent == 1)
        #expect(document.settingCriticalPercent(400).criticalPercent == 100)
    }

    @Test("an interval the pop-up has no preset for is still offered")
    func keepsACustomInterval() {
        #expect(LimitsInterval.choices(including: 300) == LimitsInterval.presets)
        #expect(LimitsInterval.choices(including: 450).contains(450))
        #expect(LimitsInterval.title(300) == "Every 5 Minutes")
        #expect(LimitsInterval.title(3600) == "Every Hour")
        #expect(LimitsInterval.title(450).hasPrefix("Every "))
    }

    @Test("a dotted path reads through nested objects")
    func readsPaths() {
        #expect(document.json[path: "codex.homes"] != nil)
        #expect(document.json[path: "codex.missing"] == nil)
        #expect(document.json[path: "general.limits_interval_secs"]?.intValue == 300)
    }
}
