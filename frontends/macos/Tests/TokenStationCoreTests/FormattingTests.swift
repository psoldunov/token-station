import Foundation
import Testing

@testable import TokenStationCore

@Suite("Formatting")
struct FormattingTests {
    @Test("a duration is at most two units, largest first")
    func formatsDurations() {
        #expect(Formatting.duration(2 * 3600 + 14 * 60) == "2 hr 14 min")
        #expect(Formatting.duration(4 * 86_400 + 13 * 3600) == "4 days 13 hr")
        #expect(Formatting.duration(0) == "0 min")
    }

    @Test("the units a duration is spelled in follow its size")
    func picksUnitsByMagnitude() {
        // Days carry hours, and nothing under them: the minutes in six days and
        // twenty-three of them are noise, and read like hours if they are shown.
        #expect(Formatting.duration(6 * 86_400 + 23 * 60) == "6 days")
        #expect(Formatting.duration(6 * 86_400 + 3600 + 23 * 60) == "6 days 1 hr")
        // Under a day the pair moves down one.
        #expect(Formatting.duration(3600 + 5 * 60) == "1 hr 5 min")
        #expect(Formatting.duration(3600) == "1 hr")
        // Under an hour there is only one unit to show.
        #expect(Formatting.duration(59 * 60 + 30) == "60 min")
        #expect(Formatting.duration(45) == "1 min")
    }

    @Test("a countdown says what it is counting to")
    func formatsCountdowns() {
        let now = Date(timeIntervalSince1970: 1_790_596_800)
        #expect(Formatting.resetsIn(nil, now: now) == nil)
        #expect(Formatting.resetsIn(now.addingTimeInterval(-5), now: now) == "resetting now")
        #expect(Formatting.resetsIn(now.addingTimeInterval(30), now: now)
            == "resets in under a minute")
        #expect(Formatting.resetsIn(now.addingTimeInterval(8040), now: now)
            == "resets in 2 hr 14 min")
        // The reading that started this: six days and twenty-three minutes.
        #expect(Formatting.resetsIn(now.addingTimeInterval(519_780), now: now)
            == "resets in 6 days")
    }

    @Test("a screen reader hears the same countdown spelled out")
    func formatsCountdownsForVoiceOver() {
        let now = Date(timeIntervalSince1970: 1_790_596_800)
        #expect(Formatting.resetsIn(now.addingTimeInterval(8040), now: now, width: .wide)
            == "resets in 2 hours, 14 minutes")
        #expect(Formatting.resetsIn(now.addingTimeInterval(519_780), now: now, width: .wide)
            == "resets in 6 days")
    }

    @Test("a token count is compact")
    func formatsCounts() {
        #expect(Formatting.compactCount(43_302_460) == "43.3M")
        #expect(Formatting.compactCount(4_743_603_614) == "4.7B")
        #expect(Formatting.compactCount(412) == "412")
    }

    @Test("a percentage is whole")
    func formatsPercentages() {
        #expect(Formatting.percent(34) == "34%")
        #expect(Formatting.percent(96.4) == "96%")
    }

    @Test("an unknown currency code falls back to a plain number")
    func formatsCurrency() {
        #expect(Formatting.currency(31.42, code: "ZZZZ").contains("31.42"))
        #expect(Formatting.currency(31.42, code: "USD").contains("31.42"))
    }

    @Test("the footer dates the snapshot")
    func formatsUpdatedAgo() {
        let now = Date(timeIntervalSince1970: 1_790_596_800)
        #expect(Formatting.updatedAgo(now.addingTimeInterval(-10), now: now) == "Updated just now")
        #expect(Formatting.updatedAgo(now.addingTimeInterval(-120), now: now)
            == "Updated 2 min ago")
    }

    @Test("empty parts do not leave a dangling separator")
    func joinsParts() {
        #expect(Formatting.join(["a", nil, "", "b"]) == "a · b")
        #expect(Formatting.joinSpoken(["a", nil, "b"]) == "a, b")
    }
}
